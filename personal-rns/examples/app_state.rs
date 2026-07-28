use core::time::Duration;
use std::cell::Cell;

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
async fn main() {
    let responder_destination = responder_destination();
    let responder_hash = responder_destination
        .destination_hash()
        .expect("Our example destination has valid app name and aspects");
    let server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("A local TCP server should bind");
    let server_address = server
        .local_addr()
        .expect("TCP server address should be valid")
        .to_string();
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
    });
    let responder_handle = responder.handle();
    let _server = responder_handle.supervise(server);

    let (heard_sender, mut heard_listener) = tokio::sync::mpsc::unbounded_channel();
    let client = TcpClientInterface::new(server_address);
    let requester = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [requester_destination()],
        app_state: AnnounceRelay {
            heard: heard_sender,
        },
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: forward_announces,
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
        loop {
            let destination = heard_listener
                .recv()
                .await
                .expect("The announce stream should stay open");
            if destination == responder_hash {
                break;
            }
        }
        let link_id = requester_handle
            .establish_link(responder_hash)
            .await
            .expect("The link to the responder should establish");
        for expected in ["hello, visitor 1", "hello, visitor 2"] {
            let (response, rtt) = requester_handle
                .request(link_id, RequestEndpointId::of(STATUS_ENDPOINT_ID), b"")
                .await
                .expect("The status request should settle");
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
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.expect("The exchange should complete within 10 seconds");
        }
        result = responder.run() => {
            result.expect("The responder should run cleanly");
            panic!("The responder stopped before the exchange");
        }
        result = requester.run() => {
            result.expect("The requester should run cleanly");
            panic!("The requester stopped before the exchange");
        }
    }
}

fn responder_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["app-state", "responder"],
        identity: try_generate_identity_secret().expect("OS entropy should be available"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::Yes,
    }
}

fn requester_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["app-state", "requester"],
        identity: try_generate_identity_secret().expect("OS entropy should be available"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}
