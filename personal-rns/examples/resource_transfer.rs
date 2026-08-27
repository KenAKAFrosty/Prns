#![expect(clippy::expect_used, clippy::panic)]

mod common;

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
        .expect("invalid example destination name");
    let tcp_server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("could not bind a localhost TCP server");
    let server_address = tcp_server
        .local_addr()
        .expect("could not read the bound server address")
        .to_string();
    let receiver = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: common::remote_control_service(0xD0, 0xD1),
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
        remote_control: common::remote_control_service(0xD2, 0xD3),
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
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
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
                .expect("The announce stream closed before delivery");
            if destination == receiver_hash {
                break;
            }
        }
        let link_id = sender_handle
            .establish_link(receiver_hash)
            .await
            .expect("the link to the receiver did not establish");
        let payload = vec![0x5a; PAYLOAD_BYTES];
        sender_handle
            .send_resource(link_id, payload.len() as u64, payload.as_slice())
            .await
            .expect("the resource transfer did not settle");
        println!("Transferred {PAYLOAD_BYTES} bytes to the accepting peer");
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.expect("The transfer did not complete within 10 seconds");
        }
        result = receiver.run() => {
            result.expect("The receiver failed");
            panic!("The receiver stopped before the transfer");
        }
        result = sender.run() => {
            result.expect("The sender failed");
            panic!("The sender stopped before the transfer");
        }
    }
}

fn example_destination(resource_strategy: ResourceStrategy) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy,
        maximum_request_bytes: Default::default(),
        app_name: "prns-example",
        aspects: &["example", "resource-transfer"],
        identity: try_generate_identity_secret().expect("identity generation failed"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}
