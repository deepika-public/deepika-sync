//! Zero-conf discovery of the session's peers on the local network, over mDNS.
//!
//! The service name is derived from the capability, so daemons of other sessions
//! never even see each other; the hash reveals nothing usable about the secret.

use futures_util::StreamExt;
use iroh::{Endpoint, EndpointAddr};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use tokio::sync::mpsc;

/// mDNS service labels hold 15 bytes at most: `sp-` plus 12 hex digits.
pub fn service_name(capability: &str) -> String {
    format!("sp-{}", &blake3::hash(capability.as_bytes()).to_hex()[..12])
}

/// Advertise this endpoint and send every peer found to `dial`. Discovery is a
/// convenience: failing to start it is logged, never fatal.
pub fn start(ep: &Endpoint, capability: &str, dial: mpsc::Sender<EndpointAddr>) {
    let mdns = match MdnsAddressLookup::builder()
        .service_name(service_name(capability))
        .build(ep.id())
    {
        Ok(mdns) => mdns,
        Err(e) => return tracing::warn!(error = %e, "mdns_discovery_unavailable"),
    };
    match ep.address_lookup() {
        Ok(services) => services.add(mdns.clone()),
        Err(e) => return tracing::warn!(error = %e, "mdns_discovery_unavailable"),
    }
    let closed = ep.closed();
    tokio::spawn(async move {
        let mut events = mdns.subscribe().await.take_until(Box::pin(closed));
        while let Some(event) = events.next().await {
            if let DiscoveryEvent::Discovered { endpoint_info, .. } = event {
                // mDNS re-announces known peers every few seconds; the dialer skips those.
                tracing::debug!(peer = %endpoint_info.endpoint_id, "mdns_peer_discovered");
                if dial.send(endpoint_info.into_endpoint_addr()).await.is_err() {
                    break;
                }
            }
        }
    });
}
