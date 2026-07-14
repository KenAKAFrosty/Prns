//! The persistence arc end to end: node B learns A's route over live TCP, flushes its routing
//! table and timebase through the handle, "reboots" as a fresh process-equivalent (new engine, new
//! reactor, resumed timeline origin), seeds itself from the store, and then reaches A with a
//! proven single packet — no announce ever crosses in phase two, so the route, A's public keys,
//! and the timeline all came from the snapshot or the packet could not have been built.

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{BitrateBps, InterfaceId};
use personal_rns::persistence::FileStore;
use personal_rns::routes;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    boot_timeline_origin, Diagnostic, Manual, PreConfiguredDestination, Prns, PrnsEvent,
    PrnsRecipe, TokioPrnsHandle,
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
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("prns-persist-{}", std::process::id()));
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
    let dir = StoreDir::new();
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

        let client = TcpClientInterface::new_with_id(
            pinned,
            addr,
            BITRATE,
            Duration::from_millis(100),
        );
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
            if let Ok(receipt) = commands_b.send_single_packet(dest_a, b"from the snapshot").await
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
