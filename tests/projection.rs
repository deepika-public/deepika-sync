use deepika_sync::{projection::Projector, workspace::Workspace};
use notify::event::{EventKind, ModifyKind, RenameMode};
use tempfile::TempDir;

#[tokio::test]
async fn projection_disk_events_and_materialization() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();
    let projector = Projector::new(ws.clone());

    // 1. Write file directly to disk
    let file_path = dir.path().join("test.md");
    std::fs::write(&file_path, "from disk").unwrap();

    let create_event = notify::Event {
        kind: EventKind::Create(notify::event::CreateKind::File),
        paths: vec![file_path.clone()],
        attrs: Default::default(),
    };
    projector.handle_fs_event(&create_event).await.unwrap();

    // Verify document was automatically created in Workspace
    let doc_id = ws
        .get_doc_id_by_path("test.md")
        .await
        .expect("imported doc");
    assert_eq!(ws.get_text(&doc_id).await.unwrap(), "from disk");

    // 2. Rename on disk
    let new_path = dir.path().join("renamed.md");
    std::fs::rename(&file_path, &new_path).unwrap();

    let rename_event = notify::Event {
        kind: EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
        paths: vec![file_path.clone(), new_path.clone()],
        attrs: Default::default(),
    };
    projector.handle_fs_event(&rename_event).await.unwrap();

    assert_eq!(ws.get_doc_id_by_path("test.md").await, None);
    assert_eq!(
        ws.get_doc_id_by_path("renamed.md").await,
        Some(doc_id.clone())
    );

    // 3. Materialize remote edit to disk
    ws.apply_edit(&doc_id, 9, 9, " and more").await.unwrap();
    projector.project_to_disk(&doc_id).await.unwrap();

    let text_on_disk = std::fs::read_to_string(&new_path).unwrap();
    assert_eq!(text_on_disk, "from disk and more");
}

#[tokio::test]
async fn moving_a_folder_moves_every_note_in_it_and_keeps_their_identity() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("group/deep")).unwrap();
    std::fs::write(dir.path().join("group/a.md"), "a").unwrap();
    std::fs::write(dir.path().join("group/deep/b.md"), "b").unwrap();
    std::fs::write(dir.path().join("groupie.md"), "not inside").unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();
    let projector = Projector::new(ws.clone());
    let a = ws.get_doc_id_by_path("group/a.md").await.unwrap();
    let b = ws.get_doc_id_by_path("group/deep/b.md").await.unwrap();

    // One event for the folder, none for what it holds -- as inotify reports it.
    std::fs::rename(dir.path().join("group"), dir.path().join("moved")).unwrap();
    std::fs::write(dir.path().join("moved/stowaway.md"), "untracked").unwrap();
    projector
        .handle_fs_event(&notify::Event {
            kind: EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            paths: vec![dir.path().join("group"), dir.path().join("moved")],
            attrs: Default::default(),
        })
        .await
        .unwrap();

    assert_eq!(ws.get_doc_id_by_path("moved/a.md").await, Some(a));
    assert_eq!(ws.get_doc_id_by_path("moved/deep/b.md").await, Some(b));
    assert!(ws.get_doc_id_by_path("moved/stowaway.md").await.is_some());
    assert!(ws.get_doc_id_by_path("groupie.md").await.is_some());
    assert_eq!(ws.get_doc_id_by_path("group/a.md").await, None);

    // Removing the folder removes what it held.
    std::fs::remove_dir_all(dir.path().join("moved")).unwrap();
    projector
        .handle_fs_event(&notify::Event {
            kind: EventKind::Remove(notify::event::RemoveKind::Folder),
            paths: vec![dir.path().join("moved")],
            attrs: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(ws.get_doc_id_by_path("moved/a.md").await, None);
    assert_eq!(
        ws.list_documents()
            .await
            .iter()
            .filter(|d| !d.deleted)
            .count(),
        1
    );
}

#[tokio::test]
async fn an_unpaired_rename_keeps_the_note_identity() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("note.md"), "same text").unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();
    let projector = Projector::new(ws.clone());
    let id = ws.get_doc_id_by_path("note.md").await.unwrap();

    // The watcher reports the new name as a plain creation.
    std::fs::rename(dir.path().join("note.md"), dir.path().join("renamed.md")).unwrap();
    projector
        .handle_fs_event(&notify::Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![dir.path().join("renamed.md")],
            attrs: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(ws.get_doc_id_by_path("renamed.md").await, Some(id));
    assert_eq!(ws.list_documents().await.len(), 1);

    // A copy is not a rename: the original is still there.
    std::fs::copy(dir.path().join("renamed.md"), dir.path().join("copy.md")).unwrap();
    projector
        .handle_fs_event(&notify::Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![dir.path().join("copy.md")],
            attrs: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(ws.list_documents().await.len(), 2);
}

#[tokio::test]
async fn a_late_event_showing_our_previous_write_does_not_revert_the_note() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::open(dir.path()).await.unwrap();
    let projector = Projector::new(ws.clone());
    let id = ws.create_document("note.md", "first").await.unwrap();
    let file = dir.path().join("note.md");
    let modified = notify::Event {
        kind: EventKind::Modify(ModifyKind::Any),
        paths: vec![file.clone()],
        attrs: Default::default(),
    };

    // Two writes in a row, as when two updates arrive from a peer back to back.
    projector.project_to_disk(&id).await.unwrap();
    ws.apply_edit(&id, 5, 5, " then second").await.unwrap();
    projector.project_to_disk(&id).await.unwrap();

    // The watcher is late: what it reports is still the first text.
    std::fs::write(&file, "first").unwrap();
    projector.handle_fs_event(&modified).await.unwrap();
    assert_eq!(ws.get_text(&id).await.unwrap(), "first then second");

    // A text we never wrote is a real outside edit, and is imported.
    std::fs::write(&file, "first then second, edited outside").unwrap();
    projector.handle_fs_event(&modified).await.unwrap();
    assert_eq!(
        ws.get_text(&id).await.unwrap(),
        "first then second, edited outside"
    );
}

/// One life of the daemon over a folder: open, catch up, let `work` happen, stop.
async fn session<T>(
    root: &std::path::Path,
    work: impl AsyncFnOnce(&Workspace, &Projector) -> T,
) -> T {
    let ws = Workspace::open(root).await.unwrap();
    let projector = Projector::new(ws.clone());
    projector.reconcile().await.unwrap();
    work(&ws, &projector).await
}

async fn live(ws: &Workspace) -> Vec<(String, String)> {
    let mut notes = Vec::new();
    for doc in ws.list_documents().await {
        if !doc.deleted {
            notes.push((doc.path.clone(), ws.get_text(&doc.id).await.unwrap()));
        }
    }
    notes.sort();
    notes
}

#[tokio::test]
async fn what_happened_to_the_folder_while_stopped_is_caught_up_at_start() {
    let dir = TempDir::new().unwrap();
    let at = |name: &str| dir.path().join(name);
    for name in ["kept.md", "edited.md", "deleted.md", "moved.md"] {
        std::fs::write(at(name), format!("text of {name}\n")).unwrap();
    }
    let moved_id = session(dir.path(), async |ws, _| {
        ws.get_doc_id_by_path("moved.md").await.unwrap()
    })
    .await;

    // Nobody is watching.
    std::fs::write(at("edited.md"), "edited while stopped\n").unwrap();
    std::fs::remove_file(at("deleted.md")).unwrap();
    std::fs::create_dir(at("folder")).unwrap();
    std::fs::rename(at("moved.md"), at("folder/moved.md")).unwrap();
    std::fs::write(at("created.md"), "created while stopped\n").unwrap();

    session(dir.path(), async |ws, _| {
        let text = |name: &str| (name.to_string(), format!("text of {name}\n"));
        assert_eq!(
            live(ws).await,
            [
                (
                    "created.md".to_string(),
                    "created while stopped\n".to_string()
                ),
                (
                    "edited.md".to_string(),
                    "edited while stopped\n".to_string()
                ),
                (
                    "folder/moved.md".to_string(),
                    "text of moved.md\n".to_string()
                ),
                text("kept.md"),
            ]
        );
        // A move keeps the note's identity; a deletion is a tombstone for the peers.
        assert_eq!(
            ws.get_doc_id_by_path("folder/moved.md").await,
            Some(moved_id)
        );
        assert!(
            ws.list_documents()
                .await
                .iter()
                .any(|d| d.path == "deleted.md" && d.deleted)
        );
    })
    .await;
}

#[tokio::test]
async fn a_note_that_never_reached_the_disk_is_written_not_deleted() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("local.md"), "here\n").unwrap();
    // A peer's note and a peer's edit are stored, and the daemon stops before projecting.
    session(dir.path(), async |ws, _| {
        ws.create_document("from a peer.md", "never written\n")
            .await
            .unwrap();
        let id = ws.get_doc_id_by_path("local.md").await.unwrap();
        ws.set_text(&id, "here, and edited by a peer\n")
            .await
            .unwrap();
    })
    .await;
    assert!(!dir.path().join("from a peer.md").exists());

    session(dir.path(), async |ws, _| {
        assert_eq!(live(ws).await.len(), 2, "nothing was taken for a deletion");
    })
    .await;
    let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap();
    assert_eq!(read("from a peer.md"), "never written\n");
    assert_eq!(
        read("local.md"),
        "here, and edited by a peer\n",
        "the untouched file follows the note"
    );
}

#[tokio::test]
async fn a_store_from_before_disk_tracking_takes_a_missing_file_for_a_deletion() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("old.md"), "x\n").unwrap();
    // As 0.5.1 left it: documents, and no record of what was seen on disk.
    drop(Workspace::open(dir.path()).await.unwrap());
    std::fs::remove_file(dir.path().join("old.md")).unwrap();

    session(dir.path(), async |ws, _| assert!(live(ws).await.is_empty())).await;
    assert!(!dir.path().join("old.md").exists());
}

/// macOS reaches temporary folders through /var, a symlink to /private/var; a vault can sit
/// behind a symlink anywhere. The watcher then reports paths the canonical root does not prefix.
#[cfg(unix)]
#[tokio::test]
async fn events_reported_through_a_symlink_of_the_root_are_seen() {
    let real = TempDir::new().unwrap();
    let links = TempDir::new().unwrap();
    let link = links.path().join("vault");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();
    let ws = Workspace::open(&link).await.unwrap();
    let projector = Projector::new(ws.clone());
    std::fs::write(link.join("note.md"), "through the link").unwrap();
    projector
        .handle_fs_event(&notify::Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![link.join("note.md")],
            attrs: Default::default(),
        })
        .await
        .unwrap();
    let id = ws
        .get_doc_id_by_path("note.md")
        .await
        .expect("imported doc");
    assert_eq!(ws.get_text(&id).await.unwrap(), "through the link");
}
