use core::time::Duration;
use std::cell::Cell;
use std::error::Error;
use std::io;

use personal_rns::prelude::*;

const STATUS_ENDPOINT_ID: &str = "/example/status";
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

struct StatusBoard {
    greeting: &'static str,
    hits: Cell<u32>,
}

struct Status;
impl RequestEndpoint<StatusBoard> for Status {
    const ENDPOINT_ID: &'static str = STATUS_ENDPOINT_ID;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, StatusBoard>) -> Result<(), Decline> {
        let hits = context.state.hits.get() + 1;
        context.state.hits.set(hits);
        let reply = format!("{}, visitor {hits}", context.state.greeting);
        context.respond_packed(reply.as_bytes())
    }
}

struct AnnounceRelay {
    heard: tokio::sync::mpsc::UnboundedSender<DestinationHash>,
}

fn forward_announces(event: PrnsEvent<'_>, relay: &AnnounceRelay) {
    if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
        let _ignored = relay.heard.send(destination);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let responder_destination = responder_destination()?;
    let responder_hash = responder_destination.destination_hash().map_err(|error| {
        io::Error::other(format!("invalid example destination name: {error:?}"))
    })?;
    let server = TcpServer::bind("127.0.0.1:0").await?;
    let server_address = server.local_addr()?.to_string();
    let responder = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [responder_destination],
        app_state: StatusBoard {
            greeting: "hello",
            hits: Cell::new(0),
        },
        storage: GrowableHeap,
        request_endpoints: request_endpoints![Status],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let responder_handle = responder.handle();
    let _server = responder_handle.supervise(server);

    let (heard_sender, mut heard_listener) = tokio::sync::mpsc::unbounded_channel();
    let client = TcpClientInterface::new(server_address);
    let requester = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [requester_destination()?],
        app_state: AnnounceRelay {
            heard: heard_sender,
        },
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: forward_announces,
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
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
        loop {
            let destination = heard_listener
                .recv()
                .await
                .ok_or_else(|| io::Error::other("The announce stream closed before delivery"))?;
            if destination == responder_hash {
                break;
            }
        }
        let link_id = requester_handle
            .establish_link(responder_hash)
            .await
            .map_err(|error| {
                io::Error::other(format!(
                    "the link to the responder did not establish: {error:?}"
                ))
            })?;
        for expected in ["hello, visitor 1", "hello, visitor 2"] {
            let (response, rtt) = requester_handle
                .request(link_id, RequestEndpointId::of(STATUS_ENDPOINT_ID), b"")
                .await
                .map_err(|error| {
                    io::Error::other(format!("the status request did not settle: {error:?}"))
                })?;
            assert_eq!(
                response.as_slice(),
                expected.as_bytes(),
                "The endpoint should serve its state's greeting and hit count"
            );
            println!("Response in {rtt:?}: {expected}");
        }
        println!(
            "Success: the endpoint served and updated the node's own state across two requests"
        );
        Ok::<(), Box<dyn Error>>(())
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result
                .map_err(|_| io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The exchange did not complete within 10 seconds",
                ))??;
        }
        result = responder.run() => {
            result?;
            return Err(io::Error::other("The responder stopped before the exchange").into());
        }
        result = requester.run() => {
            result?;
            return Err(io::Error::other("The requester stopped before the exchange").into());
        }
    }
    Ok(())
}

fn responder_destination() -> Result<PreConfiguredDestination<'static>, Box<dyn Error>> {
    Ok(PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["example", "app-state"],
        identity: try_generate_identity_secret()?,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::Yes,
    })
}

fn requester_destination() -> Result<PreConfiguredDestination<'static>, Box<dyn Error>> {
    Ok(PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["example", "app-state"],
        identity: try_generate_identity_secret()?,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    })
}
