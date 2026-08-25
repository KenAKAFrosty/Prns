use core::time::Duration;
use personal_rns::runtime::NoPersistence;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy, Settlement,
};
use personal_rns::identity::vault::FileVault;
use personal_rns::identity::{MarkDestinationUsedOutcome, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{BitrateBps, InterfaceId};
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::persistence::{read_tunnels_snapshot, FileStore, PersistedStore, SnapshotRegion};
use personal_rns::request_endpoints;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    boot_timeline_origin, DestinationIdentityRetentionControl, Diagnostic, FlushMark,
    ManuallyAttached, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, RegionFlush, RouteSeedProgress, ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::wire::DestinationHash;
use tokio::sync::mpsc::UnboundedReceiver;

const BITRATE: BitrateBps = BitrateBps::guess(1_000_000);
const ANNOUNCE_DELIVERY_DEADLINE: Duration = Duration::from_secs(5);
const SANITIZED_ANNOUNCE_DELIVERY_DEADLINE: Duration = Duration::from_secs(15);
const ANNOUNCE_RETRY_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug)]
enum AnnounceDeliveryFailure {
    Deadline,
    ChannelClosed,
    UnsuccessfulSettlement,
}

fn announce_delivery_deadline() -> Duration {
    if std::env::var_os("TSAN_OPTIONS").is_some() {
        SANITIZED_ANNOUNCE_DELIVERY_DEADLINE
    } else {
        ANNOUNCE_DELIVERY_DEADLINE
    }
}

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
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}

async fn hear_announced_destination(
    commands: &PrnsNodeHandle,
    heard_rx: &mut UnboundedReceiver<DestinationHash>,
    announce: &AnnounceNow,
) -> Result<(), AnnounceDeliveryFailure> {
    let deadline = tokio::time::Instant::now() + announce_delivery_deadline();
    let mut retry_at = tokio::time::Instant::now() + ANNOUNCE_RETRY_INTERVAL;
    loop {
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline) => {
                return Err(AnnounceDeliveryFailure::Deadline);
            }
            heard = heard_rx.recv() => {
                let Some(heard) = heard else {
                    return Err(AnnounceDeliveryFailure::ChannelClosed);
                };
                if heard == announce.destination {
                    break;
                }
            }
            () = tokio::time::sleep_until(retry_at) => {
                match tokio::time::timeout_at(
                    deadline,
                    commands.announce_now(announce.clone()),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => {
                        return Err(AnnounceDeliveryFailure::UnsuccessfulSettlement);
                    }
                    Err(_) => return Err(AnnounceDeliveryFailure::Deadline),
                }
                retry_at = tokio::time::Instant::now() + ANNOUNCE_RETRY_INTERVAL;
            }
        }
    }
    Ok(())
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
    let pinned = InterfaceId::new(*b"\x00persist");

    let single_a = single(secret(0xA1));
    let dest_a = single_a
        .destination_hash()
        .expect("the test destination name is valid");

    let flushed_high_water = {
        let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
            .await
            .expect("server binds");
        let addr = server.local_addr().expect("bound addr").to_string();
        let node_a = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [single(secret(0xA1))],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: |_event, _state| {},
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
        });
        let commands_a = node_a.handle();
        let _server_sup = commands_a.supervise(server);

        let client =
            TcpClientInterface::new_with_id(pinned, addr, BITRATE, ReconnectPolicy::STANDARD);
        let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
        let node_b = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [single(secret(0xB2))],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: move |event, _state| {
                if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event
                {
                    let _ = heard_tx.send(destination);
                }
            },
            interfaces: |node: &PrnsNodeHandle| {
                node.attach(client);
            },
            persistence: NoPersistence,
        })
        .with_timeline_origin(boot_timeline_origin(&store));
        let commands_b = node_b.handle();

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

        let hear_then_flush = async {
            let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                .await
                .expect("B hears A's announce within 5s")
                .expect("the announce channel stays open");
            assert_eq!(heard, dest_a);
            assert!(commands_b.retain_destination(dest_a).await.is_ok());
            commands_b
                .flush_to_store(&mut store)
                .await
                .expect("the flush lands both regions")
        };
        tokio::select! {
            biased;
            high_water = hear_then_flush => high_water,
            result = node_a.run() => unreachable!("node A's run loop returned: {result:?}"),
            result = node_b.run() => unreachable!("node B's run loop returned: {result:?}"),
        }
    };

    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("server rebinds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xA1))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    let client = TcpClientInterface::new_with_id(pinned, addr, BITRATE, ReconnectPolicy::STANDARD);
    let mut node_b = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xB2))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
    })
    .with_timeline_origin(boot_timeline_origin(&store));

    let origin = boot_timeline_origin(&store);
    assert!(
        origin >= flushed_high_water,
        "the resumed timeline never rewinds under the flushed high-water",
    );

    let mut route_progress = Vec::new();
    let report = node_b.seed_routes_from_store_reporting(&store, |progress| {
        route_progress.push(progress);
    });
    assert_eq!(
        route_progress,
        vec![
            RouteSeedProgress {
                processed_count: 0,
                total_count: 1,
            },
            RouteSeedProgress {
                processed_count: 1,
                total_count: 1,
            },
        ]
    );
    assert_eq!(report.seeded_count, 1, "A's route seeds from the snapshot");
    assert_eq!(report.refused_count, 0);
    assert_eq!(report.dropped_count, 0);
    let destination_identities = node_b.seed_destination_identities_from_store(&store);
    assert_eq!(destination_identities.seeded_count, 1);
    assert_eq!(destination_identities.refused_count, 0);
    assert_eq!(destination_identities.dropped_count, 0);

    let commands_b = node_b.handle();
    let proven = async {
        assert_eq!(
            commands_b.mark_destination_used(dest_a).await,
            Ok(MarkDestinationUsedOutcome::Retained),
        );
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
        result = node_a.run() => unreachable!("node A's run loop returned: {result:?}"),
        result = node_b.run() => unreachable!("node B's run loop returned: {result:?}"),
    };
    let _ = receipt;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconnecting_peer_reclaims_a_rebooted_relays_routes_through_its_tunnel() {
    let dir = StoreDir::new("tunnel");
    let mut store = FileStore::new(&dir.path);
    let pinned = InterfaceId::new(*b"\x00tunnel\x00");

    let single_c = single(secret(0xC5));
    let dest_c = single_c
        .destination_hash()
        .expect("the test destination name is valid");

    {
        let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
            .await
            .expect("server binds");
        let addr = server.local_addr().expect("bound addr").to_string();
        let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
        let relay = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [single(secret(0xB2))],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: move |event, _state| {
                if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event
                {
                    let _ = heard_tx.send(destination);
                }
            },
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
        })
        .with_timeline_origin(boot_timeline_origin(&store));
        let commands_relay = relay.handle();
        let _server_sup = commands_relay.supervise(server);

        let client =
            TcpClientInterface::new_with_id(pinned, addr, BITRATE, ReconnectPolicy::STANDARD);
        let node_c = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: Some(secret(0x77)),
            pre_configured_destinations: [single(secret(0xC5))],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: |_event, _state| {},
            interfaces: |node: &PrnsNodeHandle| {
                node.attach(client);
            },
            persistence: NoPersistence,
        });
        let commands_c = node_c.handle();

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(200));
            loop {
                ticker.tick().await;
                if commands_c
                    .issue(PrnsCommand::AnnounceNow(AnnounceNow {
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
            result = relay.run() => unreachable!("the relay's run loop returned: {result:?}"),
            result = node_c.run() => unreachable!("node C's run loop returned: {result:?}"),
        }
    }

    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("server rebinds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let mut relay = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xB2))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
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

    let client = TcpClientInterface::new_with_id(pinned, addr, BITRATE, ReconnectPolicy::STANDARD);
    let node_c = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(secret(0x77)),
        pre_configured_destinations: [single(secret(0xC5))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
    });

    let proven = async {
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
        result = relay.run() => unreachable!("the relay's run loop returned: {result:?}"),
        result = node_c.run() => unreachable!("node C's run loop returned: {result:?}"),
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

    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let (settled_tx, mut settled_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xA1)), single(secret(0xA3))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) = event {
                let _ = settled_tx.send((id, settlement));
            }
        },
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let commands_a = node_a.handle();
    let _server_sup = commands_a.supervise(server);

    let client = TcpClientInterface::new_with_bitrate(addr, BITRATE, ReconnectPolicy::STANDARD);
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
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
    let commands_b = node_b.handle();

    let choreography = async {
        let first_announce = AnnounceNow {
            destination: dest_a1,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };
        commands_a
            .announce_now(first_announce.clone())
            .await
            .expect("the first announce settles successfully");
        hear_announced_destination(&commands_a, &mut heard_rx, &first_announce)
            .await
            .expect("B hears the first announce within the delivery deadline");

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
        assert_eq!(first.destination_identities, RegionFlush::Wrote);

        let quiet = commands_b
            .flush_changed_to_store(&mut store, &mut mark)
            .await
            .expect("the quiet flush lands");
        assert_eq!(quiet.routing_table, RegionFlush::UnchangedSkipped);
        assert_eq!(quiet.tunnels, RegionFlush::UnchangedSkipped);
        assert_eq!(quiet.destination_identities, RegionFlush::UnchangedSkipped);
        assert!(quiet.high_water >= first.high_water);

        let second_announce = AnnounceNow {
            destination: dest_a2,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };
        let second_announce_id = commands_a
            .issue(PrnsCommand::AnnounceNow(second_announce.clone()))
            .expect("node A accepts the second announce");
        loop {
            let (id, settlement) = settled_rx
                .recv()
                .await
                .expect("node A's settlement channel stays open");
            if id == second_announce_id {
                assert_eq!(settlement, Settlement::AnnounceNow(Ok(())));
                break;
            }
        }
        hear_announced_destination(&commands_a, &mut heard_rx, &second_announce)
            .await
            .expect("B hears the second announce within the delivery deadline");

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
        assert_eq!(changed.destination_identities, RegionFlush::Wrote);
    };
    tokio::select! {
        biased;
        () = choreography => {}
        result = node_a.run() => unreachable!("node A's run loop returned: {result:?}"),
        result = node_b.run() => unreachable!("node B's run loop returned: {result:?}"),
    }
}

fn ratcheted(
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "persist",
        aspects: &["ratcheted"],
        identity,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::Ratcheted,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebooted_destination_decrypts_singles_sealed_to_its_pre_reboot_ratchet() {
    let dir = StoreDir::new("ratchet");
    let mut store_p = FileStore::new(dir.path.join("peer"));
    let mut store_r = FileStore::new(dir.path.join("relay"));
    let mut vault_r = FileVault::new(dir.path.join("vault"));
    let pinned = InterfaceId::new(*b"\x00ratchet");

    let dest_r = ratcheted(secret(0xD1))
        .destination_hash()
        .expect("the test destination name is valid");

    {
        let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
            .await
            .expect("server binds");
        let addr = server.local_addr().expect("bound addr").to_string();
        let node_r = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [ratcheted(secret(0xD1))],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: |_event, _state| {},
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
        })
        .with_timeline_origin(boot_timeline_origin(&store_r));
        let commands_r = node_r.handle();
        let _server_sup = commands_r.supervise(server);

        let client =
            TcpClientInterface::new_with_id(pinned, addr, BITRATE, ReconnectPolicy::STANDARD);
        let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
        let node_p = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [single(secret(0xB2))],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: move |event, _state| {
                if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event
                {
                    let _ = heard_tx.send(destination);
                }
            },
            interfaces: |node: &PrnsNodeHandle| {
                node.attach(client);
            },
            persistence: NoPersistence,
        });
        let commands_p = node_p.handle();

        let announce_r = commands_r.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(200));
            loop {
                ticker.tick().await;
                if announce_r
                    .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                        destination: dest_r,
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
                .expect("P hears R's announce within 5s")
                .expect("the announce channel stays open");
            assert_eq!(heard, dest_r);
            let flushed = commands_r
                .flush_ratchets_to_vault(&mut vault_r)
                .await
                .expect("the ratchet flush lands");
            assert_eq!(flushed, 1, "R's one ratcheted destination flushes");
            commands_r
                .flush_to_store(&mut store_r)
                .await
                .expect("R's timebase flush lands");
            commands_p
                .flush_to_store(&mut store_p)
                .await
                .expect("P's flush lands");
        };
        tokio::select! {
            biased;
            () = hear_then_flush => {}
            result = node_r.run() => unreachable!("node R's run loop returned: {result:?}"),
            result = node_p.run() => unreachable!("node P's run loop returned: {result:?}"),
        }
    }

    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE)
        .await
        .expect("server rebinds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let mut node_r = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [ratcheted(secret(0xD1))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    })
    .with_timeline_origin(boot_timeline_origin(&store_r));
    let ratchets = node_r.seed_self_ratchets_from_vault(&vault_r);
    assert_eq!(ratchets.seeded_count, 1, "R's ratchet record seeds");
    assert_eq!(ratchets.refused_count, 0);
    assert_eq!(ratchets.dropped_count, 0);
    let commands_r = node_r.handle();
    let _server_sup = commands_r.supervise(server);

    let client = TcpClientInterface::new_with_id(pinned, addr, BITRATE, ReconnectPolicy::STANDARD);
    let mut node_p = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [single(secret(0xB2))],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
    });
    let routes = node_p.seed_routes_from_store(&store_p);
    assert_eq!(routes.seeded_count, 1, "R's route seeds on P");
    let commands_p = node_p.handle();

    let proven = async {
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        loop {
            ticker.tick().await;
            let attempt = commands_p.send_single_packet(dest_r, b"sealed to the old ratchet");
            if let Ok(Ok(receipt)) = tokio::time::timeout(Duration::from_millis(900), attempt).await
            {
                break receipt;
            }
        }
    };
    let receipt = tokio::select! {
        biased;
        receipt = tokio::time::timeout(Duration::from_secs(10), proven) => {
            receipt.expect("the ratchet-sealed single proves within 10s")
        }
        result = node_r.run() => unreachable!("node R's run loop returned: {result:?}"),
        result = node_p.run() => unreachable!("node P's run loop returned: {result:?}"),
    };
    let _ = receipt;
}
