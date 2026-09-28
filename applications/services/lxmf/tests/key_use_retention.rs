#![cfg(feature = "tokio-host")]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::Path;
use std::time::Duration;

use personal_rns::identity::destination_identity::{
    DestinationIdentities, DestinationIdentityRetentionState, HeapDestinationIdentityTable,
    UNUSED_DESTINATION_LINGER_MILLIS, USED_DESTINATION_LINGER_MILLIS,
};
use personal_rns::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceDescriptor, InterfaceGravity, InterfaceId, InterfaceKind,
    InterfaceMode, ReportsStatus, TransportCapability,
};
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use personal_rns::persistence::{
    read_destination_identities_snapshot, FileStore, PersistedStore, SnapshotRegion,
};
use personal_rns::routing::announce::stored::HeapAnnounceAppData;
use personal_rns::runtime::RoutingControl;
use personal_rns::units::InstantMillis;
use personal_rns::wire::{DestinationHash, BROADCAST_MTU};
use prns_host::{
    DestinationConfig, HostConfig, HostRole, IdentityConfig, PersistenceConfig, PrnsLimits,
};
use prns_host_native::owner::OwnedSession;
use prns_host_native::{NativeEmbedding, NativePreparedAttachment};
use prns_lxmf::direct::{prepare_local_lxmf_destination, DirectNetwork, PrnsDirectNetwork};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(5);

struct AnnounceInterface {
    descriptor: InterfaceDescriptor,
    inbound: mpsc::UnboundedReceiver<Vec<u8>>,
    outbound: mpsc::UnboundedSender<Vec<u8>>,
}

impl Interface for AnnounceInterface {
    const HW_MTU: usize = BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Loopback;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn channel_tag(&self) -> &[u8] {
        self.descriptor.id.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        loop {
            tokio::select! {
                inbound = self.inbound.recv() => match inbound {
                    Some(bytes) => seam.next_inbound(&bytes).await,
                    None => return,
                },
                outbound = seam.next_outbound() => {
                    let _sent = self.outbound.send(outbound.to_vec());
                }
            }
        }
    }
}

impl ReportsStatus for AnnounceInterface {}

fn descriptor(id: u8) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id: InterfaceId::new([id; 8]),
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        gravity: InterfaceGravity::ZERO,
        bitrate: BitrateBps::guess(1_000_000),
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: InterfaceCommonPolicy::RNS_DEFAULT,
    }
}

fn interface_pair() -> (AnnounceInterface, AnnounceInterface) {
    let (to_sender, sender_inbound) = mpsc::unbounded_channel();
    let (to_receiver, receiver_inbound) = mpsc::unbounded_channel();
    (
        AnnounceInterface {
            descriptor: descriptor(0xA3),
            inbound: sender_inbound,
            outbound: to_receiver,
        },
        AnnounceInterface {
            descriptor: descriptor(0xB4),
            inbound: receiver_inbound,
            outbound: to_sender,
        },
    )
}

fn config(destinations: Vec<DestinationConfig>, directory: Option<&Path>) -> HostConfig {
    HostConfig {
        identity: IdentityConfig::GenerateEphemeral,
        persistence: directory.map_or(PersistenceConfig::Ephemeral, |path| {
            PersistenceConfig::Directory {
                path: path.to_str().unwrap().to_owned(),
            }
        }),
        role: HostRole::Endpoint,
        destinations,
        required_capabilities: vec![],
        limits: PrnsLimits::balanced(),
    }
}

fn embedding(interface: AnnounceInterface) -> NativeEmbedding {
    NativeEmbedding {
        prepare_interfaces: Some(Box::new(move |client| {
            Ok(vec![NativePreparedAttachment::Interface {
                attachment: client.protocols().add_interface(interface),
                kind: prns_host::InterfaceKind::Pipe,
            }])
        })),
        ..NativeEmbedding::default()
    }
}

async fn wait_for_key(network: &PrnsDirectNetwork, destination: [u8; 16], public_key: [u8; 64]) {
    tokio::time::timeout(WAIT, async {
        loop {
            if network.destination_public_key(destination).await == Some(public_key)
                && network.has_route(destination).await
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a real authenticated announce supplies the key and route");
}

#[tokio::test(flavor = "current_thread")]
async fn production_key_use_is_bounded_persisted_and_restored_without_a_route() {
    let directory = tempfile::tempdir().unwrap();
    let (used_identity, used_destination) =
        prepare_local_lxmf_destination(personal_rns::identity::Zeroizing::new([0x35; 64]), &[])
            .unwrap();
    let (unused_identity, unused_destination) =
        prepare_local_lxmf_destination(personal_rns::identity::Zeroizing::new([0x57; 64]), &[])
            .unwrap();
    let used_hash = used_identity.destination();
    let unused_hash = unused_identity.destination();
    let (sender_interface, receiver_interface) = interface_pair();
    let receiver = OwnedSession::open_with_embedding(
        config(vec![], Some(directory.path())),
        embedding(receiver_interface),
    )
    .await
    .unwrap();
    let sender = OwnedSession::open_with_embedding(
        config(vec![used_destination, unused_destination], None),
        embedding(sender_interface),
    )
    .await
    .unwrap();
    let sender_network = PrnsDirectNetwork::new(sender.client());
    let receiver_network = PrnsDirectNetwork::new(receiver.client());
    sender_network.announce(used_hash).await.unwrap();
    sender_network.announce(unused_hash).await.unwrap();
    wait_for_key(&receiver_network, used_hash, used_identity.public_key()).await;
    wait_for_key(&receiver_network, unused_hash, unused_identity.public_key()).await;

    let services = receiver.client().native_services().unwrap();
    let before_use = services.clock().now();
    tokio::time::timeout(WAIT, receiver_network.mark_destination_used(used_hash))
        .await
        .expect("production adapter settles the host retention command");
    let after_use = services.clock().now();
    for destination in [used_hash, unused_hash] {
        services
            .protocols()
            .drop_route(DestinationHash::new(destination))
            .await
            .unwrap();
        assert!(!receiver_network.has_route(destination).await);
    }
    sender.stop().await.unwrap();
    receiver.stop().await.unwrap();
    tokio::time::timeout(WAIT, receiver_network.mark_destination_used(used_hash))
        .await
        .expect("marking use after host shutdown is harmless");

    // Inspect only what the real host wrote; no persistence bytes or key rows are edited.
    let store = FileStore::new(directory.path());
    let region = SnapshotRegion::DestinationIdentities;
    let mut bytes = vec![0; store.stored_len(region).unwrap().unwrap()];
    let snapshot = store.load(region, &mut bytes).unwrap().unwrap();
    let rows = read_destination_identities_snapshot(snapshot)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let used = rows
        .iter()
        .find(|row| row.destination == DestinationHash::new(used_hash))
        .unwrap();
    let unused = rows
        .iter()
        .find(|row| row.destination == DestinationHash::new(unused_hash))
        .unwrap();
    assert_eq!(
        used.public_keys.public_key_bytes(),
        used_identity.public_key()
    );
    assert_eq!(
        unused.retention,
        DestinationIdentityRetentionState::NeverUsed
    );
    let DestinationIdentityRetentionState::UsedAt(used_at) = used.retention else {
        panic!("key use must persist finite UsedAt retention, never a permanent pin");
    };
    assert!(before_use <= used_at && used_at <= after_use);

    // Native hosts own separate Tokio runtimes, so caller paused time cannot advance them.
    // Exercise the public retention policy with exact persisted rows and controlled time.
    let mut retained =
        DestinationIdentities::<HeapDestinationIdentityTable, HeapAnnounceAppData>::default();
    for row in [used, unused] {
        retained
            .restore(
                row.destination,
                row.public_keys,
                row.app_data,
                row.announced_at,
                row.retention,
            )
            .unwrap();
    }
    let after_unused_expiry = InstantMillis(
        used.announced_at.0.max(unused.announced_at.0) + UNUSED_DESTINATION_LINGER_MILLIS + 1,
    );
    assert_eq!(retained.cull_expired(after_unused_expiry, |_| false), 1);
    assert!(retained.get(&unused.destination).is_none());
    assert!(retained.get(&used.destination).is_some());
    let used_expiry = InstantMillis(used_at.0 + USED_DESTINATION_LINGER_MILLIS + 1);
    assert_eq!(retained.expiry_at(&used.destination), Some(used_expiry));
    assert_eq!(
        retained.cull_expired(InstantMillis(used_expiry.0 - 1), |_| false),
        0
    );
    assert_eq!(retained.cull_expired(used_expiry, |_| false), 1);

    let restarted = OwnedSession::open(config(vec![], Some(directory.path())))
        .await
        .unwrap();
    let restarted_network = PrnsDirectNetwork::new(restarted.client());
    assert!(!restarted_network.has_route(used_hash).await);
    assert_eq!(
        restarted_network.destination_public_key(used_hash).await,
        Some(used_identity.public_key()),
    );
    restarted.stop().await.unwrap();
}
