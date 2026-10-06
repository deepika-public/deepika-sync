//! Regression tests for the CRDT core: offsets, merges, collisions, echoes, compaction.
mod common;
use deepika_sync::workspace::{Workspace, WorkspaceEvent};
use yrs::StateVector;

/// Replicate every document of `from` into `to`, as a first sync would.
async fn replicate(from: &Workspace, to: &Workspace) {
    for doc in from.list_documents().await {
        let full = from
            .encode_diff(&doc.id, &StateVector::default())
            .await
            .unwrap();
        to.apply_remote_update(&doc.id, &full, Some(1))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn edits_use_utf16_offsets_with_accents_and_emoji() {
    let (_d, ws) = common::peer().await;
    let id = ws.create_document("a.md", "été 😀 chaud").await.unwrap();
    // "été 😀 " is 4 + 2 + 1 = 7 UTF-16 units.
    ws.apply_edit(&id, 7, 7, "très ").await.unwrap();
    assert_eq!(ws.get_text(&id).await.unwrap(), "été 😀 très chaud");
    ws.apply_edit(&id, 4, 7, "").await.unwrap();
    assert_eq!(ws.get_text(&id).await.unwrap(), "été très chaud");
    assert!(ws.apply_edit(&id, 0, 999, "").await.is_err());
}

#[tokio::test]
async fn concurrent_disk_imports_merge_without_duplication() {
    let (_d1, a) = common::peer().await;
    let (_d2, b) = common::peer().await;
    let id = a
        .create_document("a.md", "ligne 1\nligne 2\n")
        .await
        .unwrap();
    replicate(&a, &b).await;

    let ua = a.set_text(&id, "LIGNE 1\nligne 2\n").await.unwrap();
    let ub = b.set_text(&id, "ligne 1\nligne 2 bis\n").await.unwrap();
    a.apply_remote_update(&id, &ub, None).await.unwrap();
    b.apply_remote_update(&id, &ua, None).await.unwrap();

    assert_eq!(a.get_text(&id).await.unwrap(), "LIGNE 1\nligne 2 bis\n");
    assert_eq!(b.get_text(&id).await.unwrap(), "LIGNE 1\nligne 2 bis\n");
}

#[tokio::test]
async fn same_path_created_on_two_peers_resolves_identically() {
    let (_d1, a) = common::peer().await;
    let (_d2, b) = common::peer().await;
    let ida = a.create_document("note.md", "from a").await.unwrap();
    let idb = b.create_document("note.md", "from b").await.unwrap();
    replicate(&a, &b).await;
    replicate(&b, &a).await;
    replicate(&a, &b).await;

    let (winner, loser) = if ida < idb {
        (&ida, &idb)
    } else {
        (&idb, &ida)
    };
    let moved = format!("note (conflit {}).md", &loser[..8]);
    for ws in [&a, &b] {
        assert_eq!(
            ws.get_doc_id_by_path("note.md").await.as_ref(),
            Some(winner)
        );
        assert_eq!(ws.get_doc_id_by_path(&moved).await.as_ref(), Some(loser));
    }
}

#[tokio::test]
async fn joining_with_an_identical_copy_keeps_a_single_note() {
    let (_d1, a) = common::peer().await;
    let (_d2, b) = common::peer().await;
    let ida = a.create_document("note.md", "same text").await.unwrap();
    let idb = b.create_document("note.md", "same text").await.unwrap();
    replicate(&a, &b).await;
    replicate(&b, &a).await;
    replicate(&a, &b).await;

    let winner = ida.clone().min(idb.clone());
    for ws in [&a, &b] {
        assert_eq!(ws.get_doc_id_by_path("note.md").await, Some(winner.clone()));
        let live: Vec<_> = ws
            .list_documents()
            .await
            .into_iter()
            .filter(|d| !d.deleted)
            .collect();
        assert_eq!(live.len(), 1, "{live:?}");
    }
}

#[tokio::test]
async fn a_deleted_path_can_be_created_again() {
    let (_d, ws) = common::peer().await;
    let first = ws.create_document("a.md", "one").await.unwrap();
    ws.delete_document(&first).await.unwrap();
    let second = ws.create_document("a.md", "two").await.unwrap();
    assert_eq!(ws.get_doc_id_by_path("a.md").await, Some(second));
}

#[tokio::test]
async fn an_update_already_known_is_not_announced_again() {
    let (_d1, a) = common::peer().await;
    let (_d2, b) = common::peer().await;
    let id = a.create_document("a.md", "hello").await.unwrap();
    replicate(&a, &b).await;

    let mut events = b.events.subscribe();
    replicate(&a, &b).await; // the echo a mesh with a cycle would produce
    assert!(events.try_recv().is_err(), "an echo must die here");

    let update = a.apply_edit(&id, 5, 5, "!").await.unwrap();
    b.apply_remote_update(&id, &update, Some(7)).await.unwrap();
    assert!(matches!(
        events.try_recv(),
        Ok(WorkspaceEvent::DocChanged {
            origin: Some(7),
            ..
        })
    ));
}

#[tokio::test]
async fn releasing_the_last_lease_lets_the_projection_catch_up() {
    let (_d, ws) = common::peer().await;
    let id = ws.create_document("a.md", "x").await.unwrap();
    ws.acquire_lease(&id).await;
    ws.acquire_lease(&id).await;
    let mut events = ws.events.subscribe();
    ws.release_lease(&id).await;
    assert!(ws.is_leased(&id).await && events.try_recv().is_err());
    ws.release_lease(&id).await;
    assert!(!ws.is_leased(&id).await);
    assert!(matches!(
        events.try_recv(),
        Ok(WorkspaceEvent::DocChanged { .. })
    ));
}

#[tokio::test]
async fn long_sessions_are_compacted_and_reload_intact() {
    let (dir, ws) = common::peer().await;
    let id = ws.create_document("a.md", "").await.unwrap();
    for i in 0..450 {
        ws.apply_edit(&id, i, i, "x").await.unwrap();
    }
    let stored = ws.storage.load_all_documents().unwrap();
    assert!(
        stored[0].updates.len() <= 201,
        "{} updates kept",
        stored[0].updates.len()
    );
    drop(ws);

    let reopened = Workspace::open(dir.path()).await.unwrap();
    assert_eq!(reopened.get_text(&id).await.unwrap(), "x".repeat(450));
    assert_eq!(
        reopened.storage.load_all_documents().unwrap()[0]
            .updates
            .len(),
        1
    );
}
