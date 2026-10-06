mod common;
use common::peer as setup;
use deepika_sync::lsp::run_connection;
use lsp_server::{Connection, Message, Request, RequestId};
use lsp_types::{
    ApplyWorkspaceEditParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, Position, Range, TextDocumentContentChangeEvent, TextDocumentItem,
    Uri, VersionedTextDocumentIdentifier,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::mutable_key_type)]
async fn lsp_handshake_incremental_editing_and_remote_push() {
    let (_dir, ws) = setup().await;
    let root_path = ws.root.path.clone();
    let note_id = ws
        .create_document("hello.md", "héllo 😀 world")
        .await
        .unwrap();

    // The LSP server is a client of the daemon's WebSocket.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 49878));
    let daemon = tokio::spawn(deepika_sync::ws::serve(ws.clone(), addr));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let (server_conn, client_conn) = Connection::memory();
    let server_root = root_path.clone();
    let server_task = tokio::spawn(async move {
        run_connection(server_conn, &server_root, "ws://127.0.0.1:49878")
            .await
            .unwrap();
    });

    let timeout = std::time::Duration::from_secs(2);

    // 1. Send initialize request
    let init_req = Request {
        id: RequestId::from(1),
        method: "initialize".into(),
        params: serde_json::json!({
            "capabilities": {},
            "rootUri": format!("file://{}", root_path.display()),
        }),
    };
    client_conn.sender.send(Message::Request(init_req)).unwrap();

    // Verify initialize response
    let init_resp = match client_conn
        .receiver
        .recv_timeout(timeout)
        .expect("recv initialize")
    {
        Message::Response(resp) => resp,
        other => panic!("expected response, got {:?}", other),
    };
    assert_eq!(init_resp.id, RequestId::from(1));
    let result = init_resp.response_result.unwrap();
    assert_eq!(
        result["capabilities"]["textDocumentSync"],
        serde_json::json!(2) // INCREMENTAL
    );

    // 2. Send initialized notification
    client_conn
        .sender
        .send(Message::Notification(lsp_server::Notification {
            method: "initialized".into(),
            params: serde_json::json!({}),
        }))
        .unwrap();

    // 3. Send textDocument/didOpen
    let file_uri: Uri = format!("file://{}/hello.md", root_path.display())
        .parse()
        .unwrap();
    client_conn
        .sender
        .send(Message::Notification(lsp_server::Notification {
            method: "textDocument/didOpen".into(),
            params: serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: file_uri.clone(),
                    language_id: "markdown".into(),
                    version: 1,
                    text: "héllo 😀 world".into(),
                },
            })
            .unwrap(),
        }))
        .unwrap();

    // Check that buffer is now leased
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(ws.is_leased(&note_id).await);

    // 4. Send textDocument/didChange (insert " brave")
    client_conn
        .sender
        .send(Message::Notification(lsp_server::Notification {
            method: "textDocument/didChange".into(),
            params: serde_json::to_value(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: file_uri.clone(),
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: Some(Range {
                        start: Position {
                            line: 0,
                            character: 5,
                        },
                        end: Position {
                            line: 0,
                            character: 5,
                        },
                    }),
                    range_length: None,
                    text: " brave".into(),
                }],
            })
            .unwrap(),
        }))
        .unwrap();

    // Verify change applied to workspace
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(ws.get_text(&note_id).await.unwrap(), "héllo brave 😀 world");

    // 5. Simulate remote edit on the workspace and verify server sends workspace/applyEdit
    // Offsets are UTF-16 units: the emoji counts for two.
    ws.apply_edit(&note_id, 20, 20, " rockstars").await.unwrap();

    // Client should receive workspace/applyEdit request
    let push_msg = match client_conn
        .receiver
        .recv_timeout(timeout)
        .expect("recv workspace/applyEdit")
    {
        Message::Request(req) => req,
        other => panic!("expected workspace/applyEdit request, got {:?}", other),
    };
    assert_eq!(push_msg.method, "workspace/applyEdit");
    let apply_params: ApplyWorkspaceEditParams = serde_json::from_value(push_msg.params).unwrap();
    let changes = apply_params.edit.changes.unwrap();
    assert!(changes.contains_key(&file_uri));
    let edits = &changes[&file_uri];
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, " rockstars");
    assert_eq!(edits[0].range.start, Position::new(0, 20));

    // 6. Send didClose
    client_conn
        .sender
        .send(Message::Notification(lsp_server::Notification {
            method: "textDocument/didClose".into(),
            params: serde_json::to_value(DidCloseTextDocumentParams {
                text_document: lsp_types::TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
            })
            .unwrap(),
        }))
        .unwrap();

    // Check lease is released
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!ws.is_leased(&note_id).await);

    // 7. Shutdown
    client_conn
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(2),
            method: "shutdown".into(),
            params: serde_json::Value::Null,
        }))
        .unwrap();

    let shutdown_resp = match client_conn
        .receiver
        .recv_timeout(timeout)
        .expect("recv shutdown")
    {
        Message::Response(resp) => resp,
        other => panic!("expected response, got {:?}", other),
    };
    assert_eq!(shutdown_resp.id, RequestId::from(2));

    client_conn
        .sender
        .send(Message::Notification(lsp_server::Notification {
            method: "exit".into(),
            params: serde_json::Value::Null,
        }))
        .unwrap();

    server_task.await.unwrap();
    daemon.abort();
}
