//! The session capability gates the transport, and never travels on it.
use deepika_sync::{
    transport::{self, ALPN, WireMessage},
    workspace::Workspace,
};
use futures_util::{SinkExt, StreamExt};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

const SECRET: &str = "correct-secret";

/// Dial the daemon, answer its Hello with `proof_for(attacker, daemon)`, and return
/// the daemon's Hello bytes (unless it hung up first) plus how many document frames followed.
async fn dial(capability_used: &str) -> (Option<Vec<u8>>, usize) {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();
    ws.set_capability(SECRET).await.unwrap();
    ws.create_document("secret.md", "confidential")
        .await
        .unwrap();
    let ep = transport::endpoint(&ws, false).await.unwrap();
    let router = transport::run(ws.clone(), ep.clone(), None).await.unwrap();

    let client = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .bind()
        .await
        .unwrap();
    let conn = client.connect(ep.addr(), ALPN).await.unwrap();
    let (tx, rx) = conn.open_bi().await.unwrap();
    let mut writer = FramedWrite::new(tx, LengthDelimitedCodec::new());
    let mut reader = FramedRead::new(rx, LengthDelimitedCodec::new());

    let hello = WireMessage::Hello {
        proof: *transport::proof(capability_used, &client.id(), &ep.id()).as_bytes(),
    };
    writer
        .send(postcard::to_stdvec(&hello).unwrap().into())
        .await
        .unwrap();

    let their_hello = match reader.next().await {
        Some(Ok(frame)) => Some(frame.to_vec()),
        _ => None,
    };
    let mut doc_frames = 0;
    while let Ok(Some(Ok(frame))) =
        tokio::time::timeout(std::time::Duration::from_millis(500), reader.next()).await
    {
        if matches!(
            postcard::from_bytes(&frame),
            Ok(WireMessage::DocSync { .. })
        ) {
            doc_frames += 1;
        }
    }
    client.close().await;
    router.shutdown().await.unwrap();
    (their_hello, doc_frames)
}

/// The Hello must carry a proof, never the capability.
fn leaks_secret(frame: &[u8]) -> bool {
    frame.windows(SECRET.len()).any(|w| w == SECRET.as_bytes())
}

#[tokio::test]
async fn wrong_capability_receives_no_documents_and_learns_no_secret() {
    let (their_hello, doc_frames) = dial("wrong-capability").await;
    assert_eq!(doc_frames, 0);
    assert!(!leaks_secret(&their_hello.unwrap_or_default()));
}

#[tokio::test]
async fn right_capability_is_offered_the_documents() {
    let (their_hello, doc_frames) = dial(SECRET).await;
    assert_eq!(doc_frames, 1);
    let their_hello = their_hello.expect("an accepted peer reads the daemon's Hello");
    assert!(matches!(
        postcard::from_bytes(&their_hello),
        Ok(WireMessage::Hello { .. })
    ));
    assert!(!leaks_secret(&their_hello));
}
