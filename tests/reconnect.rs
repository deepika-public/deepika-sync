//! At a distance nothing rediscovers a peer: the daemon itself must call back.
mod common;
use deepika_sync::{transport, workspace::Workspace};
use std::time::Duration;

/// A restarted daemon takes its port back; give the closing socket a moment to free it.
async fn restart(ws: &Workspace) -> iroh::Endpoint {
    for _ in 0..50 {
        if let Ok(ep) = transport::endpoint(ws, false).await {
            return ep;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    transport::endpoint(ws, false)
        .await
        .map_err(|e| format!("{e:#}"))
        .expect("port freed")
}

async fn has(ws: &Workspace, path: &str) -> bool {
    for _ in 0..200 {
        if ws.get_doc_id_by_path(path).await.is_some() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_calls_its_host_back_after_either_side_restarts() {
    let (_host_dir, host) = common::peer().await;
    let (_guest_dir, guest) = common::peer().await;
    for ws in [&host, &guest] {
        ws.set_capability("shared-secret").await.unwrap();
    }

    let host_ep = transport::endpoint(&host, false).await.unwrap();
    let host_addr = host_ep.addr();
    let host_router = transport::run(host.clone(), host_ep, None).await.unwrap();
    let guest_ep = transport::endpoint(&guest, false).await.unwrap();
    let guest_router = transport::run(guest.clone(), guest_ep, Some(host_addr))
        .await
        .unwrap();

    host.create_document("first.md", "one").await.unwrap();
    assert!(has(&guest, "first.md").await, "initial sync");

    // The host goes away and comes back on the same identity and port.
    host_router.shutdown().await.unwrap();
    drop(host_router); // a stopped daemon holds nothing; the router handle holds the endpoint
    let host_ep = restart(&host).await;
    let host_router = transport::run(host.clone(), host_ep, None).await.unwrap();
    host.create_document("after host restart.md", "two")
        .await
        .unwrap();
    assert!(
        has(&guest, "after host restart.md").await,
        "the guest redials a host that came back"
    );

    // The guest restarts as a plain `share`: no invitation, only what it remembers.
    guest_router.shutdown().await.unwrap();
    drop(guest_router);
    host.create_document("while guest was away.md", "three")
        .await
        .unwrap();
    let guest_ep = restart(&guest).await;
    let guest_router = transport::run(guest.clone(), guest_ep, None).await.unwrap();
    assert!(
        has(&guest, "while guest was away.md").await,
        "the guest remembers its host"
    );

    guest_router.shutdown().await.unwrap();
    host_router.shutdown().await.unwrap();
}
