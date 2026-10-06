//! Language Server Protocol adapter: lets Neovim, VS Code, Helix, Zed, Emacs or any
//! LSP client edit a deepika-sync session without a dedicated plugin.
//!
//! The server is a thin client of the running daemon. Each open buffer holds a local
//! Yrs replica synced over the daemon's WebSocket (`ws.rs`), so it never touches
//! SQLite or the session lock, and the open socket is the editor lease.

#![allow(clippy::mutable_key_type)]

use crate::{
    filesystem,
    workspace::{apply_text_diff, new_doc},
};
use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    ApplyWorkspaceEditParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, InitializeResult, MessageType, Position, Range, ServerCapabilities,
    ServerInfo, ShowMessageParams, TextDocumentContentChangeEvent, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextEdit, Uri, WorkspaceEdit,
};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{net::TcpStream, sync::mpsc};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message as WsMessage,
};
use yrs::{
    Doc, GetString, ReadTxn, Text, Transact, Update,
    sync::{Message as YMessage, SyncMessage},
    updates::{decoder::Decode, encoder::Encode},
};

/// UTF-16 offset of an LSP `{line, character}`, clamping past the end of a short line.
pub fn offset_of(text: &str, line: usize, character: usize) -> usize {
    let mut at = 0;
    for (n, content) in text.split_inclusive('\n').enumerate() {
        if n == line {
            let mut column = 0;
            for c in content.chars() {
                if column >= character || c == '\n' {
                    break;
                }
                column += c.len_utf16();
            }
            return at + column;
        }
        at += content.encode_utf16().count();
    }
    at
}

/// The LSP `{line, character}` naming a UTF-16 offset.
pub fn position_of(text: &str, offset: usize) -> (usize, usize) {
    let mut at = 0;
    let mut line = 0;
    for content in text.split_inclusive('\n') {
        let units = content.encode_utf16().count();
        if offset < at + units || (offset == at + units && !content.ends_with('\n')) {
            return (line, offset - at);
        }
        at += units;
        line += 1;
    }
    (line, 0)
}

/// The smallest LSP edits turning `old_text` into `new_text`, all relative to `old_text`.
pub fn diff_to_text_edits(old_text: &str, new_text: &str) -> Vec<TextEdit> {
    let old: Vec<char> = old_text.chars().collect();
    let new: Vec<char> = new_text.chars().collect();
    // UTF-16 offset of every char index of `old`.
    let mut units = vec![0];
    units.extend(old.iter().scan(0, |at, c| {
        *at += c.len_utf16();
        Some(*at)
    }));
    let position = |index: usize| {
        let (line, character) = position_of(old_text, units[index]);
        Position::new(line as u32, character as u32)
    };
    similar::capture_diff_slices(similar::Algorithm::Myers, &old, &new)
        .iter()
        .map(|op| op.as_tag_tuple())
        .filter(|(tag, ..)| *tag != similar::DiffTag::Equal)
        .map(|(_, old_range, new_range)| TextEdit {
            range: Range::new(position(old_range.start), position(old_range.end)),
            new_text: new[new_range].iter().collect(),
        })
        .collect()
}

fn uri_to_rel_path(root: &Path, uri: &Uri) -> Result<String> {
    let raw = uri
        .as_str()
        .strip_prefix("file://")
        .ok_or_else(|| anyhow!("invalid_file_uri"))?;
    let decoded = percent_decode_str(raw).decode_utf8()?;
    let path = Path::new(decoded.as_ref());
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let rel = canonical
        .strip_prefix(root)
        .or_else(|_| path.strip_prefix(root))
        .map_err(|_| anyhow!("path_outside_root"))?;
    let rel = rel.to_str().ok_or_else(|| anyhow!("invalid_utf8_path"))?;
    filesystem::validate(rel)?;
    Ok(rel.to_owned())
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// One open editor buffer: a local replica, its socket, and the editor's view of the text.
struct HeldDoc {
    doc: Doc,
    sink: SplitSink<Socket, WsMessage>,
    current_text: String,
    reader: tokio::task::JoinHandle<()>,
}

impl HeldDoc {
    fn text(&self) -> String {
        self.doc
            .get_or_insert_text("content")
            .get_string(&self.doc.transact())
    }

    async fn send(&mut self, msg: YMessage) -> Result<()> {
        Ok(self
            .sink
            .send(WsMessage::Binary(msg.encode_v1().into()))
            .await?)
    }

    /// Apply one editor change to the replica and push it to the daemon.
    async fn edit(&mut self, change: TextDocumentContentChangeEvent) -> Result<()> {
        let update = {
            let text = self.doc.get_or_insert_text("content");
            let mut txn = self.doc.transact_mut();
            match change.range {
                Some(range) => {
                    let at = |p: Position| {
                        offset_of(&self.current_text, p.line as usize, p.character as usize)
                    };
                    let (from, to) = (at(range.start), at(range.end));
                    if to > from {
                        text.remove_range(&mut txn, from as u32, (to - from) as u32);
                    }
                    text.insert(&mut txn, from as u32, &change.text);
                }
                None => apply_text_diff(&text, &mut txn, &change.text),
            }
            txn.encode_update_v1()
        };
        self.current_text = self.text();
        self.send(YMessage::Sync(SyncMessage::Update(update))).await
    }

    /// Handle one y-sync message from the daemon; returns true when the text moved.
    async fn receive(&mut self, bytes: &[u8]) -> Result<bool> {
        match YMessage::decode_v1(bytes)? {
            YMessage::Sync(SyncMessage::SyncStep1(sv)) => {
                let diff = self.doc.transact().encode_diff_v1(&sv);
                self.send(YMessage::Sync(SyncMessage::SyncStep2(diff)))
                    .await?;
                Ok(false)
            }
            YMessage::Sync(SyncMessage::SyncStep2(update) | SyncMessage::Update(update)) => {
                self.doc
                    .transact_mut()
                    .apply_update(Update::decode_v1(&update)?)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// What the per-document socket readers feed into the main loop; `None` = socket closed.
type Remote = (Uri, Option<Vec<u8>>);

struct Server {
    connection: Connection,
    root: PathBuf,
    ws_url: String,
    held: BTreeMap<Uri, HeldDoc>,
    remote_tx: mpsc::Sender<Remote>,
    next_request_id: i32,
}

impl Server {
    fn notify_user(&self, typ: MessageType, message: String) -> Result<()> {
        let params = ShowMessageParams { typ, message };
        Ok(self
            .connection
            .sender
            .send(Message::Notification(Notification::new(
                "window/showMessage".into(),
                params,
            )))?)
    }

    /// Connect to the daemon and wait until the replica holds the document.
    async fn open(&mut self, params: DidOpenTextDocumentParams) -> Result<()> {
        let uri = params.text_document.uri;
        let rel = uri_to_rel_path(&self.root, &uri)?;
        let encoded: Vec<String> = rel
            .split('/')
            .map(|part| utf8_percent_encode(part, NON_ALPHANUMERIC).to_string())
            .collect();
        let url = format!("{}/{}", self.ws_url, encoded.join("/"));
        let (socket, _) = connect_async(&url).await.with_context(|| {
            format!(
                "daemon deepika-sync injoignable sur {} : lancez `deepika-sync share` ou `join`",
                self.ws_url
            )
        })?;
        let (sink, mut stream) = socket.split();
        let mut held = HeldDoc {
            doc: new_doc(),
            sink,
            current_text: params.text_document.text,
            reader: tokio::spawn(async {}),
        };

        held.send(YMessage::Sync(SyncMessage::SyncStep1(Default::default())))
            .await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(msg) = stream.next().await {
                let WsMessage::Binary(bytes) = msg? else {
                    continue;
                };
                let step2 = matches!(
                    YMessage::decode_v1(&bytes),
                    Ok(YMessage::Sync(SyncMessage::SyncStep2(_)))
                );
                held.receive(&bytes).await?;
                if step2 {
                    return Ok(());
                }
            }
            bail!("daemon_closed_during_sync")
        })
        .await
        .context("daemon_sync_timeout")??;

        // A buffer the daemon has never seen and found nothing on disk for: the editor's text wins.
        if held.text().is_empty() && !held.current_text.is_empty() {
            let text = std::mem::take(&mut held.current_text);
            held.edit(TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text,
            })
            .await?;
        }

        let (tx, reader_uri) = (self.remote_tx.clone(), uri.clone());
        held.reader = tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                if let WsMessage::Binary(bytes) = msg
                    && tx
                        .send((reader_uri.clone(), Some(bytes.to_vec())))
                        .await
                        .is_err()
                {
                    return;
                }
            }
            let _ = tx.send((reader_uri, None)).await;
        });
        self.held.insert(uri.clone(), held);
        self.push_to_editor(&uri)
    }

    /// Bring the editor's buffer in line with the replica.
    fn push_to_editor(&mut self, uri: &Uri) -> Result<()> {
        let Some(held) = self.held.get_mut(uri) else {
            return Ok(());
        };
        let new_text = held.text();
        let edits = diff_to_text_edits(&held.current_text, &new_text);
        if edits.is_empty() {
            return Ok(());
        }
        held.current_text = new_text;
        let params = ApplyWorkspaceEditParams {
            label: Some("deepika_sync_remote_edit".into()),
            edit: WorkspaceEdit {
                changes: Some(HashMap::from([(uri.clone(), edits)])),
                ..Default::default()
            },
        };
        let id = RequestId::from(self.next_request_id);
        self.next_request_id += 1;
        Ok(self.connection.sender.send(Message::Request(Request::new(
            id,
            "workspace/applyEdit".into(),
            params,
        )))?)
    }

    fn close(&mut self, uri: &Uri) {
        // Dropping the sink closes the socket, which releases the lease in the daemon.
        if let Some(held) = self.held.remove(uri) {
            held.reader.abort();
        }
    }

    async fn on_notification(&mut self, notif: Notification) -> Result<()> {
        match notif.method.as_str() {
            "textDocument/didOpen" => {
                if let Err(e) = self.open(serde_json::from_value(notif.params)?).await {
                    self.notify_user(MessageType::ERROR, format!("deepika-sync : {e:#}"))?;
                }
            }
            "textDocument/didChange" => {
                let params: DidChangeTextDocumentParams = serde_json::from_value(notif.params)?;
                if let Some(held) = self.held.get_mut(&params.text_document.uri) {
                    for change in params.content_changes {
                        held.edit(change).await?;
                    }
                }
            }
            "textDocument/didClose" => {
                let params: DidCloseTextDocumentParams = serde_json::from_value(notif.params)?;
                self.close(&params.text_document.uri);
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_remote(&mut self, (uri, bytes): Remote) -> Result<()> {
        let Some(bytes) = bytes else {
            self.close(&uri);
            return self.notify_user(
                MessageType::WARNING,
                "deepika-sync : connexion au daemon perdue, ce tampon n'est plus synchronisé"
                    .into(),
            );
        };
        if let Some(held) = self.held.get_mut(&uri)
            && held.receive(&bytes).await?
        {
            self.push_to_editor(&uri)?;
        }
        Ok(())
    }
}

pub async fn run_stdio(root: &Path, ws_port: u16) -> Result<()> {
    let (connection, io_threads) = Connection::stdio();
    run_connection(connection, root, &format!("ws://127.0.0.1:{ws_port}")).await?;
    io_threads.join()?;
    Ok(())
}

pub async fn run_connection(connection: Connection, root: &Path, ws_url: &str) -> Result<()> {
    match connection.receiver.recv()? {
        Message::Request(req) if req.method == "initialize" => {
            let result = InitializeResult {
                capabilities: ServerCapabilities {
                    text_document_sync: Some(TextDocumentSyncCapability::Kind(
                        TextDocumentSyncKind::INCREMENTAL,
                    )),
                    ..Default::default()
                },
                server_info: Some(ServerInfo {
                    name: "deepika-sync-lsp".into(),
                    version: Some(env!("CARGO_PKG_VERSION").into()),
                }),
            };
            connection
                .sender
                .send(Message::Response(Response::new_ok(req.id, result)))?;
        }
        _ => bail!("expected_initialize"),
    }
    match connection.receiver.recv()? {
        Message::Notification(notif) if notif.method == "initialized" => {}
        _ => bail!("expected_initialized_notification"),
    }

    // lsp-server reads stdio on a blocking channel; bridge it into the async loop.
    let (msg_tx, mut msg_rx) = mpsc::channel(256);
    let receiver = connection.receiver.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(msg) = receiver.recv() {
            if msg_tx.blocking_send(msg).is_err() {
                break;
            }
        }
    });

    let (remote_tx, mut remote_rx) = mpsc::channel(256);
    let mut server = Server {
        connection,
        root: root.canonicalize()?,
        ws_url: ws_url.trim_end_matches('/').to_string(),
        held: BTreeMap::new(),
        remote_tx,
        next_request_id: 1,
    };

    loop {
        tokio::select! {
            msg = msg_rx.recv() => match msg {
                Some(Message::Request(req)) => {
                    let resp = if req.method == "shutdown" {
                        Response::new_ok(req.id, serde_json::Value::Null)
                    } else {
                        Response::new_err(
                            req.id,
                            lsp_server::ErrorCode::MethodNotFound as i32,
                            "method_not_found".into(),
                        )
                    };
                    server.connection.sender.send(Message::Response(resp))?;
                }
                Some(Message::Notification(notif)) if notif.method == "exit" => break,
                Some(Message::Notification(notif)) => server.on_notification(notif).await?,
                Some(Message::Response(_)) => {}
                None => break,
            },
            Some(remote) = remote_rx.recv() => server.on_remote(remote).await?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_insert_inside_a_line() {
        let edits = diff_to_text_edits("hello world", "hello brave new world");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "brave new ");
        assert_eq!(
            edits[0].range,
            Range::new(Position::new(0, 6), Position::new(0, 6))
        );
    }

    #[test]
    fn positions_count_utf16_units() {
        // 'é' is one unit, '😀' is two.
        let text = "é😀x\nfin";
        assert_eq!(offset_of(text, 0, 3), 3);
        assert_eq!(position_of(text, 3), (0, 3));
        assert_eq!(offset_of(text, 1, 99), 8);
        assert_eq!(position_of(text, 5), (1, 0));
        let edits = diff_to_text_edits(text, "é😀y\nfin");
        assert_eq!(
            edits[0].range,
            Range::new(Position::new(0, 3), Position::new(0, 4))
        );
    }
}
