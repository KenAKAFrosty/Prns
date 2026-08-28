use core::time::Duration;
use personal_rns::runtime::NoPersistence;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::BitrateBps;
use personal_rns::interfaces::{
    EffectiveInterfacePolicy, InterfaceId, InterfaceKind, InterfaceStatus,
};
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::request_endpoints;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, Fleet, InterfaceSupervisor, ManuallyAttached, PreConfiguredDestination, PrnsEvent,
    PrnsNode, PrnsNodeHandle, PrnsNodeRecipe, ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};

const BITRATE: BitrateBps = BitrateBps::guess(1_000_000);

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

struct DialOnce {
    addr: String,
}

impl InterfaceSupervisor for DialOnce {
    const KIND: InterfaceKind = InterfaceKind::Loopback;

    fn policy(&self) -> EffectiveInterfacePolicy {
        personal_rns::interfaces::tcp::DEFAULTS.configured(
            personal_rns::interfaces::ConfiguredInterfacePolicy {
                bitrate: Some(BITRATE),
                ..Default::default()
            },
        )
    }

    fn channel_tag(&self) -> &[u8] {
        self.addr.as_bytes()
    }

    async fn run(self, fleet: Fleet) {
        let _member = fleet.add(TcpClientInterface::new_with_bitrate(
            self.addr,
            BITRATE,
            ReconnectPolicy::STANDARD,
        ));
        core::future::pending::<()>().await;
    }
}

impl personal_rns::interfaces::ReportsStatus for DialOnce {}

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
async fn two_nodes_stand_up_and_one_hears_the_others_announce() {
    let single_a = single(secret(0xA1));
    let dest_a = single_a
        .destination_hash()
        .expect("the test destination name is valid");

    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    let client = TcpClientInterface::new_with_bitrate(addr, BITRATE, ReconnectPolicy::STANDARD);
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xB2))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
    });

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if commands_a
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    let heard = tokio::select! {
        biased;
        heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv()) => heard
            .expect("B hears A's announce within 5s")
            .expect("the announce channel stays open"),
        result = node_a.run() => unreachable!("node A's run loop returned: {result:?}"),
        result = node_b.run() => unreachable!("node B's run loop returned: {result:?}"),
    };
    assert_eq!(
        heard, dest_a,
        "B heard A's destination through the revived runtime"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interface_added_through_the_handle_carries_traffic_until_torn_down() {
    let single_a = single(secret(0xC1));
    let dest_a = single_a
        .destination_hash()
        .expect("the test destination name is valid");

    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state| {},
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xD2))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
    });
    let commands_b = node_b.handle();

    let attached = commands_b.add_interface(TcpClientInterface::new_with_bitrate(
        addr,
        BITRATE,
        ReconnectPolicy::STANDARD,
    ));

    assert!(
        commands_b
            .interfaces()
            .iter()
            .any(|snapshot| snapshot.id == attached.id()),
        "the runtime tracks the attached interface's status centrally"
    );

    let snapshots_a = commands_a.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if commands_a
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
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
            let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("B hears A over the runtime-added interface within 5s")
                .expect("the announce channel stays open");
            assert_eq!(heard, dest_a, "B heard A through the interface it added at runtime");

            assert!(
                snapshots_a
                    .interfaces()
                    .iter()
                    .any(|snapshot| snapshot.id.kind() == Some(InterfaceKind::TcpServerPeer)),
                "A's server-spawned member registers centrally too — fleet members, not just one-to-one wires"
            );

            attached.teardown();
            tokio::time::sleep(Duration::from_millis(300)).await;
            while heard_rx.try_recv().is_ok() {}
            assert!(
                tokio::time::timeout(Duration::from_millis(800), heard_rx.recv())
                    .await
                    .is_err(),
                "no announce reaches B after the interface is torn down"
            );
        } => {}
        result = node_a.run() => unreachable!("node A's run loop returned: {result:?}"),
        result = node_b.run() => unreachable!("node B's run loop returned: {result:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_supervisor_spawns_a_member_and_tearing_the_supervisor_down_cascades_to_it() {
    let single_a = single(secret(0xE1));
    let dest_a = single_a
        .destination_hash()
        .expect("the test destination name is valid");

    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state| {},
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xF2))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
    });
    let commands_b = node_b.handle();

    let supervisor = commands_b.supervise(DialOnce { addr });

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if commands_a
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
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
            let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("B hears A over the supervisor's member within 5s")
                .expect("the announce channel stays open");
            assert_eq!(heard, dest_a, "B heard A through the member the supervisor spawned");

            supervisor.teardown();
            tokio::time::sleep(Duration::from_millis(300)).await;
            while heard_rx.try_recv().is_ok() {}
            assert!(
                tokio::time::timeout(Duration::from_millis(800), heard_rx.recv())
                    .await
                    .is_err(),
                "tearing the supervisor down cascades to its member, so no announce reaches B"
            );
        } => {}
        result = node_a.run() => unreachable!("node A's run loop returned: {result:?}"),
        result = node_b.run() => unreachable!("node B's run loop returned: {result:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn the_server_stands_up_a_distinct_member_per_client_and_hears_each_on_its_own() {
    let single_a = single(secret(0xA1));
    let dest_a = single_a
        .destination_hash()
        .expect("the test destination name is valid");
    let single_b = single(secret(0xB2));
    let dest_b = single_b
        .destination_hash()
        .expect("the test destination name is valid");

    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let server_status = server.status();
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_s = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single(secret(0x55))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                source_interface,
                ..
            }) = event
            {
                let _ = heard_tx.send((destination, source_interface));
            }
        },
    });
    let commands_s = node_s.handle();
    let _server_sup = commands_s.supervise(server);

    let client_a =
        TcpClientInterface::new_with_bitrate(addr.clone(), BITRATE, ReconnectPolicy::STANDARD);
    let client_a_status = client_a.status();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client_a);
        },
        on_event: |_event, _state| {},
        persistence: NoPersistence,
    });
    let commands_a = node_a.handle();

    let client_b = TcpClientInterface::new_with_bitrate(addr, BITRATE, ReconnectPolicy::STANDARD);
    let client_b_status = client_b.status();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single_b],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client_b);
        },
        on_event: |_event, _state| {},
        persistence: NoPersistence,
    });
    let commands_b = node_b.handle();

    for (commands, dest) in [(commands_a, dest_a), (commands_b, dest_b)] {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(200));
            loop {
                ticker.tick().await;
                if commands
                    .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                        destination: dest,
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
            let mut member_a: Option<InterfaceId> = None;
            let mut member_b: Option<InterfaceId> = None;
            while member_a.is_none() || member_b.is_none() {
                let received = tokio::time::timeout(Duration::from_secs(10), heard_rx.recv()).await;
                let Ok(received) = received else {
                    let member_connections = server_status
                        .members()
                        .into_iter()
                        .map(|member| {
                            (
                                member.id(),
                                member.connection(),
                                member.rx_bytes(),
                                member.tx_bytes(),
                            )
                        })
                        .collect::<std::vec::Vec<_>>();
                    panic!(
                        "the server hears both clients within 10s; heard A: {:?}; heard B: {:?}; client A: ({:?}, {}, {}); client B: ({:?}, {}, {}); server members: {:?}",
                        member_a,
                        member_b,
                        client_a_status.connection(),
                        client_a_status.rx_bytes(),
                        client_a_status.tx_bytes(),
                        client_b_status.connection(),
                        client_b_status.rx_bytes(),
                        client_b_status.tx_bytes(),
                        member_connections,
                    );
                };
                let (destination, source_interface) =
                    received.expect("the announce channel stays open");
                if destination == dest_a {
                    member_a.get_or_insert(source_interface);
                } else if destination == dest_b {
                    member_b.get_or_insert(source_interface);
                }
            }
            let member_a = member_a.expect("client A was heard");
            let member_b = member_b.expect("client B was heard");
            assert_ne!(
                member_a, member_b,
                "each client is a distinct member interface on the server, not one masking the other",
            );
            assert_eq!(
                member_a.kind(),
                Some(InterfaceKind::TcpServerPeer),
                "client A arrived on a spawned TcpServerPeer member, not the supervisor itself",
            );
            assert_eq!(
                member_b.kind(),
                Some(InterfaceKind::TcpServerPeer),
                "client B arrived on a spawned TcpServerPeer member, not the supervisor itself",
            );
        } => {}
        result = node_s.run() => unreachable!("the server node's run loop returned: {result:?}"),
        result = node_a.run() => unreachable!("client A's run loop returned: {result:?}"),
        result = node_b.run() => unreachable!("client B's run loop returned: {result:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recipe_accept_destination_receives_a_resource() {
    let single_a = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects: &["link"],
        identity: secret(0xF1),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::Accept {
            max_uncompressed_bytes: 1024 * 1024,
            accept_compressed: true,
        },
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    };
    let dest_a = single_a.destination_hash().expect("valid destination");

    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state| {},
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let client = TcpClientInterface::new_with_bitrate(addr, BITRATE, ReconnectPolicy::STANDARD);
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xF2))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        persistence: NoPersistence,
    });
    let commands_b = node_b.handle();

    let announcer = commands_a.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(150));
        loop {
            ticker.tick().await;
            if announcer
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    let sent = tokio::select! {
        biased;
        result = async {
            let destination = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("B hears A within 5s")
                .expect("the announce channel stays open");
            let link_id = commands_b
                .establish_link(destination)
                .await
                .expect("the link establishes");
            let payload = std::vec![0x5au8; 64 * 1024];
            tokio::time::timeout(
                Duration::from_secs(5),
                commands_b.send_resource(link_id, payload.len() as u64, &payload[..]),
            )
            .await
            .expect("the resource transfer settles within 5s")
        } => result,
        result = node_a.run() => unreachable!("node A's run loop returned: {result:?}"),
        result = node_b.run() => unreachable!("node B's run loop returned: {result:?}"),
    };

    assert!(
        sent.is_ok(),
        "a recipe Accept destination receives a resource over a link: {sent:?}",
    );
}
