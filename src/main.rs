use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use deepika_sync::{
    lsp,
    preview::{Local, describe, plan as join_plan},
    projection::Projector,
    storage::Storage,
    transport::{self, Invite},
    workspace::{DocSummary, Workspace},
    ws,
};
use std::{
    io::{IsTerminal, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(
    version,
    about = "Local-first collaboration daemon for Markdown (Linux)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Documents {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    Share {
        root: PathBuf,
        #[arg(long)]
        relay: bool,
        #[arg(long, default_value = "4444")]
        ws_port: u16,
        /// Stop when standard input closes: a program that started the session keeps a pipe
        /// open to it, and the session ends with that program, however it ends.
        #[arg(long)]
        exit_with_parent: bool,
    },
    /// Join a session. Shows what will change in the folder and asks before touching it.
    Join {
        invite: String,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        relay: bool,
        #[arg(long, default_value = "4444")]
        ws_port: u16,
        /// Print what joining would change, as JSON, and exit without changing anything.
        #[arg(long, conflicts_with = "yes")]
        preview: bool,
        /// Join without asking: the changes were already reviewed and accepted.
        #[arg(long)]
        yes: bool,
        /// Stop when standard input closes: a program that started the session keeps a pipe
        /// open to it, and the session ends with that program, however it ends.
        #[arg(long)]
        exit_with_parent: bool,
    },
    Status {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    Rename {
        document: String,
        path: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    Delete {
        document: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// LSP server on stdio; a client of the daemon already serving `root`.
    Lsp {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "4444")]
        ws_port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let command = Cli::parse().command;
    // Consent comes first: until the user accepts, joining writes nothing in the
    // folder -- no state directory, not even a log file.
    if let Command::Join {
        invite,
        root,
        relay,
        preview,
        yes: false,
        ..
    } = &command
        && !review_join(&Invite::decode(invite)?, root, *relay, *preview).await?
    {
        return Ok(());
    }
    let log_path = match &command {
        Command::Share { root, .. } | Command::Join { root, .. } => Some(root.clone()),
        _ => None,
    };
    let logging = deepika_sync::logging::init(log_path.as_deref())?;

    match command {
        // Inspection reads the index without the session lock, so it works while the daemon runs.
        Command::Documents { root } => {
            let docs: Vec<_> = Storage::peek(&root)?
                .1
                .into_iter()
                .map(|(id, path, deleted)| DocSummary { id, path, deleted })
                .collect();
            println!("{}", serde_json::to_string_pretty(&docs)?);
            Ok(())
        }
        Command::Status { root } => {
            let (capability, docs) = Storage::peek(&root)?;
            let status = serde_json::json!({
                "root": root.canonicalize()?.display().to_string(),
                "capability": capability,
                "documents_count": docs.iter().filter(|(.., deleted)| !deleted).count(),
            });
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Command::Rename {
            root,
            document,
            path,
        } => {
            let ws = Workspace::open(&root).await?;
            ws.rename_document(&document, &path).await?;
            println!("Document {} renommé en {}", document, path);
            Ok(())
        }
        Command::Delete { root, document } => {
            let ws = Workspace::open(&root).await?;
            ws.delete_document(&document).await?;
            println!("Document {} supprimé", document);
            Ok(())
        }
        Command::Lsp { root, ws_port } => lsp::run_stdio(&root, ws_port).await,
        Command::Share {
            root,
            relay,
            ws_port,
            exit_with_parent,
        } => run(root, None, relay, ws_port, exit_with_parent, logging.path).await,
        Command::Join {
            invite,
            root,
            relay,
            ws_port,
            exit_with_parent,
            ..
        } => {
            run(
                root,
                Some(Invite::decode(&invite)?),
                relay,
                ws_port,
                exit_with_parent,
                logging.path,
            )
            .await
        }
    }
}

/// Show what joining would do to `root` and ask. Returns whether to go on joining.
async fn review_join(invite: &Invite, root: &Path, relay: bool, preview: bool) -> Result<bool> {
    // A folder tied to another session is refused before anything else, as `join` would.
    if let Ok((capability, _)) = Storage::peek(root) {
        ensure!(
            capability.is_empty() || capability == invite.capability,
            "root_already_belongs_to_another_session"
        );
    }
    let manifest = transport::fetch_manifest(invite, relay).await?;
    let plan = join_plan(&manifest, &Local::scan(root)?);
    if preview {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(false);
    }
    ensure!(
        std::io::stdin().is_terminal(),
        "confirmation_required: relancer avec --preview pour voir les changements, puis --yes pour les accepter"
    );
    print!("{}\nRejoindre la session ? [o/N] ", describe(&plan));
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let accepted = matches!(
        answer.trim().to_lowercase().as_str(),
        "o" | "oui" | "y" | "yes"
    );
    if !accepted {
        println!("Abandon : rien n'a été modifié.");
    }
    Ok(accepted)
}

/// Write a file only its owner can read (it holds the invitation), atomically.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Resolves once standard input reaches its end, which the kernel guarantees when the
/// program holding the other end of the pipe is gone -- closed, crashed or killed.
/// A plain thread: a blocked read must not hold the runtime back at shutdown.
async fn parent_gone(watch: bool) {
    if !watch {
        return std::future::pending().await;
    }
    let (gone, wait) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        let _ = gone.send(());
    });
    let _ = wait.await;
}

async fn run(
    root: PathBuf,
    invite: Option<Invite>,
    relay: bool,
    ws_port: u16,
    exit_with_parent: bool,
    log: Option<PathBuf>,
) -> Result<()> {
    let workspace = Workspace::open(&root).await?;
    let known = workspace.capability().await;
    match &invite {
        Some(invite) => {
            ensure!(
                known.is_empty() || known == invite.capability,
                "root_already_belongs_to_another_session"
            );
            workspace.set_capability(&invite.capability).await?;
        }
        None if known.is_empty() => workspace.set_capability(&transport::new_secret()).await?,
        None => {}
    }

    tracing::info!(
        event = "session opened",
        version = env!("CARGO_PKG_VERSION"),
        documents = workspace.list_documents().await.len(),
    );
    let ep = transport::endpoint(&workspace, relay).await?;
    let mut address = ep.addr();
    if let Some(bound) = ep.bound_sockets().iter().find(|addr| addr.is_ipv4()) {
        let loopback = std::net::SocketAddr::from(([127, 0, 0, 1], bound.port()));
        address.addrs.insert(iroh::TransportAddr::Ip(loopback));
    }

    let invite_str = Invite {
        version: 1,
        address,
        capability: workspace.capability().await,
    }
    .encode()?;

    // Bind before announcing anything: a busy port must fail the launch, not leave a
    // daemon running without its editor interface. Port 0 lets the system choose.
    let listener = TcpListener::bind(("127.0.0.1", ws_port)).await?;
    let ws_port = listener.local_addr()?.port();

    // A closed stdout (a launcher that went away) must not take the daemon down.
    let _ = writeln!(
        std::io::stdout(),
        "Session ouverte\nRacine : {}\nJournal : {}\nCode d'invitation : {}\nWebSocket : ws://127.0.0.1:{}",
        workspace.root.path.display(),
        log.as_ref()
            .map_or("aucun".into(), |p| p.display().to_string()),
        invite_str,
        ws_port
    );

    // Projector & watcher first: the folder is caught up with what happened while no
    // daemon ran before any peer or editor is let in.
    let projector = Projector::spawn(workspace.clone()).await?;

    // 1. P2P Transport task
    let router = transport::run(workspace.clone(), ep, invite.map(|i| i.address)).await?;

    // 2. WebSocket y-sync task
    let ws_task = tokio::spawn(ws::serve_on(workspace.clone(), listener));

    // Editors and plugins find the running session here instead of parsing stdout.
    let session_file = workspace.root.path.join(".collab/session.json");
    write_private(
        &session_file,
        &serde_json::to_vec_pretty(&serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "pid": std::process::id(),
            "root": workspace.root.path,
            "ws_port": ws_port,
            "invite": invite_str,
        }))?,
    )?;

    // Handle signals
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = sigterm.recv() => {},
        _ = parent_gone(exit_with_parent) => tracing::info!(event = "parent_gone"),
    }

    let _ = writeln!(std::io::stdout(), "\nFermeture de la session...");
    let _ = std::fs::remove_file(&session_file);
    router.shutdown().await?;
    ws_task.abort();
    projector.abort();

    Ok(())
}
