//! Durable SQLite storage for Yrs CRDT updates and workspace state.
use crate::filesystem::Root;
use anyhow::{Result, bail, ensure};
use cap_std::fs::{Dir, OpenOptions};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{os::unix::fs::PermissionsExt, sync::Mutex};

/// One row of the `documents` index: `(id, path, deleted)`.
pub type IndexRow = (String, String, bool);

pub struct StoredDoc {
    pub id: String,
    pub path: String,
    pub deleted: bool,
    pub updates: Vec<Vec<u8>>,
}

pub struct Storage {
    db: Mutex<Connection>,
    _dir: Dir,
    _lock: std::fs::File,
}

impl Storage {
    pub fn open(root: &Root) -> Result<Self> {
        if let Ok(m) = root.dir.symlink_metadata(".collab") {
            ensure!(m.is_dir() && !m.is_symlink(), "unsafe_state_directory");
        } else {
            root.dir.create_dir(".collab")?;
        }
        root.dir.set_permissions(
            ".collab",
            cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o700)),
        )?;
        let dir = root.dir.open_dir(".collab")?;
        for name in [
            "lock",
            "state.sqlite",
            "state.sqlite-wal",
            "state.sqlite-shm",
            "state.sqlite-journal",
        ] {
            if let Ok(m) = dir.symlink_metadata(name) {
                ensure!(m.is_file() && !m.is_symlink(), "unsafe_state_file:{name}");
            }
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true);
        let lock = dir.open_with("lock", &opts)?.into_std();
        if lock.try_lock().is_err() {
            bail!("session_already_running: un daemon deepika-sync tient déjà cette racine");
        }

        let private = std::fs::canonicalize(root.path.join(".collab"))?;
        ensure!(
            private == root.path.join(".collab"),
            "unsafe_state_directory"
        );
        let path = private.join("state.sqlite");
        let db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;

        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA temp_store=MEMORY;
             PRAGMA foreign_keys=ON;
             PRAGMA trusted_schema=OFF;
             CREATE TABLE IF NOT EXISTS documents(
                 id TEXT PRIMARY KEY,
                 path TEXT NOT NULL,
                 deleted INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS doc_updates(
                 document_id TEXT NOT NULL,
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 data BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_doc_updates ON doc_updates(document_id, seq);
             CREATE TABLE IF NOT EXISTS snapshots(
                 document_id TEXT PRIMARY KEY,
                 seq INTEGER NOT NULL,
                 data BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS config(
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS disk_state(
                 document_id TEXT PRIMARY KEY,
                 hash BLOB NOT NULL
             );",
        )?;

        // Before 0.5 `path` was UNIQUE, which refused to recreate a deleted note's path.
        let legacy: bool = db.query_row(
            "SELECT sql LIKE '%UNIQUE%' FROM sqlite_master WHERE name = 'documents'",
            [],
            |r| r.get(0),
        )?;
        if legacy {
            db.execute_batch(
                "BEGIN;
                 ALTER TABLE documents RENAME TO documents_legacy;
                 CREATE TABLE documents(
                     id TEXT PRIMARY KEY,
                     path TEXT NOT NULL,
                     deleted INTEGER NOT NULL DEFAULT 0
                 );
                 INSERT INTO documents SELECT id, path, deleted FROM documents_legacy;
                 DROP TABLE documents_legacy;
                 COMMIT;",
            )?;
        }

        Ok(Self {
            db: Mutex::new(db),
            _dir: dir,
            _lock: lock,
        })
    }

    /// Read the index of a session without taking its lock, so inspection commands
    /// work while the daemon runs. Returns the capability and `(id, path, deleted)` rows.
    pub fn peek(root: &std::path::Path) -> Result<(String, Vec<IndexRow>)> {
        let path = root.canonicalize()?.join(".collab/state.sqlite");
        ensure!(path.is_file(), "no_session_in_this_root");
        let db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        let capability = db
            .query_row(
                "SELECT value FROM config WHERE key = 'capability'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();
        let rows = db
            .prepare("SELECT id, path, deleted FROM documents ORDER BY path")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok((capability, rows))
    }

    pub fn load_all_documents(&self) -> Result<Vec<StoredDoc>> {
        let db = self.db.lock().unwrap();

        // 1. Load all snapshots into a map: doc_id -> (seq, data)
        let mut snap_stmt = db.prepare("SELECT document_id, seq, data FROM snapshots")?;
        let mut snapshots = std::collections::HashMap::<String, (i64, Vec<u8>)>::new();
        let snap_rows = snap_stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        for r in snap_rows {
            let (doc_id, seq, data) = r?;
            snapshots.insert(doc_id, (seq, data));
        }

        // 2. Load all incremental updates: doc_id -> Vec<(seq, data)>
        let mut u_stmt =
            db.prepare("SELECT document_id, seq, data FROM doc_updates ORDER BY seq ASC")?;
        let mut doc_updates = std::collections::HashMap::<String, Vec<(i64, Vec<u8>)>>::new();
        let u_rows = u_stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        for r in u_rows {
            let (doc_id, seq, data) = r?;
            doc_updates.entry(doc_id).or_default().push((seq, data));
        }

        // 3. Assemble documents
        let mut stmt = db.prepare("SELECT id, path, deleted FROM documents")?;
        let doc_rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
            ))
        })?;

        let mut docs = Vec::new();
        for r in doc_rows {
            let (id, path, deleted) = r?;
            let mut updates = Vec::new();

            let min_seq = if let Some((seq, data)) = snapshots.remove(&id) {
                updates.push(data);
                seq
            } else {
                0
            };

            if let Some(ups) = doc_updates.remove(&id) {
                for (seq, data) in ups {
                    if seq > min_seq {
                        updates.push(data);
                    }
                }
            }

            docs.push(StoredDoc {
                id,
                path,
                deleted,
                updates,
            });
        }
        Ok(docs)
    }

    /// Append one update and refresh the `documents` index row, atomically.
    pub fn persist(&self, id: &str, path: &str, deleted: bool, update: &[u8]) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO documents(id, path, deleted) VALUES(?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET path=excluded.path, deleted=excluded.deleted",
            params![id, path, deleted],
        )?;
        if !update.is_empty() {
            tx.execute(
                "INSERT INTO doc_updates(document_id, data) VALUES(?1, ?2)",
                params![id, update],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn compact_document(&self, id: &str, snapshot: &[u8]) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let max_seq: Option<i64> = tx
            .query_row(
                "SELECT MAX(seq) FROM doc_updates WHERE document_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;

        if let Some(seq) = max_seq {
            tx.execute(
                "INSERT INTO snapshots(document_id, seq, data) VALUES(?1, ?2, ?3)
                 ON CONFLICT(document_id) DO UPDATE SET seq=excluded.seq, data=excluded.data",
                params![id, seq, snapshot],
            )?;
            tx.execute(
                "DELETE FROM doc_updates WHERE document_id = ?1 AND seq <= ?2",
                params![id, seq],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The text last seen in a note's file, by us writing it or reading it. At the next
    /// start it tells what happened on disk meanwhile from what merely never reached it.
    pub fn record_disk(&self, id: &str, hash: Option<blake3::Hash>) -> Result<()> {
        let db = self.db.lock().unwrap();
        match hash {
            Some(hash) => db.execute(
                "INSERT INTO disk_state(document_id, hash) VALUES(?1, ?2)
                 ON CONFLICT(document_id) DO UPDATE SET hash=excluded.hash",
                params![id, hash.as_bytes().as_slice()],
            )?,
            None => db.execute("DELETE FROM disk_state WHERE document_id = ?1", params![id])?,
        };
        Ok(())
    }

    pub fn disk_state(&self) -> Result<std::collections::HashMap<String, blake3::Hash>> {
        let db = self.db.lock().unwrap();
        let mut rows = db.prepare("SELECT document_id, hash FROM disk_state")?;
        let seen = rows
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })?
            .filter_map(|row| {
                let (id, hash) = row.ok()?;
                Some((id, blake3::Hash::from_bytes(hash.try_into().ok()?)))
            })
            .collect();
        Ok(seen)
    }

    pub fn get_config(&self, key: &str) -> Result<Option<String>> {
        let db = self.db.lock().unwrap();
        let val = db
            .query_row(
                "SELECT value FROM config WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(val)
    }

    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO config(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}
