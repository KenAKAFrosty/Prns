//! A minimal shared-instance daemon for the real-RNS interop smoke test: a `PrnsNode` that supervises a [`LocalServer`] and prints every announce it hears. The smoke harness stands this up, then drives a stock `RNS.Reticulum` instance (the reference venv) that connects over the loopback shared-instance port and announces, proving a real RNS client interoperates with our server. Demo/test code, so it uses `expect` and `println!` rather than threading errors.
#![allow(clippy::expect_used)]

use std::string::String;

use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routes;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, Manual, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeRecipe,
};
use personal_rns::storage::GrowableHeap;
use prns_interfaces_tokio::shared_instance::server::LocalServer;

fn hex16(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push_str(&std::format!("{byte:02x}"));
    }
    rendered
}

#[tokio::main]
async fn main() {
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
        }],
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
    handle.supervise(LocalServer::new());
    println!("READY shared-instance on 127.0.0.1:37428");
    node.run().await;
}
