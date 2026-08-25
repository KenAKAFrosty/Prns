use personal_rns::engine::{RatchetPolicy, SendSinglePacketFailure};
use personal_rns::request_endpoints;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, Message, NoPersistence, PreConfiguredDestination, PrnsEvent,
    PrnsNode, PrnsNodeRecipe, SendError, ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::units::ByteLimit;

use super::common::{
    require_proof, required_environment, spawn_announces, ProofFailure, SingleDestination, BITRATE,
    COMPLETION_GRACE, COMPLETION_TIMEOUT,
};

const CLIENT_PAYLOAD: &[u8] = b"prns-tcp-parity-ping";
const STOCK_CLIENT_PAYLOAD: &[u8] = b"prns-tcp-parity-ping";

#[allow(dead_code)]
#[derive(Debug)]
pub enum ClientFailure {
    MissingTarget,
    Send(SendError<SendSinglePacketFailure>),
    Proof(ProofFailure),
    AnnounceStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run_client() -> Result<(), ClientFailure> {
    let target =
        required_environment("PRNS_TCP_TARGET").map_err(|_| ClientFailure::MissingTarget)?;
    let client = TcpClientInterface::new(target);
    let (destination_tx, mut destination_rx) = tokio::sync::mpsc::unbounded_channel();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: move |handle: &personal_rns::PrnsNodeHandle| {
            handle.attach(client);
        },
        persistence: NoPersistence,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = destination_tx.send(destination);
            }
        },
    });
    let handle = node.handle();
    println!("PRNS_TCP_CLIENT_UP");
    let completion = async move {
        let destination = destination_rx
            .recv()
            .await
            .ok_or(ClientFailure::AnnounceStreamClosed)?;
        let receipt = handle
            .send_single_packet(destination, CLIENT_PAYLOAD)
            .await
            .map_err(ClientFailure::Send)?;
        require_proof(receipt).map_err(ClientFailure::Proof)?;
        println!("PRNS_TCP_CLIENT_OK proof=1");
        tokio::time::sleep(COMPLETION_GRACE).await;
        Ok(())
    };
    tokio::select! {
        result = node.run() => {
            let _ = result;
            Err(ClientFailure::NodeStopped)
        }
        result = tokio::time::timeout(COMPLETION_TIMEOUT, completion) => {
            result.map_err(|_| ClientFailure::Timeout)?
        }
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum ServerFailure {
    MissingBind,
    Bind(std::io::Error),
    InvalidDestination,
    UnexpectedDelivery,
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run_server() -> Result<(), ServerFailure> {
    let bind = required_environment("PRNS_TCP_BIND").map_err(|_| ServerFailure::MissingBind)?;
    let server = TcpServer::bind_with_bitrate(bind, BITRATE)
        .await
        .map_err(ServerFailure::Bind)?;
    let destination = SingleDestination {
        app_name: "hopspot",
        aspects: &["host"],
        test_identity_byte: 0x33,
        announce_app_data: b"tcp-server-host",
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
        .map_err(|_| ServerFailure::InvalidDestination)?;
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
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery)))
                if delivery.destination == own_destination
                    && delivery.plaintext == STOCK_CLIENT_PAYLOAD =>
            {
                let _ = observed_tx.send(Ok(()));
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Single(_))) => {
                let _ = observed_tx.send(Err(ServerFailure::UnexpectedDelivery));
            }
            _ => {}
        },
    });
    let handle = node.handle();
    let _server = handle.supervise(server);
    spawn_announces(handle, own_destination);
    println!("PRNS_TCP_SERVER_UP");
    let completion = async move {
        match observed_rx.recv().await {
            Some(Ok(())) => {
                println!("PRNS_TCP_SERVER_OK received=1");
                tokio::time::sleep(COMPLETION_GRACE).await;
                Ok(())
            }
            Some(Err(error)) => Err(error),
            None => Err(ServerFailure::EventStreamClosed),
        }
    };
    tokio::select! {
        result = node.run() => {
            let _ = result;
            Err(ServerFailure::NodeStopped)
        }
        result = tokio::time::timeout(COMPLETION_TIMEOUT, completion) => {
            result.map_err(|_| ServerFailure::Timeout)?
        }
    }
}
