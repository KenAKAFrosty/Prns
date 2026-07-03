//! The Prns side of a shared-instance Matchup: a transport node that holds the local bus and
//! nothing else, so two app clients that each connect over the loopback shared-instance port meet
//! through our engine. Transport is enabled because the host's whole job here is to forward a
//! sender client's traffic to the receiver client's interface (the local-client to local-client
//! path), not just propagate announces. The `ref-client` Matchup cells stand stock `RNS.Reticulum`
//! apps on each end of this; the `us-client` cells will stand Prns apps once the outbound connector
//! lands. Ports come from the environment so the runner can pick free ones.
//!
//! Test/harness code, so it `expect`s and `println!`s rather than threading errors.
#![allow(clippy::expect_used)]

use std::string::String;

use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::runtime::{Diagnostic, Prns, PrnsEvent, PrnsRecipe};
use personal_rns::shared_instance::rpc_compat::SharedInstanceRpcCompat;
use personal_rns::shared_instance::server::LocalServer;
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::TransportId;
use personal_rns::{interfaces, routes};

fn hex16(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push_str(&std::format!("{byte:02x}"));
    }
    rendered
}

#[tokio::main]
async fn main() {
    let local_port: u16 = std::env::var("MATCHUP_LOCAL_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(37428);

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
    tokio::spawn(SharedInstanceRpcCompat::tcp([0x5a; 32], local_port + 1, handle.clone()).run());
    println!(
        "READY host local=127.0.0.1:{local_port} rpc=127.0.0.1:{}",
        local_port + 1
    );
    node.run().await;
}
