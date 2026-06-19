//! A headless WiFi/LAN auto-interface node for live two-on-one-host testing of the TCP rendezvous
//! fold: it supervises `AutoWifi`, announces itself on a cadence, and logs every announce it hears
//! tagged with the *kind* of interface it arrived over. Two instances on one host show whether they
//! bridge over the loopback rendezvous (`TcpServerPeer` on the port holder, `TcpClient` on the one
//! that bounced) as designed — co-hosted nodes share a NIC, so a shared link-local means a shared
//! peering token and they self-ignore each other's multicast, leaving the loopback link their only
//! path. A 5s interface roll-call shows the rendezvous member appear. Demo code: it `expect`s.
//!
//! Run two, then kill the first and watch the second reclaim `:42699`:
//!   NODE=11 NAME=A cargo run --release --example wifi_auto_node --features wifi-lan-auto
//!   NODE=22 NAME=B cargo run --release --example wifi_auto_node --features wifi-lan-auto
//!
//! Set `TRANSPORT=1` to make a node a transport core, so co-hosted peers that only reach it over the
//! loopback rendezvous still bridge to each other *through* it — a third node then hears the others
//! at two hops, the star the design assumes the port holder becomes.

#![cfg(feature = "wifi-lan-auto")]
#![allow(clippy::expect_used)]

use core::time::Duration;
use std::string::String;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::wifi_auto::AutoWifi;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{Diagnostic, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::TransportId;
use personal_rns::{interfaces, routes};

const BITRATE: u32 = 65_000_000;

#[tokio::main]
async fn main() {
    let node_byte = std::env::var("NODE")
        .ok()
        .and_then(|v| u8::from_str_radix(v.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x11);
    let name = std::env::var("NAME").unwrap_or_else(|_| std::format!("{node_byte:02x}"));

    let secret = Zeroizing::new([node_byte; IDENTITY_SECRET_KEY_LEN]);
    let transport = std::env::var("TRANSPORT").is_ok().then(|| {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
        TransportId::new(*signer.identity_hash().as_bytes())
    });
    let me = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "hopspot",
        aspects: &["node"],
        identity: secret,
        announce_app_data: b"wifi-auto-node",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    };
    let my_dest = me.destination_hash().expect("destination name is valid");

    let heard_name = name.clone();
    let node = Prns::new(PrnsRecipe {
        transport,
        pre_configured_destinations: [me],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                source_interface,
                hops,
                destination,
            }) = event
            {
                std::println!(
                    "[{heard_name}] HEARD dest={:02x}{:02x} via {:?} ({hops} hop)",
                    destination.as_bytes()[0],
                    destination.as_bytes()[1],
                    source_interface.kind(),
                );
            }
        },
    });
    let handle = node.handle();
    let _wifi = handle.supervise(AutoWifi::with_bitrate(BITRATE));
    std::println!(
        "[{name}] up — dest {:02x}{:02x}, transport={}, supervising auto-wifi",
        my_dest.as_bytes()[0],
        my_dest.as_bytes()[1],
        transport.is_some(),
    );

    let announce = handle.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            if announce
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

    let roll_call = handle.clone();
    let roll_name = name.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            let summary: std::vec::Vec<String> = roll_call
                .interface_snapshots()
                .iter()
                .map(|snap| std::format!("{:?}/{:?}", snap.id.kind(), snap.connection))
                .collect();
            std::println!("[{roll_name}] interfaces: {summary:?}");
        }
    });

    node.run().await;
}
