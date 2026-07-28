use core::time::Duration;
use std::error::Error;
use std::io;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::interfaces::BitrateBps;
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::request_endpoints;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, RequestEndpointRegistration,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::try_generate_identity_secret;

const BITRATE: BitrateBps = BitrateBps::guess(1_000_000);
const QUERY_PATH: &str = "/example/echo";
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

struct Responder;

struct Echo;

impl RequestEndpoint<Responder> for Echo {
    const PATH: &'static str = QUERY_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, Responder>) -> Result<(), Decline> {
        let requested = context.data;
        let _written = context.write_packed(requested);
        context.respond_packed(b"-response")
    }
}

fn destination(
    request_endpoints: RequestEndpointRegistration,
) -> Result<PreConfiguredDestination<'static>, Box<dyn Error>> {
    Ok(PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["bounded-request"],
        identity: try_generate_identity_secret()?,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints,
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn Error>> {
    let responder_destination = destination(RequestEndpointRegistration::NodeRequestEndpointSet)?;
    let responder_hash = responder_destination
        .destination_hash()
        .map_err(|error| io::Error::other(format!("invalid destination name: {error:?}")))?;
    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE).await?;
    let server_address = server.local_addr()?.to_string();
    let responder = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [responder_destination],
        app_state: Responder,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![Echo],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
    });
    let responder_handle = responder.handle();
    let _server = responder_handle.supervise(server);

    let (announce_tx, mut announce_rx) = tokio::sync::mpsc::unbounded_channel();
    let client =
        TcpClientInterface::new_with_bitrate(server_address, BITRATE, ReconnectPolicy::STANDARD);
    let requester = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination(RequestEndpointRegistration::None)?],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ignored = announce_tx.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(client);
        },
    });
    let requester_handle = requester.handle();
    let announcer = responder_handle.clone();
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: responder_hash,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });

    let exchange = async {
        let announced = loop {
            let destination = announce_rx
                .recv()
                .await
                .ok_or_else(|| io::Error::other("announce stream closed"))?;
            if destination == responder_hash {
                break destination;
            }
        };
        let link_id = requester_handle
            .establish_link(announced)
            .await
            .map_err(|error| io::Error::other(format!("link failed: {error:?}")))?;
        let (response, rtt) = requester_handle
            .request(link_id, RequestPathHash::of(QUERY_PATH), b"bounded")
            .await
            .map_err(|error| io::Error::other(format!("request failed: {error:?}")))?;
        if response.as_slice() != b"bounded-response" {
            return Err(io::Error::other("response payload differs"));
        }
        println!("Received {} bytes in {rtt:?}", response.len());
        Ok::<(), io::Error>(())
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request exceeded 10 seconds"))??;
        }
        result = responder.run() => {
            result?;
            return Err(io::Error::other("responder stopped before the exchange").into());
        }
        result = requester.run() => {
            result?;
            return Err(io::Error::other("requester stopped before the exchange").into());
        }
    }
    Ok(())
}
