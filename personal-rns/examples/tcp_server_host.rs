//! A standalone TCP-server host for live multi-client testing: binds a `TcpServer` on `0.0.0.0:PORT`
//! and logs every distinct member (a spawned `TcpServerPeer`) it hears an announce from — so dialing
//! it from several clients (an S3 board, a phone, a loopback node) shows each land as its own
//! interface, the live form of the multi-client integration test. It announces a destination of its
//! own on a cadence so the dialers can find it too. Demo code: it `expect`s on setup.
//!
//! Run: `cargo run --release --example tcp_server_host --features tcp` (set `PORT` to override 4242).
//! Then point any dialer at `this-host-ip:PORT` — e.g.
//! `HOPSPOT_TCP_TARGET=127.0.0.1:4242 cargo run -p hopspot`
//! for the loopback node, or the Heltec V4 firmware's `HOPSPOT_TCP_TARGET` for the board.

#![cfg(feature = "tcp")]
#![allow(clippy::expect_used)]

use core::time::Duration;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::tcp::server::tokio::TcpServer;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{Diagnostic, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe};
use personal_rns::storage::GrowableHeap;
use personal_rns::{interfaces, routes};

/// The host's honest order-of-magnitude pipe to a LAN client — sets the members' declared MTU tier.
const BITRATE: u32 = 65_000_000;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4242);
    let bind = std::format!("0.0.0.0:{port}");
    let server = TcpServer::bind(bind.as_str(), BITRATE)
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
        ratchet: RatchetPolicy::NoRatchets,
    };
    let my_dest = me
        .destination_hash()
        .expect("the host destination name is valid");

    // Log each member the first time we hear it, so two dialers print as two distinct interfaces.
    let seen: Arc<Mutex<HashSet<[u8; 8]>>> = Arc::new(Mutex::new(HashSet::new()));
    let seen_cb = Arc::clone(&seen);
    let node = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [me],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
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

    // Announce ourselves so the dialers can find us, too.
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

    node.run().await;
}
