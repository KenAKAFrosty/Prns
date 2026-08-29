use personal_rns::engine::RatchetPolicy;
use personal_rns::request_endpoints;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, NoPersistence, PrnsEvent, PrnsNode, PrnsNodeRecipe, ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpClientInterface;
use personal_rns::units::ByteLimit;

use super::common::{
    required_environment, spawn_announces, SingleDestination, COMPLETION_GRACE, COMPLETION_TIMEOUT,
};

const EXPECTED_FROM_STOCK: &[u8] = &[0xFF, 0x73, 0x74, 0x6F, 0x63, 0x6B, 0x00];
const SENT_FROM_PRNS: &[u8] = &[0x00, 0x70, 0x72, 0x6E, 0x73, 0xFF];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Observation {
    Expected,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    MissingTarget,
    InvalidDestination,
    UnexpectedApplicationData,
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run() -> Result<(), Failure> {
    let target = required_environment("PRNS_TCP_TARGET").map_err(|_| Failure::MissingTarget)?;
    let destination = SingleDestination {
        app_name: "prns",
        aspects: &["announce", "appdata", "interop"],
        test_identity_byte: 0xA1,
        announce_app_data: SENT_FROM_PRNS,
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: ByteLimit::Unlimited,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
    .into_preconfigured();
    let destination_hash = destination
        .destination_hash()
        .map_err(|_| Failure::InvalidDestination)?;
    let client = TcpClientInterface::new(target);
    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: move |handle: &personal_rns::PrnsNodeHandle| {
            handle.attach(client);
        },
        persistence: NoPersistence,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { app_data, .. }) = event {
                let observation = if app_data == EXPECTED_FROM_STOCK {
                    Observation::Expected
                } else {
                    Observation::Unexpected
                };
                let _ = observed_tx.send(observation);
            }
        },
    });
    spawn_announces(node.handle(), destination_hash);
    println!("PRNS_ANNOUNCE_APP_DATA_PEER_UP");

    let completion = async move {
        match observed_rx.recv().await {
            Some(Observation::Expected) => {
                println!("PRNS_ANNOUNCE_APP_DATA_OK received=1");
                tokio::time::sleep(COMPLETION_GRACE).await;
                Ok(())
            }
            Some(Observation::Unexpected) => Err(Failure::UnexpectedApplicationData),
            None => Err(Failure::EventStreamClosed),
        }
    };
    tokio::select! {
        result = node.run() => {
            let _ = result;
            Err(Failure::NodeStopped)
        }
        result = tokio::time::timeout(COMPLETION_TIMEOUT, completion) => {
            result.map_err(|_| Failure::Timeout)?
        }
    }
}
