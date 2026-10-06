//! Workspace managing Yrs CRDT documents, open editor leases, and the event bus
//! every adapter (LSP, WebSocket, P2P transport, disk projection) listens to.
use crate::{
    filesystem::{self, Root},
    storage::Storage,
};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{RwLock, broadcast};
use yrs::{
    Any, Doc, GetString, Map, OffsetKind, Options, Out, ReadTxn, StateVector, Text, Transact,
    TransactionMut, Update,
    block::ClientID,
    sync::{
        Message, SyncMessage,
        awareness::{AwarenessUpdate, AwarenessUpdateEntry},
    },
    updates::{decoder::Decode, encoder::Encode},
};

/// Updates appended to SQLite before a document is folded into one snapshot.
const COMPACT_EVERY: u32 = 200;
/// An editor renews its presence every 15 s; one silent for this long is gone.
const PRESENCE_TTL: Duration = Duration::from_secs(30);

/// Who is in a note, as last heard: cursors and names, per editor client.
type Presence = HashMap<ClientID, (Instant, AwarenessUpdateEntry)>;

/// Identifies the connection a change came from, so it is not echoed back to it.
pub type Origin = Option<u64>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocSummary {
    pub id: String,
    pub path: String,
    pub deleted: bool,
}

#[derive(Clone, Debug)]
pub enum WorkspaceEvent {
    DocChanged {
        id: String,
        path: String,
        update: Vec<u8>,
        origin: Origin,
    },
    DocRenamed {
        id: String,
        old_path: String,
        new_path: String,
        update: Vec<u8>,
        origin: Origin,
    },
    DocDeleted {
        id: String,
        path: String,
        update: Vec<u8>,
        origin: Origin,
    },
    /// An encoded y-sync awareness message, relayed untouched.
    Awareness {
        id: String,
        payload: Vec<u8>,
        origin: Origin,
    },
}

impl WorkspaceEvent {
    pub fn id(&self) -> &str {
        match self {
            Self::DocChanged { id, .. }
            | Self::DocRenamed { id, .. }
            | Self::DocDeleted { id, .. }
            | Self::Awareness { id, .. } => id,
        }
    }

    pub fn origin(&self) -> Origin {
        match self {
            Self::DocChanged { origin, .. }
            | Self::DocRenamed { origin, .. }
            | Self::DocDeleted { origin, .. }
            | Self::Awareness { origin, .. } => *origin,
        }
    }

    /// The encoded y-sync message a connection forwards for this event, if any.
    pub fn wire(&self) -> Option<Vec<u8>> {
        match self {
            Self::DocChanged { update, .. }
            | Self::DocRenamed { update, .. }
            | Self::DocDeleted { update, .. } => (!update.is_empty())
                .then(|| Message::Sync(SyncMessage::Update(update.clone())).encode_v1()),
            Self::Awareness { payload, .. } => Some(payload.clone()),
        }
    }
}

/// Offsets are UTF-16 code units, the unit shared by LSP and Yjs clients.
pub fn new_doc() -> Doc {
    Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    })
}

/// Turn `text` into `new_text` with the smallest edits, so concurrent changes merge.
pub fn apply_text_diff(text: &yrs::TextRef, txn: &mut TransactionMut, new_text: &str) {
    let old_text = text.get_string(txn);
    let old: Vec<char> = old_text.chars().collect();
    let new: Vec<char> = new_text.chars().collect();
    let utf16 = |chars: &[char]| chars.iter().map(|c| c.len_utf16()).sum::<usize>() as u32;
    // Walk backwards so earlier offsets stay valid.
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &old, &new)
        .iter()
        .rev()
    {
        let (tag, old_range, new_range) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            continue;
        }
        let at = utf16(&old[..old_range.start]);
        if !old_range.is_empty() {
            text.remove_range(txn, at, utf16(&old[old_range]));
        }
        if !new_range.is_empty() {
            text.insert(txn, at, &new[new_range].iter().collect::<String>());
        }
    }
}

/// What a transaction did to a document.
enum Effect {
    /// New content: persist and announce these bytes.
    Changed(Vec<u8>),
    /// Nothing visible yet, but the update waits for a missing dependency: persist only.
    /// Announcing it would let it circle forever in a mesh with a cycle.
    Stashed(Vec<u8>),
    /// Already known. Dropping it here is what stops echoes between peers.
    Nothing,
}

/// A Yrs document plus the path and tombstone mirrored from its `metadata` map.
struct DocumentEntry {
    doc: Doc,
    path: String,
    deleted: bool,
    /// Updates persisted since the last snapshot.
    pending: u32,
}

impl DocumentEntry {
    fn new() -> Self {
        Self {
            doc: new_doc(),
            path: String::new(),
            deleted: false,
            pending: 0,
        }
    }

    fn text(&self) -> String {
        let text = self.doc.get_or_insert_text("content");
        text.get_string(&self.doc.transact())
    }

    /// Run `change` in one transaction and report what it really did. `raw` is the
    /// update being applied, when the change is not local.
    fn transact(
        &self,
        raw: Option<&[u8]>,
        change: impl FnOnce(&mut TransactionMut) -> Result<()>,
    ) -> Result<Effect> {
        let mut txn = self.doc.transact_mut();
        let before = txn.state_vector();
        change(&mut txn)?;
        let bytes =
            |txn: &TransactionMut| raw.map_or_else(|| txn.encode_update_v1(), <[u8]>::to_vec);
        Ok(
            if txn.state_vector() != before || !txn.delete_set().is_empty() {
                Effect::Changed(bytes(&txn))
            } else if txn.has_missing_updates() {
                Effect::Stashed(bytes(&txn))
            } else {
                Effect::Nothing
            },
        )
    }

    fn set_text(&self, new_text: &str) -> Result<Effect> {
        let text = self.doc.get_or_insert_text("content");
        self.transact(None, |txn| {
            apply_text_diff(&text, txn, new_text);
            Ok(())
        })
    }

    fn apply_edit(&self, from: usize, to: usize, insert: &str) -> Result<Effect> {
        let text = self.doc.get_or_insert_text("content");
        self.transact(None, |txn| {
            ensure!(
                from <= to && to <= text.len(txn) as usize,
                "invalid_edit_range"
            );
            if to > from {
                text.remove_range(txn, from as u32, (to - from) as u32);
            }
            if !insert.is_empty() {
                text.insert(txn, from as u32, insert);
            }
            Ok(())
        })
    }

    fn set_metadata(&self, path: Option<&str>, deleted: Option<bool>) -> Result<Effect> {
        let meta = self.doc.get_or_insert_map("metadata");
        self.transact(None, |txn| {
            if let Some(p) = path {
                meta.insert(txn, "path", p);
            }
            if let Some(d) = deleted {
                meta.insert(txn, "deleted", d);
            }
            Ok(())
        })
    }

    /// Retire this document in favour of `winner`, which holds the same text at the
    /// same path. An editor holding it open reads the marker and rebinds, instead of
    /// taking this for a deletion by a collaborator.
    fn retire_for(&self, winner: &str) -> Result<Effect> {
        let meta = self.doc.get_or_insert_map("metadata");
        self.transact(None, |txn| {
            meta.insert(txn, "deleted", true);
            meta.insert(txn, "superseded_by", winner);
            Ok(())
        })
    }

    fn apply_update(&self, update: &[u8]) -> Result<Effect> {
        let decoded = Update::decode_v1(update)?;
        self.transact(Some(update), |txn| Ok(txn.apply_update(decoded)?))
    }

    /// Re-read path and tombstone from the CRDT, the single source of truth.
    fn refresh(&mut self) {
        let meta = self.doc.get_or_insert_map("metadata");
        let txn = self.doc.transact();
        if let Some(Out::Any(Any::String(p))) = meta.get(&txn, "path") {
            self.path = p.to_string();
        }
        if let Some(Out::Any(Any::Bool(d))) = meta.get(&txn, "deleted") {
            self.deleted = d;
        }
    }
}

struct WorkspaceInner {
    documents: HashMap<String, DocumentEntry>,
    path_to_id: HashMap<String, String>,
    /// Open editor connections per document.
    leases: HashMap<String, usize>,
    capability: String,
}

/// What one committed mutation must persist and announce.
struct Commit {
    id: String,
    update: Vec<u8>,
    old_path: String,
    old_deleted: bool,
    path: String,
    deleted: bool,
    snapshot: Option<Vec<u8>>,
    origin: Origin,
    announce: bool,
}

#[derive(Clone)]
pub struct Workspace {
    pub root: Arc<Root>,
    pub storage: Arc<Storage>,
    inner: Arc<RwLock<WorkspaceInner>>,
    pub events: broadcast::Sender<WorkspaceEvent>,
    /// Never persisted, never interpreted: kept only so a newcomer sees who is already there.
    presence: Arc<Mutex<HashMap<String, Presence>>>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("root", &self.root.path)
            .finish()
    }
}

/// `notes/a.md` -> `notes/a (conflit 1a2b3c4d).md`, identical on every peer.
fn conflict_path(path: &str, id: &str) -> String {
    let tag = &id[..id.len().min(8)];
    match path.rsplit_once('.') {
        Some((stem, ext)) if !ext.contains('/') => format!("{stem} (conflit {tag}).{ext}"),
        _ => format!("{path} (conflit {tag})"),
    }
}

impl Workspace {
    pub async fn open(root_path: &Path) -> Result<Self> {
        let root = Arc::new(Root::open(root_path)?);
        let storage = Arc::new(Storage::open(&root)?);
        let capability = storage.get_config("capability")?.unwrap_or_default();

        let mut documents = HashMap::new();
        let mut path_to_id = HashMap::new();
        for stored in storage.load_all_documents()? {
            let mut entry = DocumentEntry::new();
            for update in &stored.updates {
                if let Err(e) = entry.apply_update(update) {
                    tracing::warn!(doc_id = %stored.id, error = %e, "stored_update_unreadable");
                }
            }
            (entry.path, entry.deleted) = (stored.path, stored.deleted);
            entry.refresh();
            // Start every session from one snapshot per document.
            if stored.updates.len() > 1 {
                let snapshot = entry.doc.transact().encode_diff_v1(&StateVector::default());
                storage.compact_document(&stored.id, &snapshot)?;
            }
            if !entry.deleted {
                path_to_id.insert(entry.path.clone(), stored.id.clone());
            }
            documents.insert(stored.id, entry);
        }

        let ws = Self {
            root,
            storage,
            inner: Arc::new(RwLock::new(WorkspaceInner {
                documents,
                path_to_id,
                leases: HashMap::new(),
                capability,
            })),
            events: broadcast::channel(1024).0,
            presence: Arc::default(),
        };

        if ws.storage.get_config("initialized")?.is_none() {
            for path in ws.root.list_markdown()? {
                if ws.get_doc_id_by_path(&path).await.is_none() {
                    let content = ws.root.read(&path)?;
                    ws.create_document(&path, &content).await?;
                }
            }
            ws.storage.set_config("initialized", "true")?;
        }
        Ok(ws)
    }

    pub async fn capability(&self) -> String {
        self.inner.read().await.capability.clone()
    }

    pub async fn set_capability(&self, cap: &str) -> Result<()> {
        self.inner.write().await.capability = cap.to_string();
        self.storage.set_config("capability", cap)
    }

    /// The single mutation path: run `change` on the document, mirror its metadata,
    /// settle path collisions, persist, then announce. Returns the update produced.
    async fn mutate(
        &self,
        id: &str,
        origin: Origin,
        create: bool,
        change: impl FnOnce(&DocumentEntry) -> Result<Effect>,
    ) -> Result<Vec<u8>> {
        let mut commits = Vec::new();
        {
            let mut guard = self.inner.write().await;
            let inner = &mut *guard;
            let adopted = create && !inner.documents.contains_key(id);
            if adopted {
                inner.documents.insert(id.to_string(), DocumentEntry::new());
            }
            let entry = inner
                .documents
                .get_mut(id)
                .ok_or_else(|| anyhow!("document_not_found"))?;
            let (old_path, old_deleted) = (entry.path.clone(), entry.deleted);
            let (update, announce) = match change(entry) {
                Ok(Effect::Changed(update)) => (update, true),
                Ok(Effect::Stashed(update)) => (update, false),
                Ok(Effect::Nothing) | Err(_) if adopted => {
                    inner.documents.remove(id);
                    return Ok(Vec::new());
                }
                Ok(Effect::Nothing) => return Ok(Vec::new()),
                Err(e) => return Err(e),
            };
            entry.refresh();
            let (path, deleted) = (entry.path.clone(), entry.deleted);

            if inner
                .path_to_id
                .get(&old_path)
                .is_some_and(|owner| owner == id)
            {
                inner.path_to_id.remove(&old_path);
            }
            // Two peers created the same path: the greater id steps aside, everywhere.
            // An identical copy (a folder joined with the same files) is simply retired.
            let rival = inner.path_to_id.get(&path).filter(|r| *r != id).cloned();
            if let Some(rival) = rival.filter(|_| !deleted) {
                let redundant = inner.documents[&rival].text() == inner.documents[id].text();
                let (loser, winner) = if rival.as_str() > id {
                    (rival, id.to_string())
                } else {
                    (id.to_string(), rival)
                };
                let moved = conflict_path(&path, &loser);
                tracing::warn!(path = %path, loser = %loser, redundant, "path_collision");
                let entry = inner.documents.get_mut(&loser).expect("loser is tracked");
                let settle = if redundant {
                    entry.retire_for(&winner)?
                } else {
                    entry.set_metadata(Some(&moved), None)?
                };
                let Effect::Changed(settle) = settle else {
                    unreachable!("a metadata write always adds a block");
                };
                entry.refresh();
                let (settled_path, settled_deleted) = (entry.path.clone(), entry.deleted);
                if !redundant {
                    inner.path_to_id.insert(moved, loser.clone());
                }
                if loser == id {
                    commits.push(Self::commit(
                        inner,
                        id,
                        update,
                        old_path,
                        old_deleted,
                        origin,
                        announce,
                    ));
                    // `path` still belongs to the rival: announce nothing that touches its file.
                    commits.push(Self::commit(
                        inner,
                        id,
                        settle,
                        settled_path,
                        settled_deleted,
                        None,
                        true,
                    ));
                } else {
                    inner.path_to_id.insert(path.clone(), id.to_string());
                    commits.push(Self::commit(inner, &loser, settle, path, false, None, true));
                    commits.push(Self::commit(
                        inner,
                        id,
                        update,
                        old_path,
                        old_deleted,
                        origin,
                        announce,
                    ));
                }
            } else {
                if !deleted && !path.is_empty() {
                    inner.path_to_id.insert(path, id.to_string());
                }
                commits.push(Self::commit(
                    inner,
                    id,
                    update,
                    old_path,
                    old_deleted,
                    origin,
                    announce,
                ));
            }
        }

        let own_update = commits
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.update.clone())
            .unwrap_or_default();
        for commit in commits {
            self.persist_and_announce(commit).await?;
        }
        Ok(own_update)
    }

    fn commit(
        inner: &mut WorkspaceInner,
        id: &str,
        update: Vec<u8>,
        old_path: String,
        old_deleted: bool,
        origin: Origin,
        announce: bool,
    ) -> Commit {
        let entry = inner
            .documents
            .get_mut(id)
            .expect("committed doc is tracked");
        entry.pending += 1;
        let snapshot = (entry.pending >= COMPACT_EVERY).then(|| {
            entry.pending = 0;
            entry.doc.transact().encode_diff_v1(&StateVector::default())
        });
        Commit {
            id: id.to_string(),
            update,
            old_path,
            old_deleted,
            path: entry.path.clone(),
            deleted: entry.deleted,
            snapshot,
            origin,
            announce,
        }
    }

    async fn persist_and_announce(&self, c: Commit) -> Result<()> {
        let storage = self.storage.clone();
        let (id, update, path, deleted, snapshot) = (
            c.id.clone(),
            c.update.clone(),
            c.path.clone(),
            c.deleted,
            c.snapshot,
        );
        // SQLite fsyncs on every commit; keep that off the async workers.
        tokio::task::spawn_blocking(move || -> Result<()> {
            storage.persist(&id, &path, deleted, &update)?;
            if let Some(snapshot) = snapshot {
                storage.compact_document(&id, &snapshot)?;
            }
            Ok(())
        })
        .await??;

        let Commit {
            id,
            update,
            old_path,
            old_deleted,
            path,
            deleted,
            origin,
            announce,
            ..
        } = c;
        if !announce {
            return Ok(());
        }
        let event = if deleted && !old_deleted {
            WorkspaceEvent::DocDeleted {
                id,
                path: old_path,
                update,
                origin,
            }
        } else if !old_path.is_empty() && old_path != path {
            WorkspaceEvent::DocRenamed {
                id,
                old_path,
                new_path: path,
                update,
                origin,
            }
        } else {
            WorkspaceEvent::DocChanged {
                id,
                path,
                update,
                origin,
            }
        };
        let _ = self.events.send(event);
        Ok(())
    }

    pub async fn create_document(&self, path: &str, content: &str) -> Result<String> {
        filesystem::validate(path)?;
        let id = uuid::Uuid::new_v4().to_string();
        self.mutate(&id, None, true, |entry| {
            let meta = entry.doc.get_or_insert_map("metadata");
            let text = entry.doc.get_or_insert_text("content");
            let mut txn = entry.doc.transact_mut();
            text.push(&mut txn, content);
            meta.insert(&mut txn, "path", path);
            meta.insert(&mut txn, "deleted", false);
            Ok(Effect::Changed(txn.encode_update_v1()))
        })
        .await?;
        Ok(id)
    }

    pub async fn get_text(&self, id: &str) -> Result<String> {
        self.with_doc(id, |e| e.text()).await
    }

    pub async fn set_text(&self, id: &str, new_text: &str) -> Result<Vec<u8>> {
        self.mutate(id, None, false, |e| e.set_text(new_text)).await
    }

    /// Replace the UTF-16 range `from..to` with `insert`.
    pub async fn apply_edit(
        &self,
        id: &str,
        from: usize,
        to: usize,
        insert: &str,
    ) -> Result<Vec<u8>> {
        self.mutate(id, None, false, |e| e.apply_edit(from, to, insert))
            .await
    }

    /// Apply an update received from a peer or an editor, adopting unknown documents.
    pub async fn apply_remote_update(&self, id: &str, update: &[u8], origin: Origin) -> Result<()> {
        self.mutate(id, origin, true, |e| e.apply_update(update))
            .await?;
        Ok(())
    }

    pub async fn rename_document(&self, id: &str, new_path: &str) -> Result<()> {
        filesystem::validate(new_path)?;
        self.mutate(id, None, false, |e| e.set_metadata(Some(new_path), None))
            .await?;
        Ok(())
    }

    pub async fn delete_document(&self, id: &str) -> Result<()> {
        self.mutate(id, None, false, |e| e.set_metadata(None, Some(true)))
            .await?;
        Ok(())
    }

    /// Process one y-sync protocol message for a document; returns the reply, if any.
    pub async fn handle_y_msg(
        &self,
        id: &str,
        msg: Message,
        origin: Origin,
    ) -> Result<Option<Message>> {
        match msg {
            // A peer announcing a document we never saw gets asked for all of it.
            Message::Sync(SyncMessage::SyncStep1(sv)) => {
                Ok(Some(Message::Sync(match self.encode_diff(id, &sv).await {
                    Ok(diff) => SyncMessage::SyncStep2(diff),
                    Err(_) => SyncMessage::SyncStep1(StateVector::default()),
                })))
            }
            Message::Sync(SyncMessage::SyncStep2(update) | SyncMessage::Update(update)) => {
                self.apply_remote_update(id, &update, origin).await?;
                Ok(None)
            }
            Message::AwarenessQuery => Ok(self.presence(id)),
            msg @ Message::Awareness(_) => {
                if let Message::Awareness(update) = &msg {
                    self.remember_presence(id, update);
                }
                let _ = self.events.send(WorkspaceEvent::Awareness {
                    id: id.to_string(),
                    payload: msg.encode_v1(),
                    origin,
                });
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn remember_presence(&self, id: &str, update: &AwarenessUpdate) {
        let mut presence = self.presence.lock().unwrap();
        let seen = presence.entry(id.to_string()).or_default();
        for (client, entry) in &update.clients {
            if &*entry.json == "null" {
                seen.remove(client);
            } else if seen
                .get(client)
                .is_none_or(|(_, known)| known.clock <= entry.clock)
            {
                seen.insert(*client, (Instant::now(), entry.clone()));
            }
        }
        if seen.is_empty() {
            presence.remove(id);
        }
    }

    /// The editors currently in a note, for a connection that just arrived: awareness
    /// travels as changes, so without this a newcomer waits for everyone's next renewal.
    pub fn presence(&self, id: &str) -> Option<Message> {
        let mut presence = self.presence.lock().unwrap();
        let seen = presence.get_mut(id)?;
        seen.retain(|_, (heard, _)| heard.elapsed() < PRESENCE_TTL);
        let clients: HashMap<_, _> = seen.iter().map(|(c, (_, e))| (*c, e.clone())).collect();
        (!clients.is_empty()).then_some(Message::Awareness(AwarenessUpdate { clients }))
    }

    async fn with_doc<T>(&self, id: &str, read: impl FnOnce(&DocumentEntry) -> T) -> Result<T> {
        let inner = self.inner.read().await;
        inner
            .documents
            .get(id)
            .map(read)
            .ok_or_else(|| anyhow!("document_not_found"))
    }

    pub async fn get_doc_id_by_path(&self, path: &str) -> Option<String> {
        self.inner.read().await.path_to_id.get(path).cloned()
    }

    /// Path and tombstone of a document.
    pub async fn locate(&self, id: &str) -> Option<(String, bool)> {
        self.with_doc(id, |e| (e.path.clone(), e.deleted))
            .await
            .ok()
    }

    pub async fn list_documents(&self) -> Vec<DocSummary> {
        let inner = self.inner.read().await;
        inner
            .documents
            .iter()
            .map(|(id, e)| DocSummary {
                id: id.clone(),
                path: e.path.clone(),
                deleted: e.deleted,
            })
            .collect()
    }

    /// Paths and text hashes of every document, for a peer weighing whether to join.
    pub async fn manifest(&self) -> Vec<crate::preview::ManifestEntry> {
        let inner = self.inner.read().await;
        inner
            .documents
            .iter()
            .map(|(id, e)| crate::preview::ManifestEntry {
                id: id.clone(),
                path: e.path.clone(),
                deleted: e.deleted,
                hash: *blake3::hash(e.text().as_bytes()).as_bytes(),
            })
            .collect()
    }

    pub async fn acquire_lease(&self, id: &str) {
        *self
            .inner
            .write()
            .await
            .leases
            .entry(id.to_string())
            .or_default() += 1;
    }

    /// Dropping the last lease lets the projection catch up with what it deferred.
    pub async fn release_lease(&self, id: &str) {
        let freed = {
            let mut inner = self.inner.write().await;
            match inner.leases.get_mut(id) {
                Some(n) if *n > 1 => {
                    *n -= 1;
                    false
                }
                _ => inner.leases.remove(id).is_some(),
            }
        };
        // Tombstones too: a note deleted by a peer while open here still has its file.
        if freed && let Some((path, _)) = self.locate(id).await {
            let _ = self.events.send(WorkspaceEvent::DocChanged {
                id: id.to_string(),
                path,
                update: Vec::new(),
                origin: None,
            });
        }
    }

    pub async fn is_leased(&self, id: &str) -> bool {
        self.inner.read().await.leases.contains_key(id)
    }

    pub async fn encode_state_vector(&self, id: &str) -> Result<StateVector> {
        self.with_doc(id, |e| e.doc.transact().state_vector()).await
    }

    pub async fn encode_diff(&self, id: &str, sv: &StateVector) -> Result<Vec<u8>> {
        self.with_doc(id, |e| e.doc.transact().encode_diff_v1(sv))
            .await
    }
}
