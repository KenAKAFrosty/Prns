use super::*;
use core::num::NonZeroUsize;

use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::IDENTITY_SECRET_KEY_LEN;
use crate::remote_control::{
    RemoteControlPairingAvailability, RemoteControlPairingAvailabilityDestination,
    RemoteControlPairingExpiresAfter, RemoteControlPairingPublicAppData,
};
use crate::routing::announce::{AnnounceEntropy, AnnounceId};
use crate::units::{DurationMillis, HopCount, InstantMillis};
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PropagationType, WireContext, BROADCAST_MDU,
    BROADCAST_MTU,
};

const SOURCE: InterfaceId = InterfaceId::new([0xE1; 8]);

#[tokio::test]
async fn pooled_crypto_resumes_a_verified_pairing_availability_as_one_typed_observation() {
    let mut engine = EngineState::<TestStorageLayout>::default();
    assert_eq!(
        engine.configure_remote_control_pairing(),
        Ok(RemoteControlPairingAvailabilityDestination::canonical()),
    );
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (mut inbound_tx, inbound_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let app = move |journaled: Journaled<'_>| {
        if let Journaled::RemoteControlPairingAvailabilityObserved(observation) = journaled {
            let _ = observed_tx.send((
                observation.endpoint(),
                observation.observed_at(),
                observation.expires_at(),
                observation.hops(),
                observation.source_interface(),
                observation.public_app_data().as_bytes().to_vec(),
            ));
        }
    };

    tokio::spawn(run_with_store(
        engine,
        TokioHost::new(),
        ManifoldWiring {
            interfaces: std::vec![descriptor(SOURCE)],
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(SOURCE, inbound_rx)],
            commands: command_rx,
            egress: Egress::new(std::vec![]),
        },
        app,
        InterfaceStore::new(),
        CryptoPoolConfig::Pooled {
            workers: PoolWorkers::Fixed(NonZeroUsize::MIN),
        },
    ));

    let wire = pairing_availability_wire();
    inbound_tx.try_grant().unwrap().fill(&wire);
    inbound_tx.commit();
    notify_tx.send(SOURCE).unwrap();

    let (endpoint, observed_at, expires_at, hops, source_interface, public_app_data) =
        tokio::time::timeout(Duration::from_secs(2), observed_rx.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        (hops, source_interface, public_app_data),
        (HopCount(1), SOURCE, b"pool".to_vec()),
    );
    assert_eq!(
        expires_at,
        observed_at.saturating_add(DurationMillis(60_000)),
    );
    assert_ne!(
        endpoint.destination_hash(),
        RemoteControlPairingAvailabilityDestination::canonical().destination_hash(),
    );
}

fn pairing_availability_wire() -> std::vec::Vec<u8> {
    let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[0xE2; IDENTITY_SECRET_KEY_LEN]);
    let mut payload = [0u8; BROADCAST_MDU];
    let payload_len = RemoteControlPairingAvailability::write_signed(
        &signer,
        AnnounceId::mint(
            AnnounceEntropy::new([0xE3; AnnounceEntropy::LEN]),
            InstantMillis(1_000),
        ),
        RemoteControlPairingExpiresAfter::try_from(DurationMillis(60_000)).unwrap(),
        RemoteControlPairingPublicAppData::try_from(b"pool".as_slice()).unwrap(),
        &mut payload,
    )
    .unwrap();
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Plain,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: RemoteControlPairingAvailabilityDestination::canonical()
            .destination_hash()
            .to_address(),
        context: WireContext::None,
    };
    let mut wire = [0u8; BROADCAST_MTU];
    let header_len = header.write(&mut wire).unwrap();
    wire[header_len..header_len + payload_len].copy_from_slice(&payload[..payload_len]);
    wire[..header_len + payload_len].to_vec()
}
