//! Shared fixtures for tests.
#![allow(dead_code)]
use deepika_sync::workspace::Workspace;
use tempfile::TempDir;

/// A durable workspace on a fresh root. The directory must outlive the workspace.
pub async fn peer() -> (TempDir, Workspace) {
    let d = TempDir::new().unwrap();
    let ws = Workspace::open(d.path()).await.unwrap();
    (d, ws)
}
