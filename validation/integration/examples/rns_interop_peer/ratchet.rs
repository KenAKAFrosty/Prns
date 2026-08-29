use personal_rns::engine::{RatchetPolicy, SendSinglePacketFailure};
use personal_rns::request_endpoints;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, Message, NoPersistence, PrnsEvent, PrnsNode, PrnsNodeRecipe, SendError,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpClientInterface;
use personal_rns::units::ByteLimit;

use super::common::{
    require_proof, required_environment, spawn_announces, ProofFailure, SingleDestination,
    COMPLETION_GRACE, COMPLETION_TIMEOUT,
};

const SENT_FROM_PRNS: [&[u8]; 2] = [b"prns-ratchet-zero", b"prns-ratchet-one"];
const EXPECTED_FROM_STOCK: &[u8] = b"stock-ratchet-proof";

enum Observation {
    Received,
    OutboundProven,
    Failed(Failure),
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum Failure {
    MissingTarget,
    InvalidDestination,
    Send(SendError<SendSinglePacketFailure>),
    Proof(ProofFailure),
    UnexpectedDelivery,
    TooManyAnnounces,
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run() -> Result<(), Failure> {
    let target = required_environment("PRNS_TCP_TARGET").map_err(|_| Failure::MissingTarget)?;
    let destination = SingleDestination {
        app_name: "prns",
        aspects: &["ratchet", "client"],
        test_identity_byte: 0xA4,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::RatchetsRequired,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: ByteLimit::Unlimited,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
    .into_preconfigured();
    let own_destination = destination
        .destination_hash()
        .map_err(|_| Failure::InvalidDestination)?;
    let client = TcpClientInterface::new(target);
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
            handle.attach(client);
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
        for payload in SENT_FROM_PRNS {
            let Some(stock_destination) = destination_rx.recv().await else {
                let _ = observed_tx.send(Observation::Failed(Failure::EventStreamClosed));
                return;
            };
            let receipt = match handle.send_single_packet(stock_destination, payload).await {
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
        }
        if destination_rx.try_recv().is_ok() {
            let _ = observed_tx.send(Observation::Failed(Failure::TooManyAnnounces));
        }
    });
    println!("PRNS_RATCHET_CLIENT_UP");

    let completion = async move {
        let mut received = false;
        let mut proven = 0;
        while let Some(observation) = observed_rx.recv().await {
            match observation {
                Observation::Received => received = true,
                Observation::OutboundProven => proven += 1,
                Observation::Failed(error) => return Err(error),
            }
            if received && proven == SENT_FROM_PRNS.len() {
                println!("PRNS_RATCHET_OK sent=2 received=1 proven=2");
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
