use deepika_sync::{workspace::Workspace, ws};
use futures_util::{SinkExt, StreamExt};
use tempfile::TempDir;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};
use yrs::sync::{Message, SyncMessage};
use yrs::updates::{decoder::Decode, encoder::Encode};

#[tokio::test]
async fn websocket_y_sync_handshake_and_updates() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();
    let doc_id = ws.create_document("note.md", "initial text").await.unwrap();

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 49876));
    let server_ws = ws.clone();
    let server_task = tokio::spawn(async move {
        ws::serve(server_ws, addr).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connect client to websocket
    let url = format!("ws://127.0.0.1:49876/{}", doc_id);
    let (ws_stream, _) = connect_async(url).await.expect("connect to websocket");
    let (mut client_tx, mut client_rx) = ws_stream.split();

    // 1. Receive initial SyncStep1 from server
    let first_msg = client_rx.next().await.unwrap().unwrap();
    if let WsMessage::Binary(bytes) = first_msg {
        let y_msg = Message::decode_v1(&bytes).expect("decode Message");
        assert!(matches!(y_msg, Message::Sync(SyncMessage::SyncStep1(_))));
    } else {
        panic!("expected binary message");
    }

    // 2. Client sends SyncStep1
    let client_step1 =
        Message::Sync(SyncMessage::SyncStep1(yrs::StateVector::default())).encode_v1();
    client_tx
        .send(WsMessage::Binary(client_step1.into()))
        .await
        .unwrap();

    // 3. Receive SyncStep2 with document contents
    let reply_msg = client_rx.next().await.unwrap().unwrap();
    if let WsMessage::Binary(bytes) = reply_msg {
        let y_msg = Message::decode_v1(&bytes).expect("decode Message");
        assert!(matches!(y_msg, Message::Sync(SyncMessage::SyncStep2(_))));
    } else {
        panic!("expected binary message");
    }

    server_task.abort();
}

#[tokio::test]
async fn websocket_multi_doc_url_routing() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 49877));
    let server_ws = ws.clone();
    let server_task = tokio::spawn(async move {
        ws::serve(server_ws, addr).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connect to a specific document path via URL
    let url = "ws://127.0.0.1:49877/notes/architecture.md";
    let (ws_stream, _) = connect_async(url).await.expect("connect to websocket");
    let (_client_tx, mut client_rx) = ws_stream.split();

    // Expect initial SyncStep1
    let first_msg = client_rx.next().await.unwrap().unwrap();
    assert!(matches!(first_msg, WsMessage::Binary(_)));

    // Verify document was automatically routed and created
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let doc_id = ws
        .get_doc_id_by_path("notes/architecture.md")
        .await
        .expect("routed doc created");
    assert!(ws.is_leased(&doc_id).await);

    server_task.abort();
}

#[tokio::test]
async fn websocket_refuses_browser_origins() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    assert!(ws::origin_allowed(None));
    assert!(ws::origin_allowed(Some("app://obsidian.md")));
    assert!(!ws::origin_allowed(Some("https://evil.example")));

    let dir = TempDir::new().unwrap();
    let workspace = Workspace::open(dir.path()).await.unwrap();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 49879));
    let server_task = tokio::spawn(ws::serve(workspace.clone(), addr));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let mut request = "ws://127.0.0.1:49879/stolen.md"
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", "https://evil.example".parse().unwrap());
    assert!(connect_async(request).await.is_err());
    assert!(workspace.get_doc_id_by_path("stolen.md").await.is_none());
    server_task.abort();
}

#[tokio::test]
async fn websocket_relays_updates_and_awareness_between_clients() {
    let dir = TempDir::new().unwrap();
    let workspace = Workspace::open(dir.path()).await.unwrap();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 49880));
    let server_task = tokio::spawn(ws::serve(workspace.clone(), addr));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let url = "ws://127.0.0.1:49880/caf%C3%A9.md";
    let (mut alice, _) = connect_async(url).await.unwrap();
    let (mut bob, _) = connect_async(url).await.unwrap();
    alice.next().await.unwrap().unwrap(); // server SyncStep1
    bob.next().await.unwrap().unwrap();
    assert!(workspace.get_doc_id_by_path("café.md").await.is_some());

    // Alice edits her replica and announces her cursor.
    let doc = deepika_sync::workspace::new_doc();
    let mut awareness = yrs::sync::Awareness::new(doc.clone());
    awareness.set_local_state("{\"cursor\":1}").unwrap();
    let update = {
        use yrs::{Text, Transact};
        let text = doc.get_or_insert_text("content");
        let mut txn = doc.transact_mut();
        text.push(&mut txn, "salut");
        txn.encode_update_v1()
    };
    for msg in [
        Message::Sync(SyncMessage::Update(update)),
        Message::Awareness(awareness.update().unwrap()),
    ] {
        alice
            .send(WsMessage::Binary(msg.encode_v1().into()))
            .await
            .unwrap();
    }

    // Bob receives both; the awareness message comes through untouched.
    let mut updates = 0;
    loop {
        let WsMessage::Binary(bytes) = bob.next().await.unwrap().unwrap() else {
            panic!()
        };
        match Message::decode_v1(&bytes).unwrap() {
            Message::Sync(SyncMessage::Update(_)) => updates += 1,
            Message::Awareness(_) => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(updates >= 1);
    assert_eq!(
        workspace
            .get_text(&workspace.get_doc_id_by_path("café.md").await.unwrap())
            .await
            .unwrap(),
        "salut"
    );
    // Alice gets no echo of her own messages.
    let echo = tokio::time::timeout(std::time::Duration::from_millis(200), alice.next()).await;
    assert!(echo.is_err(), "no echo to the sender");

    // Carol arrives later: she is told at once who is already in the note, and may ask again.
    let (mut carol, _) = connect_async(url).await.unwrap();
    carol.next().await.unwrap().unwrap(); // server SyncStep1
    for ask in [false, true] {
        if ask {
            let query = Message::AwarenessQuery.encode_v1();
            carol.send(WsMessage::Binary(query.into())).await.unwrap();
        }
        let WsMessage::Binary(bytes) = carol.next().await.unwrap().unwrap() else {
            panic!()
        };
        let Message::Awareness(present) = Message::decode_v1(&bytes).unwrap() else {
            panic!("expected who is present")
        };
        assert!(present.clients[&doc.client_id()].json.contains("cursor"));
    }
    // Once Alice leaves, nothing is left to tell.
    awareness.clean_local_state();
    let gone = Message::Awareness(awareness.update_with_clients([doc.client_id()]).unwrap());
    alice
        .send(WsMessage::Binary(gone.encode_v1().into()))
        .await
        .unwrap();
    bob.next().await.unwrap().unwrap();
    assert!(
        workspace
            .presence(&workspace.get_doc_id_by_path("café.md").await.unwrap())
            .is_none()
    );
    server_task.abort();
}
