//! deepika-sync in a browser tab.
//!
//! The page holds a throwaway iroh endpoint. Browsers cannot send UDP, so every packet
//! goes through the relay named in the invitation -- still end-to-end encrypted: the
//! relay forwards bytes it cannot read. On top of that, this speaks the daemon's own
//! protocol (`deepika-sync/yrs/2`): length-prefixed postcard frames, a mutual proof of the
//! session capability, then y-sync messages per document.
//!
//! The page owns the documents (`Y.Doc`, in JavaScript); this side only carries frames.
//! `Session::connect` authenticates and returns the manifest; from then on every y-sync
//! message the session sends is handed to a JavaScript callback, and `send` takes the
//! page's own messages the other way.
use iroh::{
    Endpoint, EndpointAddr, EndpointId,
    endpoint::{RecvStream, SendStream, presets},
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use wasm_bindgen::prelude::*;

const ALPN: &[u8] = b"deepika-sync/yrs/2";
const MAX_FRAME: usize = 64 * 1024 * 1024;

#[derive(Deserialize)]
struct Invite {
    version: u32,
    address: EndpointAddr,
    capability: String,
}

/// Mirrors `deepika_sync::preview::ManifestEntry`.
#[derive(Serialize, Deserialize)]
struct ManifestEntry {
    id: String,
    path: String,
    deleted: bool,
    hash: [u8; 32],
}

/// Mirrors `deepika_sync::transport::WireMessage`; postcard encodes variants by position,
/// so the order is part of the protocol.
#[derive(Serialize, Deserialize)]
enum WireMessage {
    Hello { proof: [u8; 32] },
    DocSync { doc_id: String, payload: Vec<u8> },
    ManifestRequest,
    Manifest(Vec<ManifestEntry>),
}

fn proof(capability: &str, from: &EndpointId, to: &EndpointId) -> [u8; 32] {
    let key = blake3::hash(capability.as_bytes());
    *blake3::Hasher::new_keyed(key.as_bytes())
        .update(from.as_bytes())
        .update(to.as_bytes())
        .finalize()
        .as_bytes()
}

/// Frames are what `tokio_util`'s `LengthDelimitedCodec` writes: a big-endian u32 length.
async fn send(stream: &mut SendStream, msg: &WireMessage) -> Result<(), String> {
    let body = postcard::to_allocvec(msg).map_err(|e| e.to_string())?;
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.write_all(&body).await.map_err(|e| e.to_string())
}

async fn receive(stream: &mut RecvStream) -> Result<WireMessage, String> {
    let mut length = [0u8; 4];
    stream
        .read_exact(&mut length)
        .await
        .map_err(|e| e.to_string())?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err("frame_too_large".into());
    }
    let mut body = vec![0u8; length];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| e.to_string())?;
    postcard::from_bytes(&body).map_err(|e| e.to_string())
}

struct Joined {
    endpoint: Endpoint,
    tx: SendStream,
    rx: RecvStream,
    manifest: Vec<ManifestEntry>,
}

async fn join(invitation: &str) -> Result<Joined, String> {
    let raw = data_encoding::BASE64URL_NOPAD
        .decode(invitation.trim().as_bytes())
        .map_err(|e| format!("invitation: {e}"))?;
    let invite: Invite = serde_json::from_slice(&raw).map_err(|e| format!("invitation: {e}"))?;
    if invite.version != 1 {
        return Err("unsupported_invite".into());
    }
    let has_relay = invite
        .address
        .addrs
        .iter()
        .any(|a| matches!(a, iroh::TransportAddr::Relay(_)));
    if !has_relay {
        return Err("this invitation names no relay: the host must share with --relay".into());
    }

    let endpoint = Endpoint::builder(presets::N0)
        .bind()
        .await
        .map_err(|e| e.to_string())?;
    let me = endpoint.id();
    let peer = invite.address.id;
    let connection = endpoint
        .connect(invite.address, ALPN)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let (mut tx, mut rx) = connection.open_bi().await.map_err(|e| e.to_string())?;

    send(
        &mut tx,
        &WireMessage::Hello {
            proof: proof(&invite.capability, &me, &peer),
        },
    )
    .await?;
    send(&mut tx, &WireMessage::ManifestRequest).await?;
    let mut greeted = false;
    let manifest = loop {
        match receive(&mut rx).await? {
            WireMessage::Hello { proof: theirs } => {
                if theirs != proof(&invite.capability, &peer, &me) {
                    return Err("capability_mismatch".into());
                }
                greeted = true;
            }
            WireMessage::Manifest(entries) if greeted => break entries,
            _ => {}
        }
    };
    Ok(Joined {
        endpoint,
        tx,
        rx,
        manifest,
    })
}

/// A live connection to a session.
#[wasm_bindgen]
pub struct Session {
    manifest: String,
    outgoing: mpsc::UnboundedSender<WireMessage>,
    // Dropping the endpoint would close the connection under the two tasks.
    _endpoint: Endpoint,
}

#[wasm_bindgen]
impl Session {
    /// Join the session named by `invitation`. `on_message(docId, Uint8Array)` receives every
    /// y-sync message of the session; `on_close(reason)` is called once when the link ends.
    pub async fn connect(
        invitation: String,
        on_message: js_sys::Function,
        on_close: js_sys::Function,
    ) -> Result<Session, JsError> {
        console_error_panic_hook::set_once();
        let Joined {
            endpoint,
            mut tx,
            mut rx,
            manifest,
        } = join(&invitation).await.map_err(|e| JsError::new(&e))?;
        let listing: Vec<_> = manifest
            .iter()
            .map(|e| serde_json::json!({ "id": e.id, "path": e.path, "deleted": e.deleted }))
            .collect();

        let (outgoing, mut queue) = mpsc::unbounded_channel::<WireMessage>();
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(msg) = queue.recv().await {
                if send(&mut tx, &msg).await.is_err() {
                    break;
                }
            }
        });
        wasm_bindgen_futures::spawn_local(async move {
            let reason = loop {
                match receive(&mut rx).await {
                    Ok(WireMessage::DocSync { doc_id, payload }) => {
                        let bytes = js_sys::Uint8Array::from(payload.as_slice());
                        let _ =
                            on_message.call2(&JsValue::NULL, &JsValue::from_str(&doc_id), &bytes);
                    }
                    Ok(_) => {}
                    Err(e) => break e,
                }
            };
            let _ = on_close.call1(&JsValue::NULL, &JsValue::from_str(&reason));
        });
        Ok(Session {
            manifest: serde_json::to_string(&listing).map_err(|e| JsError::new(&e.to_string()))?,
            outgoing,
            _endpoint: endpoint,
        })
    }

    /// What the session held when we joined: a JSON array of `{id, path, deleted}`.
    #[wasm_bindgen(getter)]
    pub fn manifest(&self) -> String {
        self.manifest.clone()
    }

    /// Send one y-sync message about a document.
    pub fn send(&self, doc_id: String, payload: Vec<u8>) {
        let _ = self.outgoing.send(WireMessage::DocSync { doc_id, payload });
    }
}
