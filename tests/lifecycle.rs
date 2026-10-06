//! The session's process: started by another program, it ends with it.
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

#[test]
fn a_session_started_with_exit_with_parent_ends_when_its_input_closes() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("note.md"), "bonjour\n").unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_deepika-sync"))
        .args(["share", dir.path().to_str().unwrap(), "--ws-port", "0"])
        .arg("--exit-with-parent")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let session = dir.path().join(".collab/session.json");
    let within = |what: &str, mut done: Box<dyn FnMut() -> bool>| {
        let began = Instant::now();
        while !done() {
            assert!(began.elapsed() < Duration::from_secs(20), "timeout: {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    within("session announced", Box::new(|| session.exists()));
    // Holding the pipe open keeps it alive...
    std::thread::sleep(Duration::from_millis(500));
    assert!(daemon.try_wait().unwrap().is_none());
    // ...and losing it, as when the parent dies, is a clean shutdown.
    drop(daemon.stdin.take());
    let mut status = None;
    within(
        "daemon exit",
        Box::new(|| {
            status = daemon.try_wait().unwrap();
            status.is_some()
        }),
    );
    assert!(status.unwrap().success());
    assert!(!session.exists(), "the session file is withdrawn");
}
