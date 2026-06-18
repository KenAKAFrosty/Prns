//! A shared-instance daemon that *bridges*: it holds the local bus (a [`LocalServer`]) for apps on
//! this host and also dials a remote peer over TCP, then transports traffic between the two as a real
//! RNS transport node would. The real-RNS transit smoke stands this up between two stock
//! `RNS.Reticulum` instances, a local client and a TCP peer, and proves a message a local client sends
//! crosses the instance to the peer (and its proof rides home), not just that announces propagate.
//!
//! Ports come from the environment so the smoke harness can pick free ones: `PRNS_LOCAL_PORT` is the
//! loopback shared-instance port apps connect to, `PRNS_PEER_ADDR` is the `host:port` of the remote
//! RNS node's TCP server we dial. Demo/test code, so it `expect`s and `println!`s.
#![allow(clippy::expect_used)]

use core::time::Duration;
use std::string::String;

use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::local::impls::tokio::LocalServer;
use personal_rns::interfaces::rns_parity::tcp::impls::tokio::TcpClientInterface;
use personal_rns::runtime::{Diagnostic, Prns, PrnsEvent, PrnsRecipe};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::TransportId;
use personal_rns::{interfaces, routes};

const BITRATE: u32 = 10_000_000;

fn hex16(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push_str(&std::format!("{byte:02x}"));
    }
    rendered
}

#[tokio::main]
async fn main() {
    let local_port: u16 = std::env::var("PRNS_LOCAL_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(37428);
    let peer_addr = std::env::var("PRNS_PEER_ADDR").expect("PRNS_PEER_ADDR (host:port) is set");

    let secret = Zeroizing::new([0xD1u8; IDENTITY_SECRET_KEY_LEN]);
    let transport_id = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
        TransportId::new(*signer.identity_hash().as_bytes())
    };

    let node = Prns::new(PrnsRecipe {
        transport: Some(transport_id),
        pre_configured_destinations: [] as [personal_rns::runtime::PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: |event, _state: &()| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                hops,
                source_interface,
            }) = event
            {
                println!(
                    "HEARD dest={} hops={} kind={:?}",
                    hex16(destination.as_bytes()),
                    hops,
                    source_interface.kind()
                );
            }
        },
    });
    let handle = node.handle();
    handle.supervise(LocalServer::with_port(local_port));
    let _peer = handle.add_interface(TcpClientInterface::new(
        peer_addr.clone(),
        BITRATE,
        Duration::from_millis(250),
    ));

    println!("READY bridge local=127.0.0.1:{local_port} peer={peer_addr}");
    node.run().await;
}
