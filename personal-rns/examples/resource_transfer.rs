use core::time::Duration;

use personal_rns::prelude::*;

const PAYLOAD_BYTES: usize = 64 * 1024;
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() {
    let receiver_destination = example_destination(ResourceStrategy::Accept {
        max_uncompressed_bytes: PAYLOAD_BYTES as u64,
        accept_compressed: true,
    });
    let receiver_hash = receiver_destination
        .destination_hash()
        .expect("Our example destination has valid app name and aspects");
    let tcp_server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("A local TCP server should bind");
    let server_address = tcp_server
        .local_addr()
        .expect("TCP server address should be valid")
        .to_string();
    let receiver = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [receiver_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let receiver_handle = receiver.handle();
    let _server = receiver_handle.supervise(tcp_server);

    let (announce_heard_sender, mut announce_heard_listener) =
        tokio::sync::mpsc::unbounded_channel();

    let client = TcpClientInterface::new(server_address);
    let sender = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [example_destination(ResourceStrategy::AcceptNone)],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ignored = announce_heard_sender.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
    });

    let sender_handle = sender.handle();
    let announcer = receiver_handle.clone();
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: receiver_hash,
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
            if destination == receiver_hash {
                break;
            }
        }
        let link_id = sender_handle
            .establish_link(receiver_hash)
            .await
            .expect("The link to the receiver should establish");
        let payload = vec![0x5a; PAYLOAD_BYTES];
        sender_handle
            .send_resource(link_id, payload.len() as u64, payload.as_slice())
            .await
            .expect("The resource transfer should settle");
        println!("Transferred {PAYLOAD_BYTES} bytes to the accepting peer");
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.expect("The transfer should complete within 10 seconds");
        }
        result = receiver.run() => {
            result.expect("The receiver should run cleanly");
            panic!("The receiver stopped before the transfer");
        }
        result = sender.run() => {
            result.expect("The sender should run cleanly");
            panic!("The sender stopped before the transfer");
        }
    }
}

fn example_destination(resource_strategy: ResourceStrategy) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy,
        app_name: "prns-example",
        aspects: &["resource-transfer"],
        identity: try_generate_identity_secret().expect("OS entropy should be available"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}
