//! Capability-relative access; never reopen a validated path through the ambient filesystem.
use anyhow::{Result, bail, ensure};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use std::{
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub struct Root {
    pub path: PathBuf,
    pub dir: Dir,
}
/// Pure path rules, independent of any capability.
pub fn validate(path: &str) -> Result<()> {
    ensure!(!path.is_empty() && !path.contains('\\'), "invalid_path");
    for c in Path::new(path).components() {
        match c {
            Component::Normal(p) => ensure!(
                !p.to_string_lossy().starts_with('.'),
                "hidden_path_forbidden"
            ),
            _ => bail!("invalid_path"),
        }
    }
    ensure!(
        path.split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".."),
        "invalid_path"
    );
    Ok(())
}

impl Root {
    pub fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let dir = Dir::open_ambient_dir(&path, ambient_authority())?;
        Ok(Self { path, dir })
    }
    pub fn checked(&self, path: &str) -> Result<()> {
        validate(path)?;
        let mut prefix = PathBuf::new();
        for c in Path::new(path).components() {
            prefix.push(c);
            match self.dir.symlink_metadata(&prefix) {
                Ok(m) => ensure!(!m.is_symlink(), "symlink_forbidden"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(e.into()),
            }
        }
        // cap-std canonicalization is relative to the already-open root capability.
        if self.dir.exists(path) {
            self.dir.canonicalize(path)?;
        }
        Ok(())
    }
    pub fn read(&self, path: &str) -> Result<String> {
        self.checked(path)?;
        let mut f = self.dir.open(path)?;
        let meta = f.metadata()?;
        ensure!(
            meta.is_file() && meta.len() <= 8 * 1024 * 1024,
            "unsupported_file"
        );
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        Ok(s)
    }
    pub fn write(&self, path: &str, text: &str) -> Result<()> {
        self.checked(path)?;
        if let Some(parent) = Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            self.dir.create_dir_all(parent)?;
        }
        self.checked(path)?;
        let tmp = format!(".collab/tmp-{}", uuid::Uuid::new_v4());
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut f = self.dir.open_with(&tmp, &options)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        self.dir.rename(&tmp, &self.dir, path)?;
        Ok(())
    }
    pub fn remove(&self, path: &str) -> Result<()> {
        self.checked(path)?;
        self.dir.remove_file(path)?;
        self.prune_empty_parents(path);
        Ok(())
    }
    /// A directory only exists here because a file needed it; we created it
    /// ourselves when projecting one. Once empty it has no meaning, and leaving
    /// it makes a peer's tree differ from the one that moved the folder.
    /// `remove_dir` refuses a directory that still holds anything -- an
    /// attachment, a file we do not track -- so that stops the walk by itself.
    fn prune_empty_parents(&self, path: &str) {
        let mut parent = Path::new(path).parent();
        while let Some(directory) = parent.filter(|d| !d.as_os_str().is_empty()) {
            if self.dir.remove_dir(directory).is_err() {
                break;
            }
            parent = directory.parent();
        }
    }
    pub fn files(&self) -> Result<Vec<String>> {
        fn walk(root: &Root, path: &Path, out: &mut Vec<String>) -> Result<()> {
            for e in root.dir.read_dir(if path.as_os_str().is_empty() {
                Path::new(".")
            } else {
                path
            })? {
                let e = e?;
                let name = e.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                let p = path.join(name);
                let s = p.to_str().ok_or_else(|| anyhow::anyhow!("non_utf8_path"))?;
                let ty = e.file_type()?;
                if ty.is_symlink() {
                    continue;
                }
                if ty.is_dir() {
                    walk(root, &p, out)?;
                } else if ty.is_file() {
                    root.checked(s)?;
                    if root.read(s).is_ok() {
                        out.push(s.to_owned());
                    }
                }
            }
            Ok(())
        }
        let mut out = vec![];
        walk(self, Path::new(""), &mut out)?;
        out.sort();
        Ok(out)
    }
    pub fn list_markdown(&self) -> Result<Vec<String>> {
        Ok(self
            .files()?
            .into_iter()
            .filter(|p| p.ends_with(".md"))
            .collect())
    }
    /// Move a file, creating the folders it moves into and pruning those it empties.
    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.checked(from)?;
        self.checked(to)?;
        if let Some(parent) = Path::new(to).parent()
            && !parent.as_os_str().is_empty()
        {
            self.dir.create_dir_all(parent)?;
        }
        self.dir.rename(from, &self.dir, to)?;
        self.prune_empty_parents(from);
        Ok(())
    }
    pub fn exists(&self, path: &str) -> bool {
        self.dir.exists(path)
    }
    pub fn is_dir(&self, path: &str) -> bool {
        self.dir.is_dir(path)
    }
    pub fn is_file(&self, path: &str) -> bool {
        self.dir.is_file(path)
    }
}
