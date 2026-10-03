#![allow(clippy::unwrap_used, clippy::panic)]

extern crate alloc;

use personal_rns::engine::test_support::{
    TestStorageLayout, fixed_secret_key, routable_descriptor,
};
use personal_rns::engine::{
    CryptoOwed, Directive, EngineReaction, EngineState, IngestIo, Journaled, OwedWork,
};
use personal_rns::identity::IDENTITY_SECRET_KEY_LEN;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::interfaces::InterfaceId;
use personal_rns::interfaces::{AttachedInterfaces, InboundPacket};
use personal_rns::remote_control::{
    RemoteControlPairingAvailability, RemoteControlPairingAvailabilityDestination,
    RemoteControlPairingAvailabilityObservation, RemoteControlPairingAvailabilityVerification,
    RemoteControlPairingExpiresAfter, RemoteControlPairingPublicAppData,
};
use personal_rns::routing::announce::{AnnounceEntropy, AnnounceId};
use personal_rns::units::DurationMillis;
use personal_rns::units::InstantMillis;
use personal_rns::wire::{
    BROADCAST_MTU, ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader,
};

pub(crate) const USB: InterfaceId = InterfaceId::new([0xD0; 8]);
pub(crate) const OTHER: InterfaceId = InterfaceId::new([0xD1; 8]);

pub(crate) fn with_observation<R>(
    seed: u8,
    source: InterfaceId,
    observed: u64,
    lifetime: u64,
    accept: impl FnOnce(RemoteControlPairingAvailabilityObservation<'_>) -> R,
) -> R {
    let mut wire = availability_wire(seed, observed, lifetime);
    let mut engine = EngineState::<TestStorageLayout>::default();
    let identity = engine.hold_identity(fixed_secret_key()).unwrap();
    engine.configure_remote_control_pairing(identity).unwrap();
    let descriptors = [routable_descriptor(source)];
    let interfaces = AttachedInterfaces::new(&descriptors);
    let mut owed = None;
    engine.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(observed),
            source_interface: source,
            bytes: &mut wire,
        },
        IngestIo {
            interfaces,
            now: InstantMillis(observed),
            fill_random: &mut |_| {},
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if let EngineReaction::Directive(Directive::Fulfill(OwedWork::Crypto(
                    CryptoOwed::RemoteControlPairingAvailabilityVerify(work),
                ))) = reaction
                {
                    owed = Some(work);
                }
            },
        },
    );
    let RemoteControlPairingAvailabilityVerification::Verified(verified) = owed.unwrap().verify()
    else {
        panic!("signed availability did not verify");
    };
    let mut accept = Some(accept);
    let mut result = None;
    engine.resume_remote_control_pairing_availability(verified, interfaces, &mut |reaction| {
        if let EngineReaction::Journaled(Journaled::RemoteControlPairingAvailabilityObserved(
            observation,
        )) = reaction
        {
            result = Some(accept.take().unwrap()(observation));
        }
    });
    result.unwrap()
}

pub(crate) fn availability_wire(seed: u8, observed: u64, lifetime: u64) -> alloc::vec::Vec<u8> {
    let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[seed; IDENTITY_SECRET_KEY_LEN]);
    let mut wire = [0; BROADCAST_MTU];
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
    let header_len = header.write(&mut wire).unwrap();
    let payload_len = RemoteControlPairingAvailability::write_signed(
        &signer,
        AnnounceId::mint(
            AnnounceEntropy::new([seed; AnnounceEntropy::LEN]),
            InstantMillis(observed),
        ),
        RemoteControlPairingExpiresAfter::try_from(DurationMillis(lifetime)).unwrap(),
        RemoteControlPairingPublicAppData::empty(),
        wire.get_mut(header_len..).unwrap(),
    )
    .unwrap();
    wire.get(..header_len.saturating_add(payload_len))
        .unwrap()
        .to_vec()
}
