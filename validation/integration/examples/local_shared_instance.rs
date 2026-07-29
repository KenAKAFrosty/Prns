#![allow(clippy::expect_used)]

use personal_rns::runtime::NoPersistence;
use std::string::String;

use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::request_endpoints;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use prns_core::interfaces::shared_instance::DEFAULT_LOCAL_PORT;
use prns_interfaces_tokio::shared_instance::SharedInstanceServer;

fn hex16(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push_str(&std::format!("{byte:02x}"));
    }
    rendered
}

#[tokio::main]
async fn main() {
    let port = std::env::var("PRNS_LOCAL_PORT").map_or(DEFAULT_LOCAL_PORT, |value| {
        value.parse().expect("shared-instance port is a u16")
    });
    let identity = Zeroizing::new([0x5au8; IDENTITY_SECRET_KEY_LEN]);
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "personal",
            aspects: &["smoke"],
            identity,
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            request_endpoints: ServeMyRequestEndpoints::No,
        }],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
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
    handle.supervise(SharedInstanceServer::with_port(port));
    println!("READY shared-instance on 127.0.0.1:{port}");
    if let Err(error) = node.run().await {
        eprintln!("node stopped: {error}");
    }
}
