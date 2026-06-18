//! The revived high-level runtime, end to end: nodes each stood up by `Prns::new` and driven by
//! `prns.run()`, talking across a real TCP loopback. A announces (pure app policy, driven through the
//! command handle); the other side hears it through the curated `PrnsEvent` lane. The listening end
//! is the `TcpServer` supervisor, which stands up a distinct engine interface per client that
//! connects — so the multi-client test asserts two clients land as two separate members. An
//! integration test so it builds against the public API and skips the lib's `#[cfg(test)]` modules.

#![cfg(feature = "tcp")]

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::tcp::client::tokio::TcpClientInterface;
use personal_rns::interfaces::rns_parity::tcp::server::tokio::TcpServer;
use personal_rns::interfaces::{InterfaceId, InterfaceKind};
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    Diagnostic, Fleet, InterfaceSupervisor, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::{interfaces, routes};

const BITRATE: u32 = 1_000_000;

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

/// A minimal interface supervisor that stands up exactly one TCP-client member dialing `addr`, holds
/// the member handle, then parks for life, standing in for a real discovery loop. Tearing the
/// supervisor down must cascade to the member.
struct DialOnce {
    addr: String,
}

impl InterfaceSupervisor for DialOnce {
    const KIND: InterfaceKind = InterfaceKind::Loopback;

    fn reachability_tag(&self) -> &[u8] {
        self.addr.as_bytes()
    }

    async fn run(self, fleet: Fleet) {
        let _member = fleet.add(TcpClientInterface::new(
            self.addr,
            BITRATE,
            Duration::from_millis(100),
        ));
        core::future::pending::<()>().await;
    }
}

impl personal_rns::interfaces::ReportsStatus for DialOnce {}

fn single(identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        app_name: "bench",
        aspects: &["link"],
        identity,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_nodes_stand_up_and_one_hears_the_others_announce() {
    let single_a = single(secret(0xA1));
    let dest_a = single_a
        .destination_hash()
        .expect("the test destination name is valid");

    // Node A: a TCP server supervisor + a Single it will announce. The supervisor stands up a member
    // when B connects; A's announce rides that member to B.
    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: |_event, _state| {},
        interfaces: interfaces![],
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    // Node B: a TCP client to A; reports any announce it hears through the curated event lane.
    let client = TcpClientInterface::new(addr, BITRATE, Duration::from_millis(100));
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single(secret(0xB2))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: interfaces![client],
    });

    // A announces on a cadence — pure app policy — until B hears it. The handle is `Send`, so the
    // ticker rides its own task while both nodes' `run` loops are driven below.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if commands_a
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
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

    // `run` is `!Send` (it owns the reactor), so both nodes are driven on this task, racing the
    // assertion. Whichever resolves first wins; the loops never return on their own.
    let heard = tokio::select! {
        biased;
        heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv()) => heard
            .expect("B hears A's announce within 5s")
            .expect("the announce channel stays open"),
        () = node_a.run() => unreachable!("node A's run loop returned"),
        () = node_b.run() => unreachable!("node B's run loop returned"),
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

    // Node A: a TCP server supervisor that announces, wired the ordinary recipe way.
    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: |_event, _state| {},
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    // Node B is born with NO interface; it gets one at runtime through its handle.
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single(secret(0xD2))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
    });
    let commands_b = node_b.handle();

    // The wire B was not born with: a TCP client dialing A, attached after the fact.
    let attached = commands_b.add_interface(TcpClientInterface::new(
        addr,
        BITRATE,
        Duration::from_millis(100),
    ));

    assert!(
        commands_b
            .interface_snapshots()
            .iter()
            .any(|snapshot| snapshot.id == attached.id()),
        "the runtime tracks the attached interface's status centrally"
    );

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if commands_a
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
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
            // The runtime-added interface carries A's announce to B.
            let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("B hears A over the runtime-added interface within 5s")
                .expect("the announce channel stays open");
            assert_eq!(heard, dest_a, "B heard A through the interface it added at runtime");

            // Tear the interface down; B should fall silent once the wire is gone.
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
        () = node_a.run() => unreachable!("node A's run loop returned"),
        () = node_b.run() => unreachable!("node B's run loop returned"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_supervisor_spawns_a_member_and_tearing_the_supervisor_down_cascades_to_it() {
    let single_a = single(secret(0xE1));
    let dest_a = single_a
        .destination_hash()
        .expect("the test destination name is valid");

    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: |_event, _state| {},
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    // Node B starts wireless; a supervisor stands up its member at runtime.
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single(secret(0xF2))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
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
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
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
            // The member the supervisor stood up carries A's announce to B.
            let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("B hears A over the supervisor's member within 5s")
                .expect("the announce channel stays open");
            assert_eq!(heard, dest_a, "B heard A through the member the supervisor spawned");

            // Tear the *supervisor* down; the driver cascades the stop to its member, so B falls silent.
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
        () = node_a.run() => unreachable!("node A's run loop returned"),
        () = node_b.run() => unreachable!("node B's run loop returned"),
    }
}

/// The multi-client capstone: one `TcpServer` supervisor, two independent client nodes each dialing
/// it and announcing its own destination. The server must stand up a *distinct* member per client and
/// hear each announce on its own member — proving the fan-out the reference's spawned-child model
/// gives, not one connection masking the next.
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

    // The server node: no fixed interface, just the supervisor that stands up a member per client. It
    // reports each announce it hears together with the member interface it arrived on.
    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_s = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single(secret(0x55))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
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

    // Two independent client nodes, each dialing the one server and announcing its own destination.
    let client_a = TcpClientInterface::new(addr.clone(), BITRATE, Duration::from_millis(100));
    let node_a = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single_a],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![client_a],
        on_event: |_event, _state| {},
    });
    let commands_a = node_a.handle();

    let client_b = TcpClientInterface::new(addr, BITRATE, Duration::from_millis(100));
    let node_b = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single_b],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![client_b],
        on_event: |_event, _state| {},
    });
    let commands_b = node_b.handle();

    // Each client announces its own destination on a cadence until the test ends.
    for (commands, dest) in [(commands_a, dest_a), (commands_b, dest_b)] {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(200));
            loop {
                ticker.tick().await;
                if commands
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
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
            // The member each client's announce first arrived on; collect until both are heard.
            // (DestinationHash is Eq but not Hash, so two slots, not a map.)
            let mut member_a: Option<InterfaceId> = None;
            let mut member_b: Option<InterfaceId> = None;
            while member_a.is_none() || member_b.is_none() {
                let (destination, source_interface) =
                    tokio::time::timeout(Duration::from_secs(10), heard_rx.recv())
                        .await
                        .expect("the server hears both clients within 10s")
                        .expect("the announce channel stays open");
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
        () = node_s.run() => unreachable!("the server node's run loop returned"),
        () = node_a.run() => unreachable!("client A's run loop returned"),
        () = node_b.run() => unreachable!("client B's run loop returned"),
    }
}
