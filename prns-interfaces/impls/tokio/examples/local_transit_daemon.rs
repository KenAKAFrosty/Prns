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

use std::string::String;

use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::ifac::{IfacContext, IfacSize};
use personal_rns::interfaces::BitrateBps;
use personal_rns::routes;
use personal_rns::runtime::{Diagnostic, Manual, PrnsEvent, PrnsNode, PrnsNodeRecipe};
use personal_rns::storage::GrowableHeap;
use prns_interfaces_tokio::reconnect::ReconnectPolicy;
use prns_interfaces_tokio::shared_instance::rpc_compat::{
    SharedInstanceCredentials, SharedInstanceRpcCompat,
};
use prns_interfaces_tokio::shared_instance::server::LocalServer;
use prns_interfaces_tokio::tcp::client::TcpClientInterface;

const BITRATE: BitrateBps = BitrateBps::guess(10_000_000);

fn hex16(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push_str(&std::format!("{byte:02x}"));
    }
    rendered
}

fn decode_key(hex: &str) -> Option<[u8; 32]> {
    let trimmed = hex.trim();
    if trimmed.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, slot) in key.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&trimmed[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

#[tokio::main]
async fn main() {
    let local_port: u16 = std::env::var("PRNS_LOCAL_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(37428);
    let peer_addr = std::env::var("PRNS_PEER_ADDR").expect("PRNS_PEER_ADDR (host:port) is set");
    let rpc_port: u16 = std::env::var("PRNS_RPC_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(local_port + 1);
    let rpc_key: [u8; 32] = std::env::var("PRNS_RPC_KEY")
        .ok()
        .and_then(|hex| decode_key(&hex))
        .unwrap_or([0x5a; 32]);
    let ifac_network_name = std::env::var("PRNS_IFAC_NETWORK_NAME")
        .ok()
        .filter(|value| !value.is_empty());
    let ifac_passphrase = std::env::var("PRNS_IFAC_PASSPHRASE")
        .ok()
        .filter(|value| !value.is_empty());
    let ifac_size = std::env::var("PRNS_IFAC_SIZE_BYTES")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .map(IfacSize::new)
        .transpose()
        .expect("PRNS_IFAC_SIZE_BYTES is between 1 and 64")
        .unwrap_or(IfacSize::WIDE);
    let ifac = IfacContext::derive(
        ifac_network_name.as_deref(),
        ifac_passphrase.as_deref(),
        ifac_size,
    );

    let secret = Zeroizing::new([0xD1u8; IDENTITY_SECRET_KEY_LEN]);
    let credentials =
        SharedInstanceCredentials::from_identity_secret(&secret).with_rpc_key(rpc_key.to_vec());

    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(secret),
        pre_configured_destinations: [] as [personal_rns::runtime::PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
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
    let tcp = TcpClientInterface::new(peer_addr.clone(), BITRATE, ReconnectPolicy::STANDARD);
    let _peer = match ifac {
        Some(ifac) => handle.add_interface_with_ifac_name(tcp, ifac, ifac_network_name),
        None => handle.add_interface(tcp),
    };

    // The control-RPC compatibility shim: stock RNS clients fetch per-packet phy stats over this
    // channel during attachment (resource) delivery, and fault if nobody answers. It reads engine
    // state (e.g. link_count) through the handle to answer with real values.
    let rpc = SharedInstanceRpcCompat::tcp(credentials, rpc_port, handle.clone())
        .bind()
        .await
        .expect("the shared-instance RPC listener binds");
    tokio::spawn(rpc.run());

    println!("READY bridge local=127.0.0.1:{local_port} rpc=127.0.0.1:{rpc_port} peer={peer_addr}");
    node.run().await;
}
