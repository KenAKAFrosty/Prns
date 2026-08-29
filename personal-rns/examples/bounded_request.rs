#![expect(clippy::expect_used, clippy::panic)]

mod common;

use core::time::Duration;

use personal_rns::prelude::*;

/// You can actually provide whatever string you'd like. But it's common convention to use URL/filesystem-style syntax like this.
const EXAMPLE_ENDPOINT_ID: &str = "/example/echo";
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

struct Echo;
impl RequestEndpoint for Echo {
    const ENDPOINT_ID: &'static str = EXAMPLE_ENDPOINT_ID;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, ()>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        let data_from_request = context.data;
        context.respond(data_from_request)
    }
}

#[tokio::main]
async fn main() {
    let responder_destination = responder_destination();

    let responder_hash = responder_destination
        .destination_hash()
        .expect("invalid example destination name");

    let tcp_server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("could not bind a localhost TCP server");

    let server_address = tcp_server
        .local_addr()
        .expect("could not read the bound server address")
        .to_string();

    let responder = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: common::remote_control_service(0xD0, 0xD1),
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
        remote_control: common::remote_control_service(0xD2, 0xD3),
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
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
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
                .expect("The announce stream closed before delivery");
            if destination == responder_hash {
                break;
            }
        }
        let link_id = requester_handle
            .establish_link(responder_hash)
            .await
            .expect("the link to the responder did not establish");

        let original_message = b"bounded";

        let (response, rtt) = requester_handle
            .request(
                link_id,
                RequestEndpointId::of(EXAMPLE_ENDPOINT_ID),
                original_message,
            )
            .await
            .expect("the echo request did not settle");

        assert_eq!(
            response.as_slice(),
            original_message,
            "The echo response should match what was sent"
        );
        println!("Received {} bytes in {rtt:?}", response.len());
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.expect("The exchange did not complete within 10 seconds");
        }
        result = responder.run() => {
            result.expect("The responder failed");
            panic!("The responder stopped before the exchange");
        }
        result = requester.run() => {
            result.expect("The requester failed");
            panic!("The requester stopped before the exchange");
        }
    }
}

fn responder_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        request_endpoints: ServeMyRequestEndpoints::Yes,

        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        app_name: "prns-example",
        aspects: &["example", "bounded-request"],
        identity: try_generate_identity_secret().expect("identity generation failed"),
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
        maximum_request_bytes: Default::default(),
        app_name: "prns-example",
        aspects: &["example", "bounded-request"],
        identity: try_generate_identity_secret().expect("identity generation failed"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
    }
}
