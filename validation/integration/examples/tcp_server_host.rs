#![allow(clippy::expect_used)]

use core::time::Duration;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::BitrateBps;
use personal_rns::request_endpoints;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use prns_interfaces_tokio::tcp::TcpServer;

const BITRATE: BitrateBps = BitrateBps::guess(65_000_000);

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4242);
    let bind = std::format!("0.0.0.0:{port}");
    let server = TcpServer::bind_with_bitrate(bind.as_str(), BITRATE)
        .await
        .expect("the TCP server binds");
    std::println!("tcp-server-host: listening on {bind}");

    let me = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "hopspot",
        aspects: &["host"],
        identity: Zeroizing::new([0x33; IDENTITY_SECRET_KEY_LEN]),
        announce_app_data: b"tcp-server-host",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    };
    let my_dest = me
        .destination_hash()
        .expect("the host destination name is valid");

    let seen: Arc<Mutex<HashSet<[u8; 8]>>> = Arc::new(Mutex::new(HashSet::new()));
    let seen_cb = Arc::clone(&seen);
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [me],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                source_interface,
                hops,
                ..
            }) = event
            {
                let id = *source_interface.as_bytes();
                let fresh = seen_cb.lock().map(|mut s| s.insert(id)).unwrap_or(false);
                if fresh {
                    let count = seen_cb.lock().map(|s| s.len()).unwrap_or(0);
                    std::println!(
                        "tcp-server-host: member #{count} up — kind {:?}, id {:02x}{:02x}{:02x}{:02x}, heard a destination at {hops} hop(s)",
                        source_interface.kind(),
                        id[0],
                        id[1],
                        id[2],
                        id[3],
                    );
                }
            }
        },
    });
    let handle = node.handle();
    let _server_sup = handle.supervise(server);

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            if handle
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: my_dest,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    if let Err(error) = node.run().await {
        eprintln!("node stopped: {error}");
    }
}
