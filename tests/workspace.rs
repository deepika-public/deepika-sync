use deepika_sync::workspace::Workspace;
use tempfile::TempDir;

#[tokio::test]
async fn workspace_document_crud_and_edits() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();

    // 1. Create document
    let id = ws.create_document("note.md", "hello world").await.unwrap();
    assert_eq!(ws.get_text(&id).await.unwrap(), "hello world");
    assert_eq!(ws.get_doc_id_by_path("note.md").await, Some(id.clone()));

    // 2. Apply edits
    ws.apply_edit(&id, 5, 5, " brave").await.unwrap();
    assert_eq!(ws.get_text(&id).await.unwrap(), "hello brave world");

    // 3. Rename document
    ws.rename_document(&id, "folder/renamed.md").await.unwrap();
    assert_eq!(ws.get_doc_id_by_path("note.md").await, None);
    assert_eq!(
        ws.get_doc_id_by_path("folder/renamed.md").await,
        Some(id.clone())
    );

    // 4. Leases
    assert!(!ws.is_leased(&id).await);
    ws.acquire_lease(&id).await;
    assert!(ws.is_leased(&id).await);
    ws.release_lease(&id).await;
    assert!(!ws.is_leased(&id).await);

    // 5. Delete document
    ws.delete_document(&id).await.unwrap();
    assert_eq!(ws.get_doc_id_by_path("folder/renamed.md").await, None);
    let docs = ws.list_documents().await;
    let entry = docs.iter().find(|d| d.id == id).unwrap();
    assert!(entry.deleted);
}

#[tokio::test]
async fn workspace_remote_update_adoption() {
    let dir1 = TempDir::new().unwrap();
    let ws1 = Workspace::open(dir1.path()).await.unwrap();

    let id = ws1
        .create_document("note.md", "hello from remote")
        .await
        .unwrap();
    let update = ws1
        .encode_diff(&id, &yrs::StateVector::default())
        .await
        .unwrap();

    let dir2 = TempDir::new().unwrap();
    let ws2 = Workspace::open(dir2.path()).await.unwrap();

    ws2.apply_remote_update(&id, &update, None).await.unwrap();
    assert_eq!(ws2.get_text(&id).await.unwrap(), "hello from remote");
    assert_eq!(ws2.get_doc_id_by_path("note.md").await, Some(id.clone()));

    // Remote rename propagation
    ws1.rename_document(&id, "docs/note.md").await.unwrap();
    let sv2 = ws2.encode_state_vector(&id).await.unwrap();
    let rename_update = ws1.encode_diff(&id, &sv2).await.unwrap();
    ws2.apply_remote_update(&id, &rename_update, None)
        .await
        .unwrap();
    assert_eq!(ws2.get_doc_id_by_path("note.md").await, None);
    assert_eq!(
        ws2.get_doc_id_by_path("docs/note.md").await,
        Some(id.clone())
    );

    // Remote delete propagation
    ws1.delete_document(&id).await.unwrap();
    let sv2 = ws2.encode_state_vector(&id).await.unwrap();
    let delete_update = ws1.encode_diff(&id, &sv2).await.unwrap();
    ws2.apply_remote_update(&id, &delete_update, None)
        .await
        .unwrap();
    assert_eq!(ws2.get_doc_id_by_path("docs/note.md").await, None);
    let docs = ws2.list_documents().await;
    let entry = docs.iter().find(|d| d.id == id).unwrap();
    assert!(entry.deleted);
}
