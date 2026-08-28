use personal_rns::engine::{
    EstablishLinkFailure, RatchetPolicy, SendRequestFailure, SendResourceFailure,
    SendToChannelFailure,
};
use personal_rns::request_endpoints;
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::links::request::PackBinaryError;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, Message, NoPersistence, PreConfiguredDestination, PrnsEvent,
    PrnsNode, PrnsNodeRecipe, ResourceSendError, SegmentCompression, SendError,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::units::ByteLimit;

use super::common::{
    messagepack_binary, require_proof, required_environment, spawn_announces, ProofFailure,
    SingleDestination, BITRATE, COMPLETION_GRACE, COMPLETION_TIMEOUT,
};

const READY_MESSAGE_TYPE: MessageType = MessageType(0x1338);
const RESOURCE_CHUNK: &[u8] = b"prns-resource-that-must-be-rejected";
const RESOURCE_REPETITIONS: usize = 4096;
const COMPLETE_PATH: &str = "/complete";

#[allow(dead_code)]
#[derive(Debug)]
pub enum ClientFailure {
    MissingTarget,
    AnnounceStreamClosed,
    Establish(SendError<EstablishLinkFailure>),
    Resource(ResourceSendError),
    ResourceAccepted,
    Request(SendError<SendRequestFailure>),
    Pack(PackBinaryError),
    UnexpectedResponse,
    Timeout,
    NodeStopped,
}

pub async fn run_client() -> Result<(), ClientFailure> {
    let target =
        required_environment("PRNS_TCP_TARGET").map_err(|_| ClientFailure::MissingTarget)?;
    let client = TcpClientInterface::new(target);
    let (destination_tx, mut destination_rx) = tokio::sync::mpsc::unbounded_channel();
    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
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
    println!("PRNS_REJECTION_CLIENT_UP");
    let completion = async move {
        let destination = destination_rx
            .recv()
            .await
            .ok_or(ClientFailure::AnnounceStreamClosed)?;
        let link = handle
            .establish_link(destination)
            .await
            .map_err(ClientFailure::Establish)?;
        let payload = RESOURCE_CHUNK.repeat(RESOURCE_REPETITIONS);
        match handle
            .send_resource_with_compression(
                link,
                payload.len() as u64,
                payload.as_slice(),
                SegmentCompression::Never,
            )
            .await
        {
            Err(ResourceSendError::Rejected(SendResourceFailure::RejectedByPeer)) => {}
            Err(error) => return Err(ClientFailure::Resource(error)),
            Ok(()) => return Err(ClientFailure::ResourceAccepted),
        }
        let completion =
            messagepack_binary(b"prns-rejection-observed").map_err(ClientFailure::Pack)?;
        let (response, _rtt) = handle
            .request(link, RequestPathHash::of(COMPLETE_PATH), &completion)
            .await
            .map_err(ClientFailure::Request)?;
        let expected_response =
            messagepack_binary(b"stock-no-publication").map_err(ClientFailure::Pack)?;
        if response != expected_response {
            return Err(ClientFailure::UnexpectedResponse);
        }
        println!("PRNS_OBSERVED_STOCK_REJECTION published=0");
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

struct ServerState {
    completed: tokio::sync::mpsc::UnboundedSender<Result<(), ServerFailure>>,
}

struct Complete;

impl RequestEndpoint<ServerState> for Complete {
    const ENDPOINT_ID: &'static str = COMPLETE_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, ServerState>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        let expected = messagepack_binary(b"stock-rejection-observed")
            .map_err(|_| Decline::ResponseTooLarge)?;
        if context.data != expected {
            let _ = context
                .state
                .completed
                .send(Err(ServerFailure::UnexpectedCompletion));
            return Err(Decline::Ignore);
        }
        let response = context.respond_messagepack_bytes(b"prns-no-publication");
        if response.is_ok() {
            let _ = context.state.completed.send(Ok(()));
        }
        response
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum ServerFailure {
    MissingBind,
    Bind(std::io::Error),
    InvalidDestination,
    Strategy(SendError<personal_rns::engine::SetResourceStrategyFailure>),
    SendReady(SendError<SendToChannelFailure>),
    Proof(ProofFailure),
    UnexpectedPublication,
    UnexpectedCompletion,
    LinkStreamClosed,
    CompletionStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run_server() -> Result<(), ServerFailure> {
    let bind = required_environment("PRNS_TCP_BIND").map_err(|_| ServerFailure::MissingBind)?;
    let server = TcpServer::bind_with_bitrate(bind, BITRATE)
        .await
        .map_err(ServerFailure::Bind)?;
    let destination = SingleDestination {
        app_name: "prns",
        aspects: &["resource", "reject", "interop"],
        test_identity_byte: 0xA6,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: ByteLimit::Maximum(1024),
        request_endpoints: ServeMyRequestEndpoints::Yes,
    }
    .into_preconfigured();
    let own_destination = destination
        .destination_hash()
        .map_err(|_| ServerFailure::InvalidDestination)?;
    let (completed_tx, mut completed_rx) = tokio::sync::mpsc::unbounded_channel();
    let policy_tx = completed_tx.clone();
    let (link_tx, mut link_rx) = tokio::sync::mpsc::unbounded_channel();
    let state = ServerState {
        completed: completed_tx,
    };
    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [destination],
        app_state: state,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![Complete],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, state| match event {
            PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(established)) => {
                let _ = link_tx.send(established.link_id);
            }
            PrnsEvent::Message(
                Message::Resource { .. }
                | Message::ResourceNeedsDecompression { .. }
                | Message::ResourceSegment { .. },
            ) => {
                let _ = state
                    .completed
                    .send(Err(ServerFailure::UnexpectedPublication));
            }
            _ => {}
        },
    });
    let handle = node.handle();
    let _server = handle.supervise(server);
    spawn_announces(handle.clone(), own_destination);
    tokio::spawn(async move {
        let result = async move {
            let Some(link) = link_rx.recv().await else {
                return Err(ServerFailure::LinkStreamClosed);
            };
            handle
                .set_link_resource_strategy(link, ResourceStrategy::AcceptIf)
                .await
                .map_err(ServerFailure::Strategy)?;
            let receipt = handle
                .send_channel_message(link, READY_MESSAGE_TYPE, b"prns-rejection-policy-ready")
                .await
                .map_err(ServerFailure::SendReady)?;
            require_proof(receipt).map_err(ServerFailure::Proof)
        }
        .await;
        if let Err(error) = result {
            let _ = policy_tx.send(Err(error));
        }
    });
    println!("PRNS_REJECTION_SERVER_UP");
    let completion = async move {
        match completed_rx.recv().await {
            Some(Ok(())) => {
                println!("PRNS_REJECTED_STOCK published=0");
                tokio::time::sleep(COMPLETION_GRACE).await;
                Ok(())
            }
            Some(Err(error)) => Err(error),
            None => Err(ServerFailure::CompletionStreamClosed),
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
