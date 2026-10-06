//! Reactive projection between local Markdown files on disk and Yrs CRDT documents.
//! Replaces legacy ownership.rs and renames.rs with direct ACID and lease-aware projection.
use crate::workspace::{Workspace, WorkspaceEvent};
use anyhow::{Result, anyhow};
use notify::event::{EventKind, ModifyKind, RenameMode};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{RwLock, broadcast::error::RecvError};

/// What we recently wrote to each path, to tell our own writes from a user's.
///
/// The latest write alone is not enough. The watcher reports a write some hundred
/// milliseconds late; if the file was written twice meanwhile, the late event may still
/// read the first text. Remembering only the second, we took that first text for an
/// outside edit, imported it, and so reverted the newer text on every peer.
#[derive(Default)]
struct Echoes(std::sync::Mutex<HashMap<String, VecDeque<(blake3::Hash, Instant)>>>);

impl Echoes {
    /// Long enough to outlast the watcher's delay, short enough that a user who puts an
    /// earlier text back by hand is still heard.
    const WINDOW: Duration = Duration::from_secs(5);
    const KEPT: usize = 16;

    fn record(&self, path: &str, hash: blake3::Hash) {
        let mut all = self.0.lock().unwrap();
        let recent = all.entry(path.to_string()).or_default();
        recent.push_back((hash, Instant::now()));
        while recent.len() > Self::KEPT {
            recent.pop_front();
        }
    }

    fn is_echo(&self, path: &str, hash: blake3::Hash) -> bool {
        let all = self.0.lock().unwrap();
        all.get(path).is_some_and(|recent| {
            // The latest text is ours for as long as it stays; older ones only briefly.
            recent.back().is_some_and(|(h, _)| *h == hash)
                || recent
                    .iter()
                    .any(|(h, at)| *h == hash && at.elapsed() < Self::WINDOW)
        })
    }

    fn forget(&self, path: &str) {
        self.0.lock().unwrap().remove(path);
    }
}

pub struct Projector {
    workspace: Workspace,
    written: Echoes,
    /// Documents renamed by a peer while an editor held them: id -> path still on disk.
    deferred_moves: RwLock<HashMap<String, String>>,
}

impl Projector {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            written: Echoes::default(),
            deferred_moves: RwLock::new(HashMap::new()),
        }
    }

    /// Process a notify filesystem event from the disk watcher.
    pub async fn handle_fs_event(&self, event: &notify::Event) -> Result<()> {
        let root_path = &self.workspace.root.path;
        let relative = |p: &Path| -> Option<String> {
            let inside = p
                .strip_prefix(root_path)
                .map(Path::to_path_buf)
                .or_else(|_| {
                    // The root is canonical; a path can reach it through a symlink (macOS:
                    // /var is /private/var). The file itself may be gone: resolve its parent.
                    let parent = p.parent().ok_or(())?.canonicalize().map_err(|_| ())?;
                    let name = p.file_name().ok_or(())?;
                    parent
                        .join(name)
                        .strip_prefix(root_path)
                        .map(Path::to_path_buf)
                        .map_err(|_| ())
                });
            inside.ok()?.to_str().map(|s| s.to_string())
        };

        match event.kind {
            // A rename the debouncer paired: [from, to].
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() == 2 => {
                if let (Some(old), Some(new)) =
                    (relative(&event.paths[0]), relative(&event.paths[1]))
                {
                    self.moved(&old, &new).await?;
                }
            }
            EventKind::Remove(_) => {
                for rel in event.paths.iter().filter_map(|p| relative(p)) {
                    if !self.workspace.root.exists(&rel) {
                        self.removed(&rel).await?;
                    }
                }
            }
            EventKind::Create(_) | EventKind::Modify(_) => {
                for rel in event.paths.iter().filter_map(|p| relative(p)) {
                    self.import(&rel).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Documents living at `scope` or under it, as `(id, path)`.
    async fn tracked_under(&self, scope: &str) -> Vec<(String, String)> {
        let inside = format!("{scope}/");
        self.workspace
            .list_documents()
            .await
            .into_iter()
            .filter(|d| !d.deleted && (d.path == scope || d.path.starts_with(&inside)))
            .map(|d| (d.id, d.path))
            .collect()
    }

    /// A file or a whole folder moved. Moving a folder raises one event for the folder
    /// and none for what it holds, so every document under it follows here.
    async fn moved(&self, old: &str, new: &str) -> Result<()> {
        for (id, path) in self.tracked_under(old).await {
            let target = format!("{new}{}", &path[old.len()..]);
            let owner = self.workspace.get_doc_id_by_path(&target).await;
            if owner.is_some_and(|owner| owner != id) {
                // An editor followed a rename the session already made: the file that
                // left was the other note's. This one keeps its path; restore its file.
                let _ = self.project_to_disk(&id).await;
            } else if target.ends_with(".md") {
                self.workspace.rename_document(&id, &target).await?;
            } else if !self.workspace.is_leased(&id).await {
                // Renamed out of what we track (`note.md` -> `note.txt`).
                self.workspace.delete_document(&id).await?;
            }
        }
        // Whatever arrived without an identity: `draft.txt` -> `draft.md`, or
        // untracked notes inside a moved folder.
        self.import(new).await
    }

    /// A file or a whole folder disappeared.
    async fn removed(&self, rel: &str) -> Result<()> {
        for (id, _) in self.tracked_under(rel).await {
            // If buffer is currently open in an editor, do not delete!
            if !self.workspace.is_leased(&id).await {
                self.workspace.delete_document(&id).await?;
            }
        }
        Ok(())
    }

    /// Bring a file, or every note of a folder, into the CRDT.
    async fn import(&self, rel: &str) -> Result<()> {
        if rel.starts_with(".collab") {
            return Ok(());
        }
        if self.workspace.root.is_dir(rel) {
            let inside = format!("{rel}/");
            for path in self.workspace.root.list_markdown()? {
                if path.starts_with(&inside) {
                    self.import_file(&path).await?;
                }
            }
            return Ok(());
        }
        self.import_file(rel).await
    }

    /// The tracked note whose file disappeared and whose text is `content`. An open note
    /// may be ahead of its file, so a single open candidate is accepted on its own.
    async fn vanished_note(&self, content: &str) -> Option<String> {
        let mut open = Vec::new();
        for doc in self.workspace.list_documents().await {
            if doc.deleted || self.workspace.root.exists(&doc.path) {
                continue;
            }
            if self.workspace.get_text(&doc.id).await.ok().as_deref() == Some(content) {
                return Some(doc.id);
            }
            if self.workspace.is_leased(&doc.id).await {
                open.push(doc.id);
            }
        }
        (open.len() == 1).then(|| open.remove(0))
    }

    async fn import_file(&self, rel: &str) -> Result<()> {
        if !rel.ends_with(".md") || !self.workspace.root.is_file(rel) {
            return Ok(());
        }
        let Ok(disk_content) = self.workspace.root.read(rel) else {
            return Ok(());
        };
        let disk_hash = blake3::hash(disk_content.as_bytes());
        // Filter out echo events from our own disk projection writes
        if self.written.is_echo(rel, disk_hash) {
            return Ok(());
        }
        let doc_id = if let Some(doc_id) = self.workspace.get_doc_id_by_path(rel).await {
            // If leased by editor, don't overwrite user buffer!
            if self.workspace.is_leased(&doc_id).await {
                return Ok(());
            }
            if self.workspace.get_text(&doc_id).await.unwrap_or_default() != disk_content {
                tracing::info!(doc_id = %doc_id, path = %rel, "FS event updating doc text");
                self.workspace.set_text(&doc_id, &disk_content).await?;
            }
            doc_id
        } else if let Some(id) = self.vanished_note(&disk_content).await {
            // The watcher does not always pair a rename (a move right after one of our
            // own atomic writes is reported as unrelated events). A tracked note whose
            // file is gone, reappearing here, is that note -- not a new one.
            tracing::info!(doc_id = %id, path = %rel, "FS event following an unpaired rename");
            self.workspace.rename_document(&id, rel).await?;
            id
        } else {
            tracing::info!(path = %rel, "FS event creating new doc");
            self.workspace.create_document(rel, &disk_content).await?
        };
        self.written.record(rel, disk_hash);
        self.workspace
            .storage
            .record_disk(&doc_id, Some(disk_hash))?;
        Ok(())
    }

    /// Catch up with what happened to the folder while no daemon was watching it: notes
    /// deleted, edited, created or moved. Runs once, before the first watcher event.
    ///
    /// The file alone cannot tell "edited meanwhile" from "never received the latest
    /// text", nor "deleted meanwhile" from "never written": the hash of what we last saw
    /// in each file, kept in the store, does.
    pub async fn reconcile(&self) -> Result<()> {
        let storage = &self.workspace.storage;
        let seen = storage.disk_state()?;
        // A store from before 0.5.2 has seen nothing: there, a missing file is a deletion,
        // which is what it nearly always was.
        let tracked = storage.get_config("disk_tracking")?.is_some();
        let mut missing = Vec::new();
        for doc in self.workspace.list_documents().await {
            if doc.deleted {
                continue;
            }
            let Ok(content) = self.workspace.root.read(&doc.path) else {
                missing.push(doc);
                continue;
            };
            let hash = blake3::hash(content.as_bytes());
            if self.workspace.get_text(&doc.id).await? == content {
                storage.record_disk(&doc.id, Some(hash))?;
            } else if seen.get(&doc.id) == Some(&hash) {
                // The file did not move; the note did (a peer's edit never projected).
                self.project_to_disk(&doc.id).await?;
            } else {
                tracing::info!(doc_id = %doc.id, path = %doc.path, "reconcile: edited while stopped");
                self.written.record(&doc.path, hash);
                self.workspace.set_text(&doc.id, &content).await?;
                storage.record_disk(&doc.id, Some(hash))?;
            }
        }
        // Files without a note: created meanwhile -- or a missing note that moved here.
        for path in self.workspace.root.list_markdown()? {
            if self.workspace.get_doc_id_by_path(&path).await.is_none() {
                self.import_file(&path).await?;
            }
        }
        for doc in missing {
            let still = self.workspace.locate(&doc.id).await;
            if still != Some((doc.path.clone(), false)) || self.workspace.root.exists(&doc.path) {
                continue; // it was the note that moved
            }
            if !tracked || seen.contains_key(&doc.id) {
                tracing::info!(doc_id = %doc.id, path = %doc.path, "reconcile: deleted while stopped");
                self.workspace.delete_document(&doc.id).await?;
            } else {
                self.project_to_disk(&doc.id).await?;
            }
        }
        storage.set_config("disk_tracking", "true")?;
        Ok(())
    }

    /// Materialize a Yrs document update to disk if not leased by an editor.
    pub async fn project_to_disk(&self, doc_id: &str) -> Result<()> {
        if self.workspace.is_leased(doc_id).await {
            return Ok(());
        }
        let (path, deleted) = self
            .workspace
            .locate(doc_id)
            .await
            .ok_or_else(|| anyhow!("document_not_found"))?;

        // A rename that waited for the editor to let go. The editor may have moved
        // the file itself meanwhile, or another note may now own the old path.
        if let Some(old) = self.deferred_moves.write().await.remove(doc_id)
            && old != path
            && self.workspace.root.exists(&old)
            && self.workspace.get_doc_id_by_path(&old).await.is_none()
        {
            let root = &self.workspace.root;
            let _ = if deleted || root.exists(&path) {
                root.remove(&old)
            } else {
                root.rename(&old, &path)
            };
        }

        if deleted {
            // The path may already belong to a note created after the deletion.
            if self.workspace.get_doc_id_by_path(&path).await.is_none()
                && self.workspace.root.exists(&path)
            {
                let _ = self.workspace.root.remove(&path);
            }
            self.written.forget(&path);
            self.workspace.storage.record_disk(doc_id, None)?;
        } else {
            let text = self.workspace.get_text(doc_id).await?;
            let hash = blake3::hash(text.as_bytes());
            // Before the write, so the watcher can never see the file first.
            self.written.record(&path, hash);
            // Atomic write + fsync: keep it off the async workers.
            let root = self.workspace.root.clone();
            let storage = self.workspace.storage.clone();
            let id = doc_id.to_string();
            tokio::task::spawn_blocking(move || {
                root.write(&path, &text)?;
                storage.record_disk(&id, Some(hash))
            })
            .await??;
        }
        Ok(())
    }

    /// Background loop listening for Workspace events to project to disk.
    pub async fn run_projection_loop(&self) {
        let mut rx = self.workspace.events.subscribe();
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                Err(RecvError::Closed) => break,
                // Missed events: bring every unleased document back in line.
                Err(RecvError::Lagged(_)) => {
                    for doc in self.workspace.list_documents().await {
                        let _ = self.project_to_disk(&doc.id).await;
                    }
                    continue;
                }
            };
            match event {
                WorkspaceEvent::DocChanged { id, .. } => {
                    let _ = self.project_to_disk(&id).await;
                }
                WorkspaceEvent::DocRenamed {
                    id,
                    old_path,
                    new_path,
                    ..
                } => {
                    if self.workspace.is_leased(&id).await {
                        // Keep the first path: that is where the file still is.
                        self.deferred_moves
                            .write()
                            .await
                            .entry(id)
                            .or_insert(old_path);
                    } else {
                        if self.workspace.root.exists(&old_path)
                            && !self.workspace.root.exists(&new_path)
                        {
                            let _ = self.workspace.root.rename(&old_path, &new_path);
                        }
                        let _ = self.project_to_disk(&id).await;
                    }
                }
                WorkspaceEvent::DocDeleted { id, path, .. } => {
                    // The path may already belong to another note (a settled collision).
                    if !self.workspace.is_leased(&id).await
                        && self.workspace.get_doc_id_by_path(&path).await.is_none()
                        && self.workspace.root.exists(&path)
                    {
                        let _ = self.workspace.root.remove(&path);
                    }
                }
                WorkspaceEvent::Awareness { .. } => {}
            }
        }
    }

    /// Spawns the complete reactive projector: debounced disk watcher + projection loop.
    /// Start projecting. Returns once the folder has been caught up with (see `reconcile`),
    /// so call it before anything that lets peers or editors in.
    pub async fn spawn(workspace: Workspace) -> Result<ProjectorHandle> {
        let projector = Arc::new(Self::new(workspace.clone()));

        // 1. Projection loop (CRDT events -> disk)
        let p_loop = projector.clone();
        let projection_task = tokio::spawn(async move {
            p_loop.run_projection_loop().await;
        });

        // 2. Debounced filesystem watcher (disk changes -> CRDT)
        let (wake_tx, mut wake_rx) = tokio::sync::mpsc::channel(1024);
        let root_path = workspace.root.path.clone();

        let debouncer = notify_debouncer_full::new_debouncer(
            std::time::Duration::from_millis(100),
            None,
            move |result: notify_debouncer_full::DebounceEventResult| match result {
                Ok(batch) => {
                    let events: Vec<_> = batch
                        .into_iter()
                        .map(|e| e.event)
                        .filter(|e| {
                            matches!(
                                e.kind,
                                notify::EventKind::Create(_)
                                    | notify::EventKind::Modify(_)
                                    | notify::EventKind::Remove(_)
                            ) && e
                                .paths
                                .iter()
                                .any(|p| !p.components().any(|c| c.as_os_str() == ".collab"))
                        })
                        .collect();
                    if !events.is_empty() {
                        let _ = wake_tx.try_send(events);
                    }
                }
                Err(e) => {
                    tracing::warn!(errors = ?e, "watcher_error");
                }
            },
        )?;

        let mut watcher = debouncer;
        watcher.watch(&root_path, notify::RecursiveMode::Recursive)?;

        let p_fs = projector.clone();
        // The watcher is already listening: what changes during this pass is queued.
        projector.reconcile().await?;

        let fs_task = tokio::spawn(async move {
            while let Some(mut events) = wake_rx.recv().await {
                // Within a batch: moves, then appearances, then disappearances, so a
                // note is re-homed before its old path is seen missing.
                events.sort_by_key(|e: &notify::Event| match e.kind {
                    EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => 0,
                    EventKind::Remove(_) => 2,
                    _ => 1,
                });
                for ev in events {
                    let _ = p_fs.handle_fs_event(&ev).await;
                }
            }
        });

        Ok(ProjectorHandle {
            projection_task,
            fs_task,
            _debouncer: watcher,
        })
    }
}

pub struct ProjectorHandle {
    projection_task: tokio::task::JoinHandle<()>,
    fs_task: tokio::task::JoinHandle<()>,
    _debouncer: notify_debouncer_full::Debouncer<
        notify::RecommendedWatcher,
        notify_debouncer_full::RecommendedCache,
    >,
}

impl ProjectorHandle {
    pub fn abort(&self) {
        self.projection_task.abort();
        self.fs_task.abort();
    }
}
