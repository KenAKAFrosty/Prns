#![expect(clippy::expect_used, clippy::panic)]

mod common;

use core::time::Duration;

use personal_rns::prelude::*;

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() {
    let announcing_destination = example_destination();
    let announced_hash = announcing_destination
        .destination_hash()
        .expect("invalid example destination name");

    let server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("could not bind a localhost TCP server");
    let relay_address = server
        .local_addr()
        .expect("could not read the bound server address")
        .to_string();
    let relay = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(
            try_generate_identity_secret().expect("identity generation failed"),
        ),
        remote_control: common::remote_control_service(0xD0, 0xD1),
        pre_configured_destinations: [example_destination()],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let relay_handle = relay.handle();
    let _server = relay_handle.supervise(server);
    println!("Relay: transport node listening on {relay_address}");

    let announcer_client = TcpClientInterface::new(relay_address.clone());
    let announcing_node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: common::remote_control_service(0xD2, 0xD3),
        pre_configured_destinations: [announcing_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(announcer_client);
        },
        persistence: NoPersistence,
    });
    let announcing_handle = announcing_node.handle();

    let (heard_sender, mut heard_listener) = tokio::sync::mpsc::unbounded_channel();
    let listener_client = TcpClientInterface::new(relay_address);
    let listening_node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: common::remote_control_service(0xD4, 0xD5),
        pre_configured_destinations: [example_destination()],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ignored = heard_sender.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(listener_client);
        },
        persistence: NoPersistence,
    });
    println!("Announcer and listener: TCP clients of the relay, with no link to each other");

    let announcer = announcing_handle.clone();
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: announced_hash,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });

    let mut run_relay = std::pin::pin!(relay.run());
    let mut run_announcer = std::pin::pin!(announcing_node.run());
    let mut run_listener = std::pin::pin!(listening_node.run());
    let deadline = tokio::time::Instant::now() + DELIVERY_TIMEOUT;
    loop {
        let heard = tokio::select! {
            heard = tokio::time::timeout_at(deadline, heard_listener.recv()) => {
                heard
                    .expect("The relayed announce did not arrive within 10 seconds")
                    .expect("The listener's event stream closed before delivery")
            }
            result = &mut run_relay => {
                result.expect("The relay failed");
                panic!("The relay stopped before delivery");
            }
            result = &mut run_announcer => {
                result.expect("The announcing node failed");
                panic!("The announcing node stopped before delivery");
            }
            result = &mut run_listener => {
                result.expect("The listening node failed");
                panic!("The listening node stopped before delivery");
            }
        };
        if heard == announced_hash {
            break;
        }
    }
    println!("Success: the announce crossed two links; only the transport node connected them");
}

fn example_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        app_name: "prns-example",
        aspects: &["example", "transport-node"],
        identity: try_generate_identity_secret().expect("identity generation failed"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}
