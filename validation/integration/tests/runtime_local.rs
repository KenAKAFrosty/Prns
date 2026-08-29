use core::time::Duration;
use personal_rns::runtime::NoPersistence;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::BitrateBps;
use personal_rns::interfaces::InterfaceKind;
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::request_endpoints;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, ServeMyRequestEndpoints,
};
use personal_rns::shared_instance::SharedInstanceServer;
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};

const BITRATE: BitrateBps = BitrateBps::guess(1_000_000);

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

fn single(identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "bench",
        aspects: &["link"],
        identity,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_app_dials_the_shared_instance_and_is_heard_at_a_discounted_hop() {
    let app_single = single(secret(0xB1));
    let dest_app = app_single
        .destination_hash()
        .expect("the test destination name is valid");

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("an ephemeral port binds")
        .local_addr()
        .expect("the bound port is known")
        .port();

    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let daemon = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xD1))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                hops,
                source_interface,
                app_data: _,
            }) = event
            {
                let _ = heard_tx.send((destination, hops, source_interface));
            }
        },
    });
    let daemon_commands = daemon.handle();
    let _server = daemon_commands.supervise(SharedInstanceServer::with_port(port));

    let app = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [app_single],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state| {},
    });
    let app_commands = app.handle();
    let _attached = app_commands.add_interface(TcpClientInterface::new_with_bitrate(
        std::format!("127.0.0.1:{port}"),
        BITRATE,
        ReconnectPolicy::STANDARD,
    ));

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if app_commands
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: dest_app,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    tokio::select! {
        biased;
        () = async {
            let (destination, hops, source_interface) =
                tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                    .await
                    .expect("the daemon hears the app within 5s")
                    .expect("the announce channel stays open");
            assert_eq!(destination, dest_app, "the daemon heard the app's destination");
            assert_eq!(
                source_interface.kind(),
                Some(InterfaceKind::LocalClient),
                "the daemon heard it on a spawned LocalClient member, not the supervisor itself"
            );
            assert_eq!(
                hops, 0,
                "the free local transit is discounted: a 0-hop announce stays 0 across the shared instance"
            );
        } => {}
        result = daemon.run() => unreachable!("the daemon's run loop returned: {result:?}"),
        result = app.run() => unreachable!("the app's run loop returned: {result:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_leaf_shared_instance_carries_announces_across_its_local_boundary() {
    let local_port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("an ephemeral local port binds")
        .local_addr()
        .expect("the local port is known")
        .port();
    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("the network server binds");
    let network_addr = server
        .local_addr()
        .expect("the network address is known")
        .to_string();

    let daemon = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state| {},
    })
    .with_shared_instance_identity(secret(0xD1))
    .expect("the shared instance identity fits");
    let daemon_handle = daemon.handle();
    let _local = daemon_handle.supervise(SharedInstanceServer::with_port(local_port));
    let _network = daemon_handle.supervise(server);

    let network_single = single(secret(0xA1));
    let network_destination = network_single
        .destination_hash()
        .expect("the network destination is valid");
    let (network_heard_tx, mut network_heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let network_client =
        TcpClientInterface::new_with_bitrate(network_addr, BITRATE, ReconnectPolicy::STANDARD);
    let network_node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [network_single],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(network_client);
        },
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = network_heard_tx.send(destination);
            }
        },
        persistence: NoPersistence,
    });
    let network_handle = network_node.handle();

    let local_single = single(secret(0xB2));
    let local_destination = local_single
        .destination_hash()
        .expect("the local destination is valid");
    let (local_heard_tx, mut local_heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let local_client = TcpClientInterface::new_with_bitrate(
        std::format!("127.0.0.1:{local_port}"),
        BITRATE,
        ReconnectPolicy::STANDARD,
    );
    let local_node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [local_single],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(local_client);
        },
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = local_heard_tx.send(destination);
            }
        },
        persistence: NoPersistence,
    });
    let local_handle = local_node.handle();

    for (handle, destination) in [
        (network_handle, network_destination),
        (local_handle, local_destination),
    ] {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(200));
            loop {
                ticker.tick().await;
                if handle
                    .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });
    }

    tokio::select! {
        biased;
        () = async {
            let heard_local = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if local_heard_rx.recv().await == Some(network_destination) {
                        break;
                    }
                }
            }).await;
            assert!(heard_local.is_ok(), "the local app hears the network destination");

            let heard_network = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if network_heard_rx.recv().await == Some(local_destination) {
                        break;
                    }
                }
            }).await;
            assert!(heard_network.is_ok(), "the network peer hears the local destination");
        } => {}
        result = daemon.run() => unreachable!("the daemon's run loop returned: {result:?}"),
        result = network_node.run() => unreachable!("the network node's run loop returned: {result:?}"),
        result = local_node.run() => unreachable!("the local node's run loop returned: {result:?}"),
    }
}
