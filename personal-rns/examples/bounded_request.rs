use core::time::Duration;

use personal_rns::prelude::*;

/// You can actually provide whatever string you'd like. But it's common convention to use URL/filesystem-style syntax like this.
const EXAMPLE_ENDPOINT_ID: &str = "/example/echo";
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

struct Echo;
impl RequestEndpoint for Echo {
    const ENDPOINT_ID: &'static str = EXAMPLE_ENDPOINT_ID;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, ()>) -> Result<(), Decline> {
        let data_from_request = context.data;
        context.respond_packed(data_from_request)
    }
}

#[tokio::main]
async fn main() {
    let responder_destination = responder_destination();

    let responder_hash = responder_destination
        .destination_hash()
        .expect("Our example destination has valid app name and aspects");

    let tcp_server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("A local TCP server should bind");

    let server_address = tcp_server
        .local_addr()
        .expect("TCP server address should be valid")
        .to_string();

    let responder = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [responder_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![Echo],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let responder_handle = responder.handle();
    let _server = responder_handle.supervise(tcp_server);

    let (announce_heard_sender, mut announce_heard_listener) =
        tokio::sync::mpsc::unbounded_channel();

    let tcp_client = TcpClientInterface::new(server_address);
    let requester = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [requester_destination()],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ignored = announce_heard_sender.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(tcp_client);
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
            let destination = announce_heard_listener
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

        let original_message = b"bounded";

        let (response, rtt) = requester_handle
            .request(
                link_id,
                RequestEndpointId::of(EXAMPLE_ENDPOINT_ID),
                original_message,
            )
            .await
            .expect("The echo request should settle");

        assert_eq!(
            response.as_slice(),
            original_message,
            "The echo response should match what was sent"
        );
        println!("Received {} bytes in {rtt:?}", response.len());
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
        request_endpoints: ServeMyRequestEndpoints::Yes,

        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["bounded-request", "responder"],
        identity: try_generate_identity_secret().expect("OS entropy should be available"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
    }
}

fn requester_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        request_endpoints: ServeMyRequestEndpoints::No,

        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["bounded-request", "requester"],
        identity: try_generate_identity_secret().expect("OS entropy should be available"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
    }
}
