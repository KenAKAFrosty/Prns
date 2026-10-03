#![allow(clippy::unwrap_used, clippy::panic)]

extern crate alloc;

mod properties;
mod transitions;

use super::*;
use prns_core::engine::test_support::{TestStorageLayout, fixed_secret_key, routable_descriptor};
use prns_core::engine::{
    CryptoOwed, Directive, EngineReaction, EngineState, IngestIo, Journaled, OwedWork,
};
use prns_core::identity::IDENTITY_SECRET_KEY_LEN;
use prns_core::identity::in_memory::InMemoryNodeIdentity;
use prns_core::interfaces::{AttachedInterfaces, InboundPacket};
use prns_core::remote_control::{
    RemoteControlPairingAvailability, RemoteControlPairingAvailabilityDestination,
    RemoteControlPairingAvailabilityObservation, RemoteControlPairingAvailabilityVerification,
    RemoteControlPairingExpiresAfter, RemoteControlPairingPublicAppData,
};
use prns_core::routing::announce::{AnnounceEntropy, AnnounceId};
use prns_core::units::DurationMillis;
use prns_core::wire::{
    BROADCAST_MTU, ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader,
};

const USB: InterfaceId = InterfaceId::new([0xD0; 8]);
const OTHER: InterfaceId = InterfaceId::new([0xD1; 8]);

fn with_observation<R>(
    seed: u8,
    source: InterfaceId,
    observed: u64,
    lifetime: u64,
    accept: impl FnOnce(RemoteControlPairingAvailabilityObservation<'_>) -> R,
) -> R {
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
            bytes: wire
                .get_mut(..header_len.saturating_add(payload_len))
                .unwrap(),
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

fn observe<const N: usize>(
    discovery: &mut UsbPairingDiscovery<N>,
    seed: u8,
    source: InterfaceId,
    observed: u64,
    lifetime: u64,
) -> Result<ObserveUsbPairingAvailabilityOutcome, ObserveUsbPairingAvailabilityError> {
    with_observation(seed, source, observed, lifetime, |observation| {
        discovery.step(ObserveUsbPairingAvailability { observation })
    })
}

fn candidate(seed: u8, observed: u64, lifetime: u64) -> PairingCandidate {
    with_observation(seed, USB, observed, lifetime, |observation| {
        PairingCandidate::from(&observation)
    })
}

fn snapshot<const N: usize>(
    now: u64,
    candidates: &[PairingCandidate],
) -> UsbPairingCandidatesSnapshot<N> {
    UsbPairingCandidatesSnapshot {
        interface: USB,
        as_of: InstantMillis(now),
        candidates: heapless::Vec::from_slice(candidates).unwrap(),
    }
}
