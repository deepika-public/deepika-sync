use deepika_sync::workspace::Workspace;
use tempfile::TempDir;

#[tokio::test]
async fn sqlite_storage_lifecycle_and_reload() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();
    let doc_id = ws.create_document("note.md", "hello world").await.unwrap();

    // Edit document
    ws.apply_edit(&doc_id, 5, 5, " brave").await.unwrap();
    assert_eq!(ws.get_text(&doc_id).await.unwrap(), "hello brave world");

    // Close and reopen workspace from disk/SQLite
    drop(ws);
    let ws2 = Workspace::open(dir.path()).await.unwrap();
    assert_eq!(ws2.get_text(&doc_id).await.unwrap(), "hello brave world");
    assert_eq!(ws2.list_documents().await.len(), 1);
}

#[tokio::test]
async fn index_follows_renames_and_is_readable_while_the_session_is_locked() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();
    let id = ws.create_document("a.md", "content a").await.unwrap();
    ws.rename_document(&id, "folder/a_renamed.md")
        .await
        .unwrap();

    // A second daemon is refused, with a message a human can act on...
    let refused = Workspace::open(dir.path()).await.err().unwrap().to_string();
    assert!(refused.starts_with("session_already_running"), "{refused}");
    // ...but `status` and `documents` still read the index.
    let (_capability, rows) = deepika_sync::storage::Storage::peek(dir.path()).unwrap();
    assert_eq!(rows, vec![(id, "folder/a_renamed.md".to_string(), false)]);
}

#[tokio::test]
async fn legacy_unique_path_schema_is_migrated() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".collab")).unwrap();
    let db = rusqlite::Connection::open(dir.path().join(".collab/state.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE documents(id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, deleted INTEGER NOT NULL DEFAULT 0);
         INSERT INTO documents VALUES('old-id', 'kept.md', 1);",
    )
    .unwrap();
    drop(db);

    let ws = Workspace::open(dir.path()).await.unwrap();
    // The tombstoned row survived, and its path is free again.
    assert!(ws.list_documents().await.iter().any(|d| d.id == "old-id"));
    ws.create_document("kept.md", "again").await.unwrap();
}

#[tokio::test]
async fn sqlite_internal_symlinks_are_rejected() {
    for name in [
        "state.sqlite",
        "state.sqlite-wal",
        "state.sqlite-shm",
        "state.sqlite-journal",
        "lock",
    ] {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let secret = outside.path().join("secret");
        std::fs::write(&secret, "unchanged").unwrap();
        std::fs::create_dir(dir.path().join(".collab")).unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join(".collab").join(name)).unwrap();
        assert!(Workspace::open(dir.path()).await.is_err(), "{name}");
        assert_eq!(std::fs::read_to_string(secret).unwrap(), "unchanged");
    }
}
