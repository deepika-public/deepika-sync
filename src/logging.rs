//! One stream of events, two destinations.
//!
//! A daemon started from a terminal writes readable lines to stderr. A daemon started
//! by an editor has no terminal at all -- the plugin keeps only the last few kilobytes
//! of its stderr, and only shows them if the process dies -- so it also writes a
//! bounded JSON file inside `.collab`, which survives and is machine-readable.
//!
//! What is never logged: note text, invitation codes and capabilities. Paths and
//! document identifiers are logged, because a conflict cannot be diagnosed without
//! them, and the file lives inside the user's own session directory.
use anyhow::Result;
use file_rotate::{ContentLimit, FileRotate, compression::Compression, suffix::AppendCount};
use std::{
    io,
    path::{Path, PathBuf},
};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Kept small enough to attach to a message, large enough to hold a long session.
const MAX_BYTES: usize = 8 * 1024 * 1024;

fn filter(variable: &str, fallback: &str) -> EnvFilter {
    std::env::var(variable)
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new(fallback))
}

/// What `init` hands back: the log file, and the guard flushing it on drop.
pub struct Logging {
    pub path: Option<PathBuf>,
    _guard: Option<WorkerGuard>,
}

/// Start logging. `root` is the session directory for a daemon that will keep
/// running; short CLI calls pass `None` and only write to stderr.
///
/// `RUST_LOG` controls the terminal, `DEEPIKA_SYNC_LOG` the file.
pub fn init(root: Option<&Path>) -> Result<Logging> {
    let terminal = fmt::layer()
        .with_writer(io::stderr)
        .with_target(false)
        .with_filter(filter("RUST_LOG", "deepika_sync=info"));
    let mut logging = Logging {
        path: None,
        _guard: None,
    };
    let file = match root {
        Some(root) => {
            // Absolute, so the line printed at startup can be copied straight into a
            // command from anywhere.
            let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            let directory = root.join(".collab");
            std::fs::create_dir_all(&directory)?;
            let at = directory.join("daemon.log");
            // One backup (`daemon.log.1`), then start fresh: a session can never fill
            // the disk, and the previous run stays readable.
            let rotating = FileRotate::new(
                &at,
                AppendCount::new(1),
                ContentLimit::BytesSurpassed(MAX_BYTES),
                Compression::None,
                None,
            );
            let (writer, guard) = tracing_appender::non_blocking(rotating);
            logging = Logging {
                path: Some(at),
                _guard: Some(guard),
            };
            Some(
                fmt::layer()
                    .json()
                    .with_ansi(false)
                    .with_writer(writer)
                    .with_filter(filter("DEEPIKA_SYNC_LOG", "deepika_sync=debug")),
            )
        }
        None => None,
    };
    tracing_subscriber::registry()
        .with(terminal)
        .with(file)
        .init();
    Ok(logging)
}
