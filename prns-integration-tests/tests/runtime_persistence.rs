//! The persistence arc end to end: node B learns A's route over live TCP, flushes its routing
//! table and timebase through the handle, "reboots" as a fresh process-equivalent (new engine, new
//! reactor, resumed timeline origin), seeds itself from the store, and then reaches A with a
//! proven single packet — no announce ever crosses in phase two, so the route, A's public keys,
//! and the timeline all came from the snapshot or the packet could not have been built.
//! The tunnel capstone closes the ephemeral-client gap on top: the relay's per-connection
//! interface id never comes back after its reboot, so only the peer's reconnect synthesize —
//! matched against the seeded tunnel — can repoint the seeded route onto the live connection.

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{BitrateBps, InterfaceId};
use personal_rns::persistence::{read_tunnels_snapshot, FileStore, PersistedStore, SnapshotRegion};
use personal_rns::routes;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    boot_timeline_origin, Diagnostic, FlushMark, Manual, PreConfiguredDestination, Prns, PrnsEvent,
    PrnsRecipe, RegionFlush, TokioPrnsHandle,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::client::TcpClientInterface;
use personal_rns::tcp::server::TcpServer;

const BITRATE: BitrateBps = BitrateBps::guess(1_000_000);

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

fn single(identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "persist",
        aspects: &["capstone"],
        identity,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    }
}

struct StoreDir {
    path: std::path::PathBuf,
}

impl StoreDir {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("prns-persist-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }
}

impl Drop for StoreDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebooted_node_reaches_a_peer_from_its_seeded_snapshot_alone() {
    let dir = StoreDir::new("snapshot");
    let mut store = FileStore::new(&dir.path);
    // B pins its client interface id so the reboot re-derives the same id even though the
    // rebuilt server binds a fresh port; a config-tagged interface gets this for free.
    let pinned = InterfaceId::new(*b"\x00persist");

    let single_a = single(secret(0xA1));
    let dest_a = single_a
        .destination_hash()
        .expect("the test destination name is valid");

    // Phase one: B (wall-anchored timeline) hears A's announce, then flushes and "powers off".
    let flushed_high_water = {
        let server = TcpServer::bind("127.0.0.1:0", BITRATE)
            .await
            .expect("server binds");
        let addr = server.local_addr().expect("bound addr").to_string();
        let node_a = Prns::new(PrnsRecipe {
            transport_identity: None,
            pre_configured_destinations: [single(secret(0xA1))],
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            on_event: |_event, _state| {},
            interfaces: Manual,
        });
        let commands_a = node_a.handle();
        let _server_sup = commands_a.supervise(server);

        let client =
            TcpClientInterface::new_with_id(pinned, addr, BITRATE, Duration::from_millis(100));
        let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
        let node_b = Prns::new(PrnsRecipe {
            transport_identity: None,
            pre_configured_destinations: [single(secret(0xB2))],
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            on_event: move |event, _state| {
                if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event
                {
                    let _ = heard_tx.send(destination);
                }
            },
            interfaces: |node: &TokioPrnsHandle| {
                node.attach(client);
            },
        })
        .with_timeline_origin(boot_timeline_origin(&store));
        let commands_b = node_b.handle();

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

        let hear_then_flush = async {
            let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("B hears A's announce within 5s")
                .expect("the announce channel stays open");
            assert_eq!(heard, dest_a);
            commands_b
                .flush_to_store(&mut store)
                .await
                .expect("the flush lands both regions")
        };
        tokio::select! {
            biased;
            high_water = hear_then_flush => high_water,
            () = node_a.run() => unreachable!("node A's run loop returned"),
            () = node_b.run() => unreachable!("node B's run loop returned"),
        }
    };

    // Phase two: everything from phase one is dropped. A returns on a fresh port and never
    // announces; B reboots from the store alone.
    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server rebinds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = Prns::new(PrnsRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xA1))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: |_event, _state| {},
        interfaces: Manual,
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    let client = TcpClientInterface::new_with_id(pinned, addr, BITRATE, Duration::from_millis(100));
    let mut node_b = Prns::new(PrnsRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xB2))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: |_event, _state| {},
        interfaces: |node: &TokioPrnsHandle| {
            node.attach(client);
        },
    })
    .with_timeline_origin(boot_timeline_origin(&store));

    let origin = boot_timeline_origin(&store);
    assert!(
        origin >= flushed_high_water,
        "the resumed timeline never rewinds under the flushed high-water",
    );

    let report = node_b.seed_routes_from_store(&store);
    assert_eq!(report.seeded_count, 1, "A's route seeds from the snapshot");
    assert_eq!(report.refused_count, 0);
    assert_eq!(report.dropped_count, 0);

    let commands_b = node_b.handle();
    let proven = async {
        // The pinned interface reconnects on its own clock, so retry until the proof lands.
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        loop {
            ticker.tick().await;
            if let Ok(receipt) = commands_b
                .send_single_packet(dest_a, b"from the snapshot")
                .await
            {
                break receipt;
            }
        }
    };
    let receipt = tokio::select! {
        biased;
        receipt = tokio::time::timeout(Duration::from_secs(10), proven) => {
            receipt.expect("the seeded route carries a proven single within 10s")
        }
        () = node_a.run() => unreachable!("node A's run loop returned"),
        () = node_b.run() => unreachable!("node B's run loop returned"),
    };
    let _ = receipt;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconnecting_peer_reclaims_a_rebooted_relays_routes_through_its_tunnel() {
    let dir = StoreDir::new("tunnel");
    let mut store = FileStore::new(&dir.path);
    // C pins its own client id: in production the dial target is fixed so the id derives stably,
    // and a stable id is what keeps C's tunnel_id identical across the relay's reboot. The
    // relay-side interface C arrives on is pinned by nothing — it derives from C's ephemeral
    // source port, and its disappearance at reboot is exactly the gap the tunnel closes.
    let pinned = InterfaceId::new(*b"\x00tunnel\x00");

    let single_c = single(secret(0xC5));
    let dest_c = single_c
        .destination_hash()
        .expect("the test destination name is valid");

    // Phase one: the relay hears C's announce and C's connect-time tunnel synthesize, flushes
    // until the store holds the tunnel row, and powers off.
    {
        let server = TcpServer::bind("127.0.0.1:0", BITRATE)
            .await
            .expect("server binds");
        let addr = server.local_addr().expect("bound addr").to_string();
        let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
        let relay = Prns::new(PrnsRecipe {
            transport_identity: None,
            pre_configured_destinations: [single(secret(0xB2))],
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            on_event: move |event, _state| {
                if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event
                {
                    let _ = heard_tx.send(destination);
                }
            },
            interfaces: Manual,
        })
        .with_timeline_origin(boot_timeline_origin(&store));
        let commands_relay = relay.handle();
        let _server_sup = commands_relay.supervise(server);

        let client =
            TcpClientInterface::new_with_id(pinned, addr, BITRATE, Duration::from_millis(100));
        let node_c = Prns::new(PrnsRecipe {
            transport_identity: Some(secret(0x77)),
            pre_configured_destinations: [single(secret(0xC5))],
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            on_event: |_event, _state| {},
            interfaces: |node: &TokioPrnsHandle| {
                node.attach(client);
            },
        });
        let commands_c = node_c.handle();

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(200));
            loop {
                ticker.tick().await;
                if commands_c
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination: dest_c,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });

        let hear_then_flush = async {
            let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("the relay hears C's announce within 5s")
                .expect("the announce channel stays open");
            assert_eq!(heard, dest_c);
            // The synthesize left C before its first announce, but flush until the store proves
            // it landed rather than assuming the ordering.
            let flush_until_tunnel_stored = async {
                let mut ticker = tokio::time::interval(Duration::from_millis(100));
                loop {
                    ticker.tick().await;
                    commands_relay
                        .flush_to_store(&mut store)
                        .await
                        .expect("the flush lands all regions");
                    let Ok(Some(len)) = store.stored_len(SnapshotRegion::Tunnels) else {
                        continue;
                    };
                    let mut buf = vec![0u8; len];
                    let Ok(Some(bytes)) = store.load(SnapshotRegion::Tunnels, &mut buf) else {
                        continue;
                    };
                    let stored_rows = read_tunnels_snapshot(bytes)
                        .map(|rows| rows.row_count())
                        .unwrap_or(0);
                    if stored_rows == 1 {
                        break;
                    }
                }
            };
            tokio::time::timeout(Duration::from_secs(5), flush_until_tunnel_stored)
                .await
                .expect("the tunnel row reaches the store within 5s");
        };
        tokio::select! {
            biased;
            () = hear_then_flush => {}
            () = relay.run() => unreachable!("the relay's run loop returned"),
            () = node_c.run() => unreachable!("node C's run loop returned"),
        }
    }

    // Phase two: the relay reboots from the store alone; C reconnects on a fresh source port and
    // never announces, so only the seeded tunnel catching C's synthesize can revive the route.
    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server rebinds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let mut relay = Prns::new(PrnsRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xB2))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: |_event, _state| {},
        interfaces: Manual,
    })
    .with_timeline_origin(boot_timeline_origin(&store));

    let routes = relay.seed_routes_from_store(&store);
    assert_eq!(routes.seeded_count, 1, "C's route seeds from the snapshot");
    let tunnels = relay.seed_tunnels_from_store(&store);
    assert_eq!(
        tunnels.seeded_count, 1,
        "C's tunnel seeds from the snapshot"
    );
    assert_eq!(tunnels.refused_count, 0);
    assert_eq!(tunnels.dropped_count, 0);

    let commands_relay = relay.handle();
    let _server_sup = commands_relay.supervise(server);

    let client = TcpClientInterface::new_with_id(pinned, addr, BITRATE, Duration::from_millis(100));
    let node_c = Prns::new(PrnsRecipe {
        transport_identity: Some(secret(0x77)),
        pre_configured_destinations: [single(secret(0xC5))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: |_event, _state| {},
        interfaces: |node: &TokioPrnsHandle| {
            node.attach(client);
        },
    });

    let proven = async {
        // Until C's reconnect synthesize repoints it, the seeded route points at phase one's
        // dead relay-side interface: a send there is accepted, dropped at egress, and its
        // receipt waits out the full retry ladder — so time-box each attempt and try again.
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        loop {
            ticker.tick().await;
            let attempt = commands_relay.send_single_packet(dest_c, b"over the reclaimed tunnel");
            if let Ok(Ok(receipt)) = tokio::time::timeout(Duration::from_millis(900), attempt).await
            {
                break receipt;
            }
        }
    };
    let receipt = tokio::select! {
        biased;
        receipt = tokio::time::timeout(Duration::from_secs(10), proven) => {
            receipt.expect("the reclaimed route carries a proven single within 10s")
        }
        () = relay.run() => unreachable!("the relay's run loop returned"),
        () = node_c.run() => unreachable!("node C's run loop returned"),
    };
    let _ = receipt;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quiet_flush_skips_unchanged_regions_and_a_change_rewrites() {
    let dir = StoreDir::new("changed");
    let mut store = FileStore::new(&dir.path);

    let dest_a1 = single(secret(0xA1))
        .destination_hash()
        .expect("the test destination name is valid");
    let dest_a2 = single(secret(0xA3))
        .destination_hash()
        .expect("the test destination name is valid");

    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = Prns::new(PrnsRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xA1)), single(secret(0xA3))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: |_event, _state| {},
        interfaces: Manual,
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    let client = TcpClientInterface::new(addr, BITRATE, Duration::from_millis(100));
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = Prns::new(PrnsRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xB2))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: |node: &TokioPrnsHandle| {
            node.attach(client);
        },
    });
    let commands_b = node_b.handle();

    let announce_first = commands_a.clone();
    let announcer = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announce_first
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a1,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    let choreography = async {
        loop {
            let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("B hears the first announce within 5s")
                .expect("the announce channel stays open");
            if heard == dest_a1 {
                break;
            }
        }
        // Stop re-announcing so the table sits still, and let any in-flight announce land
        // before the flush pair whose second half asserts nothing changed.
        announcer.abort();
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut mark = FlushMark::default();
        let first = commands_b
            .flush_changed_to_store(&mut store, &mut mark)
            .await
            .expect("the first flush lands");
        assert_eq!(
            first.routing_table,
            RegionFlush::Wrote,
            "a fresh mark writes"
        );
        assert_eq!(first.tunnels, RegionFlush::Wrote);

        let quiet = commands_b
            .flush_changed_to_store(&mut store, &mut mark)
            .await
            .expect("the quiet flush lands");
        assert_eq!(quiet.routing_table, RegionFlush::UnchangedSkipped);
        assert_eq!(quiet.tunnels, RegionFlush::UnchangedSkipped);
        assert!(quiet.high_water >= first.high_water);

        commands_a.issue(EngineCommand::AnnounceNow(AnnounceNow {
            destination: dest_a2,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        }));
        loop {
            let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("B hears the second announce within 5s")
                .expect("the announce channel stays open");
            if heard == dest_a2 {
                break;
            }
        }

        let changed = commands_b
            .flush_changed_to_store(&mut store, &mut mark)
            .await
            .expect("the post-change flush lands");
        assert_eq!(
            changed.routing_table,
            RegionFlush::Wrote,
            "a new route rewrites the routing region",
        );
        assert_eq!(
            changed.tunnels,
            RegionFlush::UnchangedSkipped,
            "the untouched tunnels region still skips",
        );
    };
    tokio::select! {
        biased;
        () = choreography => {}
        () = node_a.run() => unreachable!("node A's run loop returned"),
        () = node_b.run() => unreachable!("node B's run loop returned"),
    }
}
