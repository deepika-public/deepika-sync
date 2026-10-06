//! P2P transport: Iroh QUIC streams carrying the standard y-sync binary protocol,
//! one frame per document message.

use crate::{discovery, preview::ManifestEntry, workspace::Workspace};
use anyhow::{Result, anyhow, bail, ensure};
use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use futures_util::{SinkExt, StreamExt};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast::error::RecvError, mpsc};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use yrs::{
    sync::{Message, SyncMessage},
    updates::{decoder::Decode, encoder::Encode},
};

pub const ALPN: &[u8] = b"deepika-sync/yrs/2";
const MAX_FRAME: usize = 64 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Invite {
    pub version: u32,
    pub address: EndpointAddr,
    pub capability: String,
}

impl Invite {
    pub fn encode(&self) -> Result<String> {
        Ok(BASE64URL_NOPAD.encode(&serde_json::to_vec(self)?))
    }
    pub fn decode(s: &str) -> Result<Self> {
        let v: Self = serde_json::from_slice(&BASE64URL_NOPAD.decode(s.as_bytes())?)?;
        ensure!(v.version == 1, "unsupported_invite");
        Ok(v)
    }
}

/// A fresh 32-byte secret, hex encoded: session capabilities and endpoint keys.
pub fn new_secret() -> String {
    HEXLOWER.encode(&rand::random::<[u8; 32]>())
}

#[derive(Serialize, Deserialize, Debug)]
pub enum WireMessage {
    /// Proves knowledge of the session capability without ever sending it.
    Hello {
        proof: [u8; 32],
    },
    DocSync {
        doc_id: String,
        payload: Vec<u8>,
    },
    /// Asked by a peer weighing whether to join: what does the session hold?
    ManifestRequest,
    /// Paths and text hashes, never text. Sent only after the proof checked out.
    Manifest(Vec<ManifestEntry>),
}

/// Keyed by the capability and bound to both endpoint ids, so a proof captured from
/// one connection opens no other.
pub fn proof(capability: &str, from: &EndpointId, to: &EndpointId) -> blake3::Hash {
    let key = blake3::hash(capability.as_bytes());
    blake3::Hasher::new_keyed(key.as_bytes())
        .update(from.as_bytes())
        .update(to.as_bytes())
        .finalize()
}

pub async fn endpoint(workspace: &Workspace, relay: bool) -> Result<Endpoint> {
    let storage = &workspace.storage;
    let raw = match storage.get_config("secret")? {
        Some(s) => s,
        None => {
            let s = new_secret();
            storage.set_config("secret", &s)?;
            s
        }
    };
    let key = SecretKey::from_bytes(
        &HEXLOWER
            .decode(raw.as_bytes())?
            .try_into()
            .map_err(|_| anyhow!("invalid_key"))?,
    );
    let port: u16 = storage
        .get_config("bind_port")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let builder = if relay {
        Endpoint::builder(presets::N0)
    } else {
        Endpoint::builder(presets::Minimal)
    };
    let ep = builder
        .bind_addr(std::net::SocketAddr::from(([0, 0, 0, 0], port)))?
        .transport_config(
            iroh::endpoint::QuicTransportConfig::builder()
                .max_idle_timeout(Some(Duration::from_secs(15).try_into()?))
                .keep_alive_interval(Duration::from_secs(5))
                .build(),
        )
        .secret_key(key)
        .bind()
        .await?;

    if let Some(addr) = ep.bound_sockets().iter().find(|addr| addr.is_ipv4()) {
        storage.set_config("bind_port", &addr.port().to_string())?;
    }
    if relay {
        let _ = tokio::time::timeout(Duration::from_secs(15), ep.online()).await;
    }
    Ok(ep)
}

/// Live connections per peer, so a peer we already talk to is not dialed again.
type Live = Arc<Mutex<HashMap<EndpointId, usize>>>;

#[derive(Debug, Clone)]
struct Sync {
    workspace: Workspace,
    capability: String,
    me: EndpointId,
    live: Live,
    /// Peers a `keep_connected` task already looks after.
    kept: Arc<Mutex<HashSet<EndpointId>>>,
}

impl ProtocolHandler for Sync {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        self.serve(conn, true)
            .await
            .map_err(|e| AcceptError::from_boxed(e.into()))
    }
}

impl Sync {
    async fn serve(&self, conn: Connection, incoming: bool) -> Result<()> {
        let peer = conn.remote_id();
        *self.live.lock().unwrap().entry(peer).or_default() += 1;
        let result = self.converse(conn, incoming).await;
        match &result {
            Ok(()) => tracing::info!(peer = %peer, "peer_disconnected"),
            Err(e) => tracing::info!(peer = %peer, error = %e, "peer_disconnected"),
        }
        let mut live = self.live.lock().unwrap();
        if let Some(n) = live.get_mut(&peer) {
            *n -= 1;
            if *n == 0 {
                live.remove(&peer);
            }
        }
        result
    }

    async fn converse(&self, conn: Connection, incoming: bool) -> Result<()> {
        let peer = conn.remote_id();
        let (send, recv) = if incoming {
            conn.accept_bi().await?
        } else {
            conn.open_bi().await?
        };
        let codec = || {
            LengthDelimitedCodec::builder()
                .max_frame_length(MAX_FRAME)
                .new_codec()
        };
        let mut writer = FramedWrite::new(send, codec());
        let mut reader = FramedRead::new(recv, codec());
        let frame = |doc_id: &str, payload: Vec<u8>| -> Result<tokio_util::bytes::Bytes> {
            let wire = WireMessage::DocSync {
                doc_id: doc_id.to_string(),
                payload,
            };
            Ok(postcard::to_stdvec(&wire)?.into())
        };

        // Mutual proof first; nothing about the documents leaves before it checks out.
        let hello = WireMessage::Hello {
            proof: *proof(&self.capability, &self.me, &peer).as_bytes(),
        };
        writer.send(postcard::to_stdvec(&hello)?.into()).await?;
        let first = reader
            .next()
            .await
            .ok_or_else(|| anyhow!("connection_closed_before_hello"))??;
        match postcard::from_bytes(&first)? {
            WireMessage::Hello { proof: theirs } => ensure!(
                blake3::Hash::from_bytes(theirs) == proof(&self.capability, &peer, &self.me),
                "capability_mismatch"
            ),
            _ => bail!("expected_hello"),
        }

        tracing::info!(peer = %peer, incoming, "peer_connected");
        let conn_id: u64 = rand::random();
        // Subscribe before the first state vector so no update falls in between.
        let mut events = self.workspace.events.subscribe();
        let workspace = &self.workspace;
        for doc in workspace.list_documents().await {
            let sv = workspace.encode_state_vector(&doc.id).await?;
            let payload = Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1();
            writer.send(frame(&doc.id, payload)?).await?;
            if let Some(present) = workspace.presence(&doc.id) {
                writer.send(frame(&doc.id, present.encode_v1())?).await?;
            }
        }

        loop {
            tokio::select! {
                incoming = reader.next() => {
                    let Some(Ok(bytes)) = incoming else { break };
                    let (doc_id, payload) = match postcard::from_bytes(&bytes) {
                        Ok(WireMessage::DocSync { doc_id, payload }) => (doc_id, payload),
                        Ok(WireMessage::ManifestRequest) => {
                            let manifest = WireMessage::Manifest(workspace.manifest().await);
                            writer.send(postcard::to_stdvec(&manifest)?.into()).await?;
                            continue;
                        }
                        _ => continue,
                    };
                    let Ok(msg) = Message::decode_v1(&payload) else { continue };
                    match workspace.handle_y_msg(&doc_id, msg, Some(conn_id)).await {
                        Ok(Some(reply)) => writer.send(frame(&doc_id, reply.encode_v1())?).await?,
                        Ok(None) => {}
                        Err(e) => tracing::warn!(doc_id = %doc_id, error = %e, "remote_message_rejected"),
                    }
                }
                event = events.recv() => match event {
                    Ok(event) if event.origin() != Some(conn_id) => {
                        if let Some(payload) = event.wire() {
                            writer.send(frame(event.id(), payload)?).await?;
                        }
                    }
                    Ok(_) => {}
                    // We missed events and cannot tell which: resend full states, merging is idempotent.
                    Err(RecvError::Lagged(_)) => {
                        for doc in workspace.list_documents().await {
                            let diff = workspace.encode_diff(&doc.id, &Default::default()).await?;
                            let payload = Message::Sync(SyncMessage::Update(diff)).encode_v1();
                            writer.send(frame(&doc.id, payload)?).await?;
                        }
                    }
                    Err(RecvError::Closed) => break,
                },
            }
        }
        Ok(())
    }
}

/// Peers this folder has talked to, remembered across restarts. At a distance there is
/// no mDNS to find them again: a guest who restarts must call its host back.
fn known_peers(workspace: &Workspace) -> Vec<EndpointAddr> {
    workspace
        .storage
        .get_config("peers")
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn remember_peer(workspace: &Workspace, addr: &EndpointAddr) {
    let mut peers = known_peers(workspace);
    // The latest address of a peer replaces the one we knew.
    peers.retain(|p| p.id != addr.id);
    peers.push(addr.clone());
    if let Ok(raw) = serde_json::to_string(&peers) {
        let _ = workspace.storage.set_config("peers", &raw);
    }
}

/// How long to wait before calling a peer again: 1 s, doubling up to this.
const REDIAL_CEILING: Duration = Duration::from_secs(30);

/// Keep one peer connected for as long as the daemon runs. A connection that ends --
/// network cut, sleep, the peer restarting -- is dialed again, with a growing pause.
fn keep_connected(sync: Sync, ep: Endpoint, addr: EndpointAddr) {
    if addr.id == sync.me || !sync.kept.lock().unwrap().insert(addr.id) {
        return;
    }
    tokio::spawn(async move {
        let mut pause = Duration::from_secs(1);
        let attempts = async {
            loop {
                // The peer may have called us first: one connection is enough.
                if !sync.live.lock().unwrap().contains_key(&addr.id) {
                    match ep.connect(addr.clone(), ALPN).await {
                        Ok(conn) => {
                            let started = std::time::Instant::now();
                            let _ = sync.serve(conn, false).await;
                            if started.elapsed() > REDIAL_CEILING {
                                pause = Duration::from_secs(1);
                            }
                        }
                        Err(e) => tracing::debug!(peer = %addr.id, error = %e, "dial_failed"),
                    }
                }
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(REDIAL_CEILING);
            }
        };
        // Leaving the session closes the endpoint; holding on to it past that point
        // would keep its port bound and block the next start.
        tokio::select! {
            _ = ep.closed() => {}
            () = attempts => {}
        }
    });
}

/// Serve the session: accept peers, and keep calling the inviter, the peers remembered
/// from earlier runs, and whoever mDNS finds.
/// The returned router must be kept alive; shut it down to leave the session.
pub async fn run(
    workspace: Workspace,
    ep: Endpoint,
    inviter: Option<EndpointAddr>,
) -> Result<Router> {
    if let Some(addr) = &inviter {
        remember_peer(&workspace, addr);
    }
    let peers = known_peers(&workspace);
    let sync = Sync {
        capability: workspace.capability().await,
        workspace,
        me: ep.id(),
        live: Live::default(),
        kept: Default::default(),
    };
    let router = Router::builder(ep.clone())
        .accept(ALPN, sync.clone())
        .spawn();

    for addr in peers {
        keep_connected(sync.clone(), ep.clone(), addr);
    }
    let (dial_tx, mut dial_rx) = mpsc::channel::<EndpointAddr>(64);
    discovery::start(&ep, &sync.capability, dial_tx);
    tokio::spawn(async move {
        let closed = ep.closed();
        tokio::pin!(closed);
        loop {
            tokio::select! {
                _ = &mut closed => break,
                found = dial_rx.recv() => match found {
                    Some(addr) => keep_connected(sync.clone(), ep.clone(), addr),
                    None => break,
                },
            }
        }
    });
    Ok(router)
}

/// Ask the inviter what the session holds, as a stranger to the folder: a throwaway
/// endpoint, nothing read from or written to disk. The proof is still required.
pub async fn fetch_manifest(invite: &Invite, relay: bool) -> Result<Vec<ManifestEntry>> {
    let builder = if relay {
        Endpoint::builder(presets::N0)
    } else {
        Endpoint::builder(presets::Minimal)
    };
    let ep = builder.bind().await?;
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        if relay {
            let _ = tokio::time::timeout(Duration::from_secs(15), ep.online()).await;
        }
        let peer = invite.address.id;
        let conn = ep.connect(invite.address.clone(), ALPN).await?;
        let (send, recv) = conn.open_bi().await?;
        let codec = || {
            LengthDelimitedCodec::builder()
                .max_frame_length(MAX_FRAME)
                .new_codec()
        };
        let mut writer = FramedWrite::new(send, codec());
        let mut reader = FramedRead::new(recv, codec());
        let hello = WireMessage::Hello {
            proof: *proof(&invite.capability, &ep.id(), &peer).as_bytes(),
        };
        writer.send(postcard::to_stdvec(&hello)?.into()).await?;
        writer
            .send(postcard::to_stdvec(&WireMessage::ManifestRequest)?.into())
            .await?;
        let mut greeted = false;
        while let Some(frame) = reader.next().await {
            match postcard::from_bytes(&frame?)? {
                WireMessage::Hello { proof: theirs } => {
                    ensure!(
                        blake3::Hash::from_bytes(theirs)
                            == proof(&invite.capability, &peer, &ep.id()),
                        "capability_mismatch"
                    );
                    greeted = true;
                }
                WireMessage::Manifest(entries) if greeted => return Ok(entries),
                _ => {}
            }
        }
        bail!("session_closed_before_manifest")
    })
    .await
    .map_err(|_| anyhow!("inviter_unreachable: l'hôte de l'invitation ne répond pas"))?;
    ep.close().await;
    result
}
