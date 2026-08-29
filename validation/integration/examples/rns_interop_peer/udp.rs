use personal_rns::engine::{RatchetPolicy, SendSinglePacketFailure};
use personal_rns::interfaces::udp::UDP_BITRATE_ESTIMATE;
use personal_rns::request_endpoints;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, Message, NoPersistence, PrnsEvent, PrnsNode, PrnsNodeRecipe, SendError,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::udp::UdpInterface;
use personal_rns::units::ByteLimit;

use super::common::{
    require_proof, required_environment, spawn_announces, ProofFailure, SingleDestination,
    COMPLETION_GRACE, COMPLETION_TIMEOUT,
};

const EXPECTED_FROM_STOCK: &[u8] = b"stock-udp-proof";
const SENT_FROM_PRNS: &[u8] = b"prns-udp-proof";

enum Observation {
    Received,
    OutboundProven,
    Failed(Failure),
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum Failure {
    MissingLocal,
    MissingPeer,
    Bind(std::io::Error),
    InvalidDestination,
    Send(SendError<SendSinglePacketFailure>),
    Proof(ProofFailure),
    UnexpectedDelivery,
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run() -> Result<(), Failure> {
    let local = required_environment("PRNS_UDP_LOCAL").map_err(|_| Failure::MissingLocal)?;
    let peer = required_environment("PRNS_UDP_PEER").map_err(|_| Failure::MissingPeer)?;
    let interface = UdpInterface::bind(local, peer, UDP_BITRATE_ESTIMATE)
        .await
        .map_err(Failure::Bind)?;
    let destination = SingleDestination {
        app_name: "prns",
        aspects: &["udp", "client"],
        test_identity_byte: 0xA3,
        announce_app_data: b"",
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
    let (destination_tx, mut destination_rx) = tokio::sync::mpsc::unbounded_channel();
    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_tx = observed_tx.clone();
    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: move |handle: &personal_rns::PrnsNodeHandle| {
            handle.attach(interface);
        },
        persistence: NoPersistence,
        on_event: move |event, _state| match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                let _ = destination_tx.send(destination);
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery)))
                if delivery.destination == own_destination
                    && delivery.plaintext == EXPECTED_FROM_STOCK =>
            {
                let _ = event_tx.send(Observation::Received);
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Single(_))) => {
                let _ = event_tx.send(Observation::Failed(Failure::UnexpectedDelivery));
            }
            _ => {}
        },
    });
    let handle = node.handle();
    spawn_announces(handle.clone(), own_destination);
    tokio::spawn(async move {
        let Some(stock_destination) = destination_rx.recv().await else {
            let _ = observed_tx.send(Observation::Failed(Failure::EventStreamClosed));
            return;
        };
        let receipt = match handle
            .send_single_packet(stock_destination, SENT_FROM_PRNS)
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                let _ = observed_tx.send(Observation::Failed(Failure::Send(error)));
                return;
            }
        };
        if let Err(error) = require_proof(receipt) {
            let _ = observed_tx.send(Observation::Failed(Failure::Proof(error)));
            return;
        }
        let _ = observed_tx.send(Observation::OutboundProven);
    });
    println!("PRNS_UDP_CLIENT_UP");

    let completion = async move {
        let mut received = false;
        let mut outbound_proven = false;
        while let Some(observation) = observed_rx.recv().await {
            match observation {
                Observation::Received => received = true,
                Observation::OutboundProven => outbound_proven = true,
                Observation::Failed(error) => return Err(error),
            }
            if received && outbound_proven {
                println!("PRNS_UDP_OK received=1 proven=1");
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
