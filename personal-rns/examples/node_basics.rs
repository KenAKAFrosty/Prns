//! A complete, bounded two-node Reticulum exchange over an isolated localhost TCP link. See `docs/getting-started.md` for context.

#![expect(clippy::expect_used, clippy::panic)]

mod common;

use core::time::Duration;

use personal_rns::prelude::*;

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let destination_a = example_preconfigured_destination();

    let destination_a_hash = destination_a
        .destination_hash()
        .expect("invalid example destination name");

    let destination_b = example_preconfigured_destination();

    let tcp_server_interface = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("could not bind a localhost TCP server");

    let server_address = tcp_server_interface
        .local_addr()
        .expect("could not read the bound server address")
        .to_string();

    println!("Node A: TCP server listening on {server_address}");

    let node_a = PrnsNode::new(PrnsNodeRecipe {
        pre_configured_destinations: [destination_a],
        transport_identity: None,
        remote_control: common::remote_control_service(0xD0, 0xD1),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        app_state: (),
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let node_a_handle = node_a.handle();
    let _server = node_a_handle.supervise(tcp_server_interface);

    let client = TcpClientInterface::new(server_address);
    let (heard_announce_sender, mut heard_announce_listener) =
        tokio::sync::mpsc::unbounded_channel();

    let node_b = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: common::remote_control_service(0xD2, 0xD3),
        pre_configured_destinations: [destination_b],
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        app_state: (),
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                source_interface,
                ..
            }) = event
            {
                let _ignored = heard_announce_sender.send((destination, source_interface));
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
    });
    let node_b_handle = node_b.handle();
    println!("Node B: TCP client only (no radio or USB discovery)");

    let announcer = node_a_handle.clone(); //The handle is cheap to clone. It does not clone the whole node.
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;

            //Since this is a destination with registered app data, and we're just announcing to every interface, we could also use the simple convenience method of `announcer.announce(destination_a_hash);`
            //This example uses the `.issue` + `PrnsCommand` approach because
            // (a) it shows you this is possible and provides you a "catalog of actions", to so speak, in the form of the PrnsCommand enum; and
            // (b) announcing doesn't have meaningful data to await for anyway, so a fire-and-forget command is especially appropriate here.
            //
            // As you continue to work through the example ladder you'll see the use of other APIs that stay awaitable-method-focused instead of using this command issuance approach.
            if announcer
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: destination_a_hash,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });

    let observed = tokio::select! {
        heard_result = tokio::time::timeout(DELIVERY_TIMEOUT, heard_announce_listener.recv()) => {
            heard_result
                .expect("Node B did not observe Node A's announce over TCP within 10 seconds")
                .expect("Node B's event stream closed before delivery")
        }
        result = node_a.run() => {
            result.expect("Node A failed");
            panic!("Node A stopped before delivery");
        }
        result = node_b.run() => {
            result.expect("Node B failed");
            panic!("Node B stopped before delivery");
        }
    };

    assert_eq!(
        observed.0, destination_a_hash,
        "Node B should observe Node A's destination"
    );
    assert_eq!(
        observed.1.kind(),
        Some(InterfaceKind::TcpClient),
        "The announce should arrive through Node B's TCP client"
    );

    println!(
        "Success: Node B observed Node A's real Reticulum announce on {:?} ({:?}).",
        observed.1,
        observed.1.kind()
    );
    println!("Node B interface inventory:");
    for interface in node_b_handle.interfaces() {
        println!(
            "  {:?} connection={:?} rx={} tx={}",
            interface.id, interface.connection, interface.rx_bytes, interface.tx_bytes
        );
    }
}

fn example_preconfigured_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        app_name: "prns-example",
        aspects: &["example", "node-basics"],
        identity: try_generate_identity_secret().expect("identity generation failed"),
        announce_app_data: b"hello from node A",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}
