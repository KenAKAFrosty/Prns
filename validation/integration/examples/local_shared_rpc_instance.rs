#![allow(clippy::expect_used)]

use personal_rns::identity::IDENTITY_SECRET_KEY_LEN;
use personal_rns::request_endpoints;
use personal_rns::runtime::NoPersistence;
use personal_rns::runtime::{ManuallyAttached, PrnsNode, PrnsNodeRecipe};
use personal_rns::storage::GrowableHeap;
use prns_interfaces_tokio::shared_instance::rns_rpc::{
    SharedInstanceCredentials, SharedInstanceRpcServer,
};
use prns_interfaces_tokio::shared_instance::SharedInstanceServer;

fn env_port(name: &str, fallback: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(fallback)
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
    let local_port = env_port("PRNS_LOCAL_PORT", 37428);
    let rpc_port = env_port("PRNS_RPC_PORT", local_port + 1);
    let rpc_key = std::env::var("PRNS_RPC_KEY")
        .ok()
        .and_then(|hex| decode_key(&hex))
        .unwrap_or([0x5a; 32]);
    let credentials =
        SharedInstanceCredentials::from_identity_secret(&[0xD2; IDENTITY_SECRET_KEY_LEN])
            .with_rpc_key(rpc_key.to_vec());

    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [] as [personal_rns::runtime::PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    });

    let handle = node.handle();
    handle.supervise(SharedInstanceServer::with_port(local_port));
    let rpc = SharedInstanceRpcServer::tcp(credentials, rpc_port, handle.clone())
        .bind()
        .await
        .expect("the shared-instance RPC listener binds");
    tokio::spawn(rpc.run());

    println!("READY shared-instance bus=127.0.0.1:{local_port} rpc=127.0.0.1:{rpc_port}");
    if let Err(error) = node.run().await {
        eprintln!("node stopped: {error}");
    }
}
