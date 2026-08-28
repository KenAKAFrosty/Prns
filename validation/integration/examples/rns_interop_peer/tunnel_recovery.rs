use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, RatchetPolicy, SendSinglePacketFailure,
};
use personal_rns::interfaces::{ConnectionState, InterfaceStatus};
use personal_rns::request_endpoints;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::node_introspection::NodeIntrospection;
use personal_rns::runtime::{
    AnnounceNowError, Diagnostic, ManuallyAttached, Message, NoPersistence,
    PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeRecipe, SendError,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::units::ByteLimit;

use super::common::{
    require_proof, required_environment, secret, ProofFailure, SingleDestination, BITRATE,
    COMPLETION_GRACE, COMPLETION_TIMEOUT,
};

const INITIAL_PAYLOAD: &[u8] = b"tunnel-route-initial";
const RECOVERED_PAYLOAD: &[u8] = b"tunnel-route-recovered";
const STOCK_ANNOUNCE_APP_DATA: &[u8] = b"stock-tunnel-recovery";

enum ClientObservation {
    Initial,
    Recovered,
    Failed(ClientFailure),
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum ClientFailure {
    MissingTarget,
    InvalidDestination,
    Announce(AnnounceNowError),
    UnexpectedDelivery,
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run_client() -> Result<(), ClientFailure> {
    let target =
        required_environment("PRNS_TCP_TARGET").map_err(|_| ClientFailure::MissingTarget)?;
    let destination = SingleDestination {
        app_name: "prns",
        aspects: &["tunnel", "recovery", "client"],
        test_identity_byte: 0xC5,
        announce_app_data: b"prns-tunnel-recovery",
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
        .map_err(|_| ClientFailure::InvalidDestination)?;
    let client = TcpClientInterface::new(target);
    let client_id = client.id();
    let status = client.status();
    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: Some(secret(0x71)),
        pre_configured_destinations: [destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: move |handle: &personal_rns::PrnsNodeHandle| {
            handle.attach(client);
        },
        persistence: NoPersistence,
        on_event: move |event, _state| match event {
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery)))
                if delivery.destination == own_destination
                    && delivery.plaintext == INITIAL_PAYLOAD =>
            {
                let _ = observed_tx.send(ClientObservation::Initial);
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery)))
                if delivery.destination == own_destination
                    && delivery.plaintext == RECOVERED_PAYLOAD =>
            {
                let _ = observed_tx.send(ClientObservation::Recovered);
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery)))
                if delivery.destination == own_destination =>
            {
                let _ =
                    observed_tx.send(ClientObservation::Failed(ClientFailure::UnexpectedDelivery));
            }
            _ => {}
        },
    });
    let handle = node.handle();
    println!("PRNS_TUNNEL_CLIENT_UP");

    let announce = async {
        while status.connection() != ConnectionState::Connected || status.tx_bytes() == 0 {
            tokio::time::sleep(core::time::Duration::from_millis(10)).await;
        }
        handle
            .announce_now(AnnounceNow {
                destination: own_destination,
                target: AnnounceTarget::Interface(client_id),
                app_data: AnnounceAppData::Registered,
            })
            .await
            .map_err(ClientFailure::Announce)?;
        println!("PRNS_TUNNEL_ANNOUNCED count=1");
        Ok(())
    };
    let receive = async {
        match observed_rx.recv().await {
            Some(ClientObservation::Initial) => {
                println!("PRNS_TUNNEL_INITIAL_OK received=1");
            }
            Some(ClientObservation::Failed(error)) => return Err(error),
            Some(ClientObservation::Recovered) => return Err(ClientFailure::UnexpectedDelivery),
            None => return Err(ClientFailure::EventStreamClosed),
        }
        match observed_rx.recv().await {
            Some(ClientObservation::Recovered) => Ok(()),
            Some(ClientObservation::Failed(error)) => Err(error),
            Some(ClientObservation::Initial) => Err(ClientFailure::UnexpectedDelivery),
            None => Err(ClientFailure::EventStreamClosed),
        }
    };
    let completion = async move {
        tokio::try_join!(announce, receive)?;
        println!("PRNS_TUNNEL_RECOVERY_OK received=2 announce_count=1");
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
    MissingRoute,
    Send(SendError<SendSinglePacketFailure>),
    Proof(ProofFailure),
    FreshAnnounce { count: usize },
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run_server() -> Result<(), ServerFailure> {
    let bind = required_environment("PRNS_TCP_BIND").map_err(|_| ServerFailure::MissingBind)?;
    let server = TcpServer::bind_with_bitrate(bind, BITRATE)
        .await
        .map_err(ServerFailure::Bind)?;
    let announce_count = Arc::new(AtomicUsize::new(0));
    let announce_count_for_on_event = announce_count.clone();
    let (destination_tx, mut destination_rx) = tokio::sync::mpsc::unbounded_channel();
    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: Some(secret(0x72)),
        pre_configured_destinations: [] as [PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                app_data,
                ..
            }) = event
            {
                if app_data == STOCK_ANNOUNCE_APP_DATA {
                    announce_count_for_on_event.fetch_add(1, Ordering::Relaxed);
                    let _ = destination_tx.send(destination);
                }
            }
        },
    });
    let handle = node.handle();
    let _server = handle.supervise(server);
    println!("PRNS_TUNNEL_RELAY_UP");

    let completion = async move {
        let destination = destination_rx
            .recv()
            .await
            .ok_or(ServerFailure::EventStreamClosed)?;
        if announce_count.load(Ordering::Relaxed) != 1 {
            return Err(ServerFailure::FreshAnnounce {
                count: announce_count.load(Ordering::Relaxed),
            });
        }
        let initial_route = handle
            .route(destination)
            .await
            .ok_or(ServerFailure::MissingRoute)?;
        let initial_receipt = handle
            .send_single_packet(destination, INITIAL_PAYLOAD)
            .await
            .map_err(ServerFailure::Send)?;
        require_proof(initial_receipt).map_err(ServerFailure::Proof)?;
        println!("PRNS_TUNNEL_RELAY_INITIAL_OK proof=1 announce_count=1");

        loop {
            let count = announce_count.load(Ordering::Relaxed);
            if count != 1 {
                return Err(ServerFailure::FreshAnnounce { count });
            }
            if let Some(route) = handle.route(destination).await {
                if route.interface != initial_route.interface {
                    break;
                }
            }
            tokio::time::sleep(core::time::Duration::from_millis(50)).await;
        }

        let recovered_receipt = handle
            .send_single_packet(destination, RECOVERED_PAYLOAD)
            .await
            .map_err(ServerFailure::Send)?;
        require_proof(recovered_receipt).map_err(ServerFailure::Proof)?;
        tokio::time::sleep(COMPLETION_GRACE).await;
        let count = announce_count.load(Ordering::Relaxed);
        if count != 1 {
            return Err(ServerFailure::FreshAnnounce { count });
        }
        println!("PRNS_TUNNEL_RELAY_OK proof=2 announce_count=1 route_repointed=1");
        Ok(())
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
