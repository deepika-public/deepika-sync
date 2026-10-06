//! What joining a session would do to a folder, worked out before anything is touched.
//!
//! The joiner fetches the session's manifest -- paths and text hashes, no text -- and
//! compares it with the folder as it stands. Nothing is written: not the state
//! directory, not even a log file. The user then accepts or walks away.
use crate::{filesystem::Root, storage::Storage};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

/// One document of the session, as its peers describe it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub id: String,
    pub path: String,
    pub deleted: bool,
    /// BLAKE3 of the text.
    pub hash: [u8; 32],
}

/// The folder before joining: its notes, and what it already knows of a session.
#[derive(Default)]
pub struct Local {
    /// Path -> BLAKE3 of the file.
    pub files: HashMap<String, [u8; 32]>,
    /// Document id -> `(path, deleted)`, when the folder took part in the session before.
    pub known: HashMap<String, (String, bool)>,
}

impl Local {
    /// Read the folder without creating or changing anything in it.
    pub fn scan(root: &Path) -> Result<Self> {
        let opened = Root::open(root)?;
        let mut files = HashMap::new();
        for path in opened.list_markdown()? {
            let text = opened.read(&path)?;
            files.insert(path, *blake3::hash(text.as_bytes()).as_bytes());
        }
        let known = match Storage::peek(root) {
            Ok((_, rows)) => rows
                .into_iter()
                .map(|(id, path, deleted)| (id, (path, deleted)))
                .collect(),
            Err(_) => HashMap::new(),
        };
        Ok(Self { files, known })
    }
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rename {
    pub from: String,
    pub to: String,
}

/// Every list holds paths relative to the shared folder, sorted.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JoinPlan {
    /// Live notes in the session.
    pub session_documents: usize,
    /// Notes of the session that will appear as new files.
    pub create: Vec<String>,
    /// Same name, different text, no common history: both are kept, one of them as
    /// `name (conflit xxxxxxxx).md`.
    pub conflict: Vec<String>,
    /// Known notes whose file will be merged with the session's text.
    pub update: Vec<String>,
    /// Known notes the session renamed.
    pub rename: Vec<Rename>,
    /// Known notes the session deleted: their file will be removed.
    pub delete: Vec<String>,
    /// Notes already identical on both sides: nothing happens to them.
    pub identical: usize,
    /// Local notes the session does not have: untouched here, sent to every peer.
    pub share: Vec<String>,
}

impl JoinPlan {
    /// True when joining leaves every file of this folder as it is.
    pub fn leaves_folder_untouched(&self) -> bool {
        self.create.is_empty()
            && self.conflict.is_empty()
            && self.update.is_empty()
            && self.rename.is_empty()
            && self.delete.is_empty()
    }
}

pub fn plan(manifest: &[ManifestEntry], local: &Local) -> JoinPlan {
    let mut plan = JoinPlan::default();
    let mut settled = HashSet::new();
    for entry in manifest {
        if !entry.deleted {
            plan.session_documents += 1;
        }
        if let Some((path, deleted)) = local.known.get(&entry.id) {
            // A note this folder already shares with the session.
            settled.insert(path.as_str());
            let Some(hash) = local.files.get(path).filter(|_| !deleted) else {
                continue;
            };
            if entry.deleted {
                plan.delete.push(path.clone());
                continue;
            }
            if entry.path != *path {
                plan.rename.push(Rename {
                    from: path.clone(),
                    to: entry.path.clone(),
                });
            }
            if *hash == entry.hash {
                plan.identical += 1;
            } else {
                plan.update.push(entry.path.clone());
            }
        } else if !entry.deleted {
            match local.files.get(&entry.path) {
                None => plan.create.push(entry.path.clone()),
                Some(hash) if *hash == entry.hash => {
                    plan.identical += 1;
                    settled.insert(entry.path.as_str());
                }
                Some(_) => {
                    plan.conflict.push(entry.path.clone());
                    settled.insert(entry.path.as_str());
                }
            }
        }
    }
    plan.share = local
        .files
        .keys()
        .filter(|path| !settled.contains(path.as_str()))
        .cloned()
        .collect();
    for list in [
        &mut plan.create,
        &mut plan.conflict,
        &mut plan.update,
        &mut plan.delete,
        &mut plan.share,
    ] {
        list.sort();
    }
    plan.rename.sort_by(|a, b| a.from.cmp(&b.from));
    plan
}

/// The plan as a person reads it in a terminal.
pub fn describe(plan: &JoinPlan) -> String {
    let mut out = format!("La session contient {} note(s).\n", plan.session_documents);
    let mut section = |title: &str, lines: Vec<String>| {
        if !lines.is_empty() {
            out.push_str(&format!("\n{title} ({}) :\n", lines.len()));
            for line in lines {
                out.push_str(&format!("  {line}\n"));
            }
        }
    };
    section("Fichiers créés dans ce dossier", plan.create.clone());
    section(
        "Même nom, contenu différent — les deux versions sont gardées, l'une sous « nom (conflit …).md »",
        plan.conflict.clone(),
    );
    section(
        "Fichiers fusionnés avec la version de la session",
        plan.update.clone(),
    );
    section(
        "Fichiers renommés",
        plan.rename
            .iter()
            .map(|r| format!("{} → {}", r.from, r.to))
            .collect(),
    );
    section(
        "Fichiers SUPPRIMÉS (supprimés dans la session)",
        plan.delete.clone(),
    );
    section(
        "Notes de ce dossier envoyées aux autres participants (inchangées ici)",
        plan.share.clone(),
    );
    if plan.identical > 0 {
        out.push_str(&format!(
            "\n{} note(s) déjà identiques : rien à faire.\n",
            plan.identical
        ));
    }
    if plan.leaves_folder_untouched() {
        out.push_str("\nAucun fichier de ce dossier ne sera modifié.\n");
    }
    out
}
