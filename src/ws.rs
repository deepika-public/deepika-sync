//! WebSocket server speaking the standard y-sync / y-awareness binary protocol, one
//! document per URL path. Serves `deepika-sync lsp`, Obsidian (y-codemirror, y-websocket)
//! and any other Yjs client. An open connection is an editor lease on its document.

use crate::workspace::Workspace;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::broadcast::error::RecvError,
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        http::StatusCode,
        protocol::Message as WsMessage,
    },
};
use yrs::{
    sync::{Message, SyncMessage},
    updates::{decoder::Decode, encoder::Encode},
};

/// Browsers attach an `Origin` to every WebSocket, and any web page may target
/// 127.0.0.1. Native clients send none; Obsidian sends its own app scheme.
pub fn origin_allowed(origin: Option<&str>) -> bool {
    origin.is_none_or(|o| o.starts_with("app://") || o.starts_with("file://"))
}

pub async fn serve(workspace: Workspace, bind_addr: SocketAddr) -> Result<()> {
    serve_on(workspace, TcpListener::bind(bind_addr).await?).await
}

/// Serve on a listener the caller bound, so a busy port fails the launch itself.
pub async fn serve_on(workspace: Workspace, listener: TcpListener) -> Result<()> {
    tracing::info!(
        "WebSocket y-sync server listening on {}",
        listener.local_addr()?
    );
    while let Ok((stream, peer_addr)) = listener.accept().await {
        let ws = workspace.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(ws, stream).await {
                tracing::debug!("WebSocket error from {}: {}", peer_addr, e);
            }
        });
    }
    Ok(())
}

/// The document a URL path names: an id, a tracked path, or a note to create.
async fn resolve(workspace: &Workspace, requested: &str) -> Result<String> {
    if requested.is_empty() {
        return match workspace
            .list_documents()
            .await
            .into_iter()
            .find(|d| !d.deleted)
        {
            Some(first) => Ok(first.id),
            None => workspace.create_document("untitled.md", "").await,
        };
    }
    if workspace.locate(requested).await.is_some() {
        return Ok(requested.to_string());
    }
    if let Some(id) = workspace.get_doc_id_by_path(requested).await {
        return Ok(id);
    }
    let initial = workspace.root.read(requested).unwrap_or_default();
    workspace.create_document(requested, &initial).await
}

async fn handle_connection(workspace: Workspace, stream: TcpStream) -> Result<()> {
    let mut requested = String::new();
    #[allow(clippy::result_large_err)]
    let ws_stream = accept_hdr_async(stream, |req: &Request, res: Response| {
        let origin = req.headers().get("origin").and_then(|o| o.to_str().ok());
        if !origin_allowed(origin) {
            let mut refusal = ErrorResponse::new(Some("origin_forbidden".into()));
            *refusal.status_mut() = StatusCode::FORBIDDEN;
            return Err(refusal);
        }
        requested = percent_encoding::percent_decode_str(req.uri().path().trim_start_matches('/'))
            .decode_utf8_lossy()
            .into_owned();
        Ok(res)
    })
    .await?;
    let (mut sender, mut receiver) = ws_stream.split();

    let doc_id = resolve(&workspace, &requested).await?;
    let conn_id: u64 = rand::random();
    // Subscribe before the first state vector so no update falls in between.
    let mut events = workspace.events.subscribe();
    workspace.acquire_lease(&doc_id).await;

    let result: Result<()> = async {
        let sv = workspace.encode_state_vector(&doc_id).await?;
        let step1 = Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1();
        sender.send(WsMessage::Binary(step1.into())).await?;
        if let Some(present) = workspace.presence(&doc_id) {
            sender.send(WsMessage::Binary(present.encode_v1().into())).await?;
        }

        loop {
            tokio::select! {
                msg = receiver.next() => {
                    let Some(Ok(msg)) = msg else { break };
                    if let WsMessage::Binary(bytes) = msg
                        && let Ok(y_msg) = Message::decode_v1(&bytes)
                        && let Some(reply) = workspace.handle_y_msg(&doc_id, y_msg, Some(conn_id)).await?
                    {
                        sender.send(WsMessage::Binary(reply.encode_v1().into())).await?;
                    }
                }
                event = events.recv() => match event {
                    Ok(event) if event.id() == doc_id && event.origin() != Some(conn_id) => {
                        if let Some(payload) = event.wire() {
                            sender.send(WsMessage::Binary(payload.into())).await?;
                        }
                    }
                    Ok(_) => {}
                    // We missed events and cannot tell which: resend the full state.
                    Err(RecvError::Lagged(_)) => {
                        let diff = workspace.encode_diff(&doc_id, &Default::default()).await?;
                        let update = Message::Sync(SyncMessage::Update(diff)).encode_v1();
                        sender.send(WsMessage::Binary(update.into())).await?;
                    }
                    Err(RecvError::Closed) => break,
                },
            }
        }
        Ok(())
    }
    .await;

    workspace.release_lease(&doc_id).await;
    result
}
