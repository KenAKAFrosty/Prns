use personal_rns::engine::RatchetPolicy;
use personal_rns::interfaces::{IfacContext, IfacSize, IfacSizeError};
use personal_rns::request_endpoints;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, Message, NoPersistence, PrnsEvent, PrnsNode, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpServer;
use personal_rns::units::ByteLimit;

use super::common::{
    required_environment, spawn_announces, SingleDestination, BITRATE, COMPLETION_GRACE,
    COMPLETION_TIMEOUT,
};

const BEFORE_PAYLOAD: &[u8] = b"ifac-matching-before";
const AFTER_PAYLOAD: &[u8] = b"ifac-matching-after";

enum Observation {
    Before,
    After,
    Failed(Failure),
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum Failure {
    MissingBind,
    MissingNetworkName,
    MissingPassphrase,
    MissingSize,
    InvalidSizeNumber(std::num::ParseIntError),
    InvalidSize(IfacSizeError),
    MissingIfac,
    Bind(std::io::Error),
    InvalidDestination,
    AuthenticationBypass,
    UnexpectedDelivery,
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run_server() -> Result<(), Failure> {
    let bind = required_environment("PRNS_IFAC_BIND").map_err(|_| Failure::MissingBind)?;
    let network_name =
        required_environment("PRNS_IFAC_NETWORK_NAME").map_err(|_| Failure::MissingNetworkName)?;
    let passphrase =
        required_environment("PRNS_IFAC_PASSPHRASE").map_err(|_| Failure::MissingPassphrase)?;
    let size = required_environment("PRNS_IFAC_SIZE_BYTES")
        .map_err(|_| Failure::MissingSize)?
        .parse::<usize>()
        .map_err(Failure::InvalidSizeNumber)
        .and_then(|bytes| IfacSize::new(bytes).map_err(Failure::InvalidSize))?;
    let ifac = IfacContext::derive(Some(&network_name), Some(&passphrase), size)
        .ok_or(Failure::MissingIfac)?;
    let server = TcpServer::bind_with_bitrate(bind, BITRATE)
        .await
        .map_err(Failure::Bind)?;
    let destination = SingleDestination {
        app_name: "prns",
        aspects: &["ifac", "server"],
        test_identity_byte: 0xB4,
        announce_app_data: b"prns-ifac-server",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: ByteLimit::Unlimited,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
    .into_preconfigured();
    let own_destination = destination
        .destination_hash()
        .map_err(|_| Failure::InvalidDestination)?;
    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state| match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { app_data, .. })
                if app_data == b"missing" || app_data == b"wrong" =>
            {
                let _ = observed_tx.send(Observation::Failed(Failure::AuthenticationBypass));
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery)))
                if delivery.destination == own_destination
                    && delivery.plaintext == BEFORE_PAYLOAD =>
            {
                let _ = observed_tx.send(Observation::Before);
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery)))
                if delivery.destination == own_destination
                    && delivery.plaintext == AFTER_PAYLOAD =>
            {
                let _ = observed_tx.send(Observation::After);
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Single(_))) => {
                let _ = observed_tx.send(Observation::Failed(Failure::UnexpectedDelivery));
            }
            _ => {}
        },
    });
    let handle = node.handle();
    handle.supervise_with_ifac_name(server, ifac, Some(network_name));
    spawn_announces(handle, own_destination);
    println!("PRNS_IFAC_SERVER_UP");

    let completion = async move {
        let mut before = false;
        let mut after = false;
        while let Some(observation) = observed_rx.recv().await {
            match observation {
                Observation::Before => {
                    before = true;
                    println!("PRNS_IFAC_MATCHING_OK phase=before");
                }
                Observation::After => {
                    after = true;
                    println!("PRNS_IFAC_MATCHING_OK phase=after");
                }
                Observation::Failed(error) => return Err(error),
            }
            if before && after {
                tokio::time::sleep(COMPLETION_GRACE).await;
                return Ok(());
            }
        }
        Err(Failure::EventStreamClosed)
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
