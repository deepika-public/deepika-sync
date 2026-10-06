//! Joining is preceded by a plan of what it will change, and by nothing else.
use deepika_sync::{
    preview::{JoinPlan, Local, ManifestEntry, Rename, describe, plan},
    transport::{self, Invite},
    workspace::Workspace,
};
use std::collections::HashMap;

fn hash(text: &str) -> [u8; 32] {
    *blake3::hash(text.as_bytes()).as_bytes()
}
fn remote(id: &str, path: &str, text: &str) -> ManifestEntry {
    ManifestEntry {
        id: id.into(),
        path: path.into(),
        deleted: false,
        hash: hash(text),
    }
}
fn files(entries: &[(&str, &str)]) -> HashMap<String, [u8; 32]> {
    entries
        .iter()
        .map(|(path, text)| (path.to_string(), hash(text)))
        .collect()
}

#[test]
fn a_fresh_folder_sees_creations_conflicts_twins_and_what_it_shares() {
    let manifest = [
        remote("1", "new.md", "from the session"),
        remote("2", "same.md", "identical"),
        remote("3", "notes/clash.md", "theirs"),
        ManifestEntry {
            deleted: true,
            ..remote("4", "gone.md", "")
        },
    ];
    let local = Local {
        files: files(&[
            ("same.md", "identical"),
            ("notes/clash.md", "mine"),
            ("mine only.md", "x"),
            ("gone.md", "kept"),
        ]),
        known: HashMap::new(),
    };
    let result = plan(&manifest, &local);
    assert_eq!(
        result,
        JoinPlan {
            session_documents: 3,
            create: vec!["new.md".into()],
            conflict: vec!["notes/clash.md".into()],
            identical: 1,
            // A note deleted in the session says nothing about an unrelated local file.
            share: vec!["gone.md".into(), "mine only.md".into()],
            ..Default::default()
        }
    );
    assert!(!result.leaves_folder_untouched());
    let text = describe(&result);
    assert!(text.contains("new.md") && text.contains("conflit") && text.contains("mine only.md"));
}

#[test]
fn a_returning_folder_sees_merges_renames_and_deletions_by_identity() {
    let manifest = [
        remote("1", "renamed.md", "unchanged"),
        remote("2", "edited.md", "new text"),
        ManifestEntry {
            deleted: true,
            ..remote("3", "deleted.md", "")
        },
        remote("4", "quiet.md", "same"),
    ];
    let local = Local {
        files: files(&[
            ("old name.md", "unchanged"),
            ("edited.md", "old text"),
            ("deleted.md", "bye"),
            ("quiet.md", "same"),
        ]),
        known: [
            ("1", "old name.md"),
            ("2", "edited.md"),
            ("3", "deleted.md"),
            ("4", "quiet.md"),
        ]
        .into_iter()
        .map(|(id, path)| (id.to_string(), (path.to_string(), false)))
        .collect(),
    };
    assert_eq!(
        plan(&manifest, &local),
        JoinPlan {
            session_documents: 3,
            update: vec!["edited.md".into()],
            rename: vec![Rename {
                from: "old name.md".into(),
                to: "renamed.md".into()
            }],
            delete: vec!["deleted.md".into()],
            identical: 2,
            ..Default::default()
        }
    );
}

#[test]
fn joining_an_identical_folder_changes_nothing() {
    let result = plan(
        &[remote("1", "a.md", "x")],
        &Local {
            files: files(&[("a.md", "x")]),
            known: HashMap::new(),
        },
    );
    assert!(result.leaves_folder_untouched());
    assert!(describe(&result).contains("Aucun fichier de ce dossier ne sera modifié"));
}

#[tokio::test]
async fn the_manifest_is_fetched_without_touching_the_joining_folder() {
    let host_dir = tempfile::TempDir::new().unwrap();
    let host = Workspace::open(host_dir.path()).await.unwrap();
    host.set_capability("the-secret").await.unwrap();
    host.create_document("shared.md", "hello").await.unwrap();
    let ep = transport::endpoint(&host, false).await.unwrap();
    let router = transport::run(host.clone(), ep.clone(), None)
        .await
        .unwrap();
    let invite = Invite {
        version: 1,
        address: ep.addr(),
        capability: "the-secret".into(),
    };

    let joiner = tempfile::TempDir::new().unwrap();
    std::fs::write(joiner.path().join("shared.md"), "different").unwrap();
    let manifest = transport::fetch_manifest(&invite, false).await.unwrap();
    let result = plan(&manifest, &Local::scan(joiner.path()).unwrap());
    assert_eq!(result.conflict, vec!["shared.md".to_string()]);
    // Looking is free: no state directory, no log, the note as it was.
    let left: Vec<_> = std::fs::read_dir(joiner.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, vec![std::ffi::OsString::from("shared.md")]);
    assert_eq!(
        std::fs::read_to_string(joiner.path().join("shared.md")).unwrap(),
        "different"
    );

    // Without the capability there is no manifest to look at.
    let stranger = Invite {
        capability: "a guess".into(),
        ..invite
    };
    assert!(transport::fetch_manifest(&stranger, false).await.is_err());
    router.shutdown().await.unwrap();
}
