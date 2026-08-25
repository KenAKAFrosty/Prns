use personal_rns::engine::{EstablishLinkFailure, RatchetPolicy, SendRequestFailure};
use personal_rns::request_endpoints;
use personal_rns::routing::links::request::PackBinaryError;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{
    Diagnostic, NoPersistence, PrnsEvent, PrnsNode, PrnsNodeRecipe, SendError,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpClientInterface;
use personal_rns::units::ByteLimit;

use super::common::{
    messagepack_binary, required_environment, spawn_announces, SingleDestination, COMPLETION_GRACE,
    COMPLETION_TIMEOUT,
};

const REQUEST_PATH: &str = "/large";
const REQUEST_FROM_PRNS: &[u8] = b"prns-request";
const REQUEST_FROM_STOCK: &[u8] = b"stock-request";
const RESPONSE_SIZE: usize = 128 * 1024;

struct Large;

impl RequestEndpoint for Large {
    const ENDPOINT_ID: &'static str = REQUEST_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, ()>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        let expected =
            messagepack_binary(REQUEST_FROM_STOCK).map_err(|_| Decline::ResponseTooLarge)?;
        if context.data != expected {
            return Err(Decline::Ignore);
        }
        context.respond_messagepack_bytes(&candidate_response())
    }
}

fn candidate_response() -> Vec<u8> {
    (0..RESPONSE_SIZE)
        .map(|index| (index.wrapping_mul(29).wrapping_add(7)) as u8)
        .collect()
}

fn stock_response() -> Vec<u8> {
    (0..RESPONSE_SIZE)
        .map(|index| (index.wrapping_mul(17).wrapping_add(3)) as u8)
        .collect()
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum Failure {
    MissingTarget,
    InvalidDestination,
    AnnounceStreamClosed,
    Establish(SendError<EstablishLinkFailure>),
    Request(SendError<SendRequestFailure>),
    Pack(PackBinaryError),
    UnexpectedResponse,
    Timeout,
    NodeStopped,
}

pub async fn run() -> Result<(), Failure> {
    let target = required_environment("PRNS_TCP_TARGET").map_err(|_| Failure::MissingTarget)?;
    let destination = SingleDestination {
        app_name: "prns",
        aspects: &["large", "client"],
        test_identity_byte: 0xA5,
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
        .map_err(|_| Failure::InvalidDestination)?;
    let client = TcpClientInterface::new(target);
    let (destination_tx, mut destination_rx) = tokio::sync::mpsc::unbounded_channel();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![Large],
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
    spawn_announces(handle.clone(), own_destination);
    println!("PRNS_LARGE_REQUEST_CLIENT_UP");
    let completion = async move {
        let stock_destination = destination_rx
            .recv()
            .await
            .ok_or(Failure::AnnounceStreamClosed)?;
        let link = handle
            .establish_link(stock_destination)
            .await
            .map_err(Failure::Establish)?;
        let packed_request = messagepack_binary(REQUEST_FROM_PRNS).map_err(Failure::Pack)?;
        let (response, _rtt) = handle
            .request(link, RequestPathHash::of(REQUEST_PATH), &packed_request)
            .await
            .map_err(Failure::Request)?;
        let expected_response = messagepack_binary(&stock_response()).map_err(Failure::Pack)?;
        if response != expected_response {
            return Err(Failure::UnexpectedResponse);
        }
        println!("PRNS_LARGE_REQUEST_OK response=131072 responded=131072");
        tokio::time::sleep(COMPLETION_GRACE).await;
        Ok(())
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
