#![allow(clippy::expect_used)]

use personal_rns::engine::{EngineProtocolPolicy, RecursivePathRequestDefault};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::BitrateBps;
use personal_rns::request_endpoints;
use personal_rns::runtime::{ManuallyAttached, NoPersistence, PrnsNode, PrnsNodeRecipe};
use personal_rns::storage::GrowableHeap;
use prns_interfaces_tokio::reconnect::ReconnectPolicy;
use prns_interfaces_tokio::tcp::{TcpClientInterface, TcpServer};

const BITRATE: BitrateBps = BitrateBps::guess(10_000_000);

#[tokio::main]
async fn main() {
    let listen_port: u16 = std::env::var("PRNS_MULTIHOP_LISTEN_PORT")
        .expect("PRNS_MULTIHOP_LISTEN_PORT is set")
        .parse()
        .expect("PRNS_MULTIHOP_LISTEN_PORT is a port");
    let peer = std::env::var("PRNS_MULTIHOP_PEER").expect("PRNS_MULTIHOP_PEER is set");
    let alternate_peer = std::env::var("PRNS_MULTIHOP_ALTERNATE_PEER").ok();
    let bind = std::format!("127.0.0.1:{listen_port}");
    let server = TcpServer::bind_with_bitrate(&bind, BITRATE)
        .await
        .expect("the mixed multi-hop TCP server binds");
    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: Some(Zeroizing::new([0xD2; IDENTITY_SECRET_KEY_LEN])),
        pre_configured_destinations: [] as [personal_rns::runtime::PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_, _state: &()| {},
    })
    .with_protocol_policy(EngineProtocolPolicy {
        recursive_path_request_default: RecursivePathRequestDefault::Enabled,
        ..Default::default()
    });
    let handle = node.handle();
    let _server = handle.supervise(server);
    let _client = handle.add_interface(TcpClientInterface::new_with_bitrate(
        peer.clone(),
        BITRATE,
        ReconnectPolicy::STANDARD,
    ));
    let _alternate_client = alternate_peer.as_ref().map(|alternate| {
        handle.add_interface(TcpClientInterface::new_with_bitrate(
            alternate.clone(),
            BITRATE,
            ReconnectPolicy::STANDARD,
        ))
    });
    println!(
        "MIXED_MULTIHOP_READY listen={bind} peer={peer} alternate={}",
        alternate_peer.as_deref().unwrap_or("none")
    );
    if let Err(error) = node.run().await {
        eprintln!("mixed multi-hop node stopped: {error}");
    }
}
