#![allow(clippy::expect_used)]

use personal_rns::runtime::NoPersistence;
use std::string::String;
use std::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::request_endpoints;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, Message, PreConfiguredDestination, PrnsEvent, PrnsNode,
    PrnsNodeRecipe, ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use prns_core::interfaces::shared_instance::DEFAULT_LOCAL_PORT;
use prns_interfaces_tokio::shared_instance::SharedInstanceServer;

const EXPECTED_FROM_STOCK: &[u8] = b"stock-to-prns-shared-server";
const EXPECTED_STOCK_ANNOUNCE: &[u8] = b"stock-shared-client";

enum Observation {
    Announce,
    Delivery,
}

enum WatchOutcome {
    TrafficObserved,
    ObservationsClosed,
}

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
    let destination = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "personal",
        aspects: &["smoke"],
        identity,
        announce_app_data: b"prns-shared-server",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    };
    let own_destination = destination
        .destination_hash()
        .expect("shared-instance destination is valid");
    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
    let delivery_tx = observed_tx.clone();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state: &()| match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                hops,
                source_interface,
                app_data,
            }) => {
                println!(
                    "HEARD dest={} hops={} kind={:?}",
                    hex16(destination.as_bytes()),
                    hops,
                    source_interface.kind()
                );
                if app_data == EXPECTED_STOCK_ANNOUNCE {
                    let _ = observed_tx.send(Observation::Announce);
                }
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery)))
                if delivery.destination == own_destination
                    && delivery.plaintext == EXPECTED_FROM_STOCK =>
            {
                let _ = delivery_tx.send(Observation::Delivery);
            }
            _ => {}
        },
    });
    let handle = node.handle();
    handle.supervise(SharedInstanceServer::with_port(port));
    let announce_handle = handle.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            if announce_handle
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: own_destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });
    println!("READY shared-instance on 127.0.0.1:{port}");
    tokio::select! {
        result = node.run() => {
            if let Err(error) = result {
                eprintln!("node stopped: {error}");
            }
        }
        () = async move {
            let watch = async {
                let mut announcement = false;
                let mut delivery = false;
                while let Some(observation) = observed_rx.recv().await {
                    match observation {
                        Observation::Announce => announcement = true,
                        Observation::Delivery => delivery = true,
                    }
                    if announcement && delivery {
                        return WatchOutcome::TrafficObserved;
                    }
                }
                WatchOutcome::ObservationsClosed
            };
            match tokio::time::timeout(Duration::from_secs(30), watch).await {
                Ok(WatchOutcome::TrafficObserved) => {
                    println!("PRNS_SHARED_SERVER_TRAFFIC_OK bytes={}", EXPECTED_FROM_STOCK.len());
                    std::future::pending::<()>().await
                }
                Ok(WatchOutcome::ObservationsClosed) | Err(_) => {}
            }
        } => {
            eprintln!("node stopped: shared-instance traffic timed out");
        }
    }
}
