use crate::engine::test_support::{fixed_secret_key, routable_descriptor, TestStorageLayout};
use crate::engine::{
    CommandId, DeferredCrypto, Directive, EngineReaction, EngineState, IgnoreReason, IngestIo,
    IngestPacketOutcome, Journaled, PathRequestId, PathRequestWriteOutcome, RequestPath,
    WakeSchedule,
};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::IDENTITY_SECRET_KEY_LEN;
use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceId};
use crate::remote_control::{
    RemoteControlPairingAvailability, RemoteControlPairingAvailabilityDestination,
    RemoteControlPairingAvailabilityVerification, RemoteControlPairingExpiresAfter,
    RemoteControlPairingPublicAppData,
};
use crate::routing::announce::{AnnounceEntropy, AnnounceId};
use crate::routing::{NextHop, RouteRemovalCause, RouteResponsiveness, RouteRetention};
use crate::units::{DurationMillis, InstantMillis};
use crate::wire::{
    wire_hop_count_is_valid, ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WirePacketHeader, BROADCAST_MDU, BROADCAST_MTU,
};

const SOURCE: InterfaceId = InterfaceId::new([0xD1; 8]);
const OBSERVED_AT: InstantMillis = InstantMillis(8_000);

fn availability_payload() -> ([u8; BROADCAST_MDU], usize) {
    let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[0xD2; IDENTITY_SECRET_KEY_LEN]);
    let mut payload = [0u8; BROADCAST_MDU];
    let len = RemoteControlPairingAvailability::write_signed(
        &signer,
        AnnounceId::mint(
            AnnounceEntropy::new([0xD3; AnnounceEntropy::LEN]),
            InstantMillis(1_000),
        ),
        RemoteControlPairingExpiresAfter::try_from(DurationMillis(60_000)).unwrap(),
        RemoteControlPairingPublicAppData::try_from(b"nearby".as_slice()).unwrap(),
        &mut payload,
    )
    .unwrap();
    (payload, len)
}

fn canonical_outer_header() -> WirePacketHeader {
    WirePacketHeader {
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
    }
}

fn availability_wire(header: WirePacketHeader) -> std::vec::Vec<u8> {
    let (payload, payload_len) = availability_payload();
    availability_wire_with_payload(header, &payload[..payload_len])
}

fn availability_wire_with_payload(header: WirePacketHeader, payload: &[u8]) -> std::vec::Vec<u8> {
    let mut wire = [0u8; BROADCAST_MTU];
    let header_len = header.write(&mut wire).unwrap();
    wire.get_mut(header_len..header_len + payload.len())
        .unwrap()
        .copy_from_slice(payload);
    wire.get(..header_len + payload.len()).unwrap().to_vec()
}

fn configured_engine() -> EngineState<TestStorageLayout> {
    let mut engine = EngineState::default();
    let target_identity = engine.hold_identity(fixed_secret_key()).unwrap();
    assert_eq!(
        engine.configure_remote_control_pairing(target_identity),
        Ok(RemoteControlPairingAvailabilityDestination::canonical()),
    );
    engine
}

#[test]
fn a_verified_zero_hop_availability_installs_one_direct_route_and_one_typed_observation() {
    let interfaces = [routable_descriptor(SOURCE)];
    let mut engine = configured_engine();
    let mut wire = availability_wire(canonical_outer_header());
    let mut observed = None;
    let mut ordinary_announces = 0;
    let mut ordinary_deliveries = 0;
    let mut directives = 0;

    let wake = engine.ingest_packet_into(
        InboundPacket {
            arrived_at: OBSERVED_AT,
            source_interface: SOURCE,
            bytes: &mut wire,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&interfaces),
            now: OBSERVED_AT,
            fill_entropy: &mut |_| {},
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if let EngineReaction::Journaled(
                    Journaled::RemoteControlPairingAvailabilityObserved(observation),
                ) = &reaction
                {
                    observed = Some((
                        observation.endpoint(),
                        observation.observed_at(),
                        observation.expires_at(),
                        observation.source_interface(),
                        observation.public_app_data().as_bytes().to_vec(),
                    ));
                }
                if matches!(
                    &reaction,
                    EngineReaction::Journaled(Journaled::AnnounceHeard { .. })
                ) {
                    ordinary_announces += 1;
                }
                if matches!(
                    &reaction,
                    EngineReaction::Journaled(Journaled::Delivered(_))
                ) {
                    ordinary_deliveries += 1;
                }
                if matches!(&reaction, EngineReaction::Directive(Directive::Send { .. })) {
                    directives += 1;
                }
            },
        },
    );

    let (endpoint, observed_at, expires_at, source_interface, public_app_data) = observed.unwrap();
    assert_eq!(
        (observed_at, expires_at, source_interface, public_app_data),
        (
            OBSERVED_AT,
            InstantMillis(68_000),
            SOURCE,
            b"nearby".to_vec(),
        ),
    );
    let route = engine
        .route_snapshot(
            endpoint.destination_hash(),
            AttachedInterfaces::new(&interfaces),
        )
        .unwrap();
    assert_eq!(
        (
            route.destination,
            route.hops,
            route.via,
            route.learned_at,
            route.interface,
        ),
        (
            endpoint.destination_hash(),
            1,
            NextHop::Direct,
            OBSERVED_AT,
            SOURCE,
        ),
    );
    assert_eq!(wake.expired_routes, WakeSchedule::AtMost(route.expires_at));
    assert_eq!(wake.scheduled_announces, WakeSchedule::Unchanged);
    assert_eq!(
        (ordinary_announces, ordinary_deliveries, directives),
        (0, 0, 0)
    );
    assert_eq!(engine.scheduled_announce_count(), 0);

    let mut replay = availability_wire(canonical_outer_header());
    let mut replay_observations = 0;
    engine.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(9_000),
            source_interface: SOURCE,
            bytes: &mut replay,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&interfaces),
            now: InstantMillis(9_000),
            fill_entropy: &mut |_| {},
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if matches!(
                    reaction,
                    EngineReaction::Journaled(Journaled::RemoteControlPairingAvailabilityObserved(
                        _
                    ))
                ) {
                    replay_observations += 1;
                }
            },
        },
    );
    assert_eq!(replay_observations, 0);
    assert_eq!(engine.route_count(), 1);
}

#[test]
fn deferred_verification_owns_the_availability_and_preserves_arrival_provenance() {
    let interfaces = [routable_descriptor(SOURCE)];
    let mut engine = configured_engine();
    let mut wire = availability_wire(canonical_outer_header());
    let mut deferred = DeferredCrypto::default();
    let outcome = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: OBSERVED_AT,
            source_interface: SOURCE,
            bytes: &mut wire,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        Some(&mut deferred),
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::OwesRemoteControlPairingAvailabilityVerify,
    );
    assert_eq!(engine.route_count(), 0);
    let DeferredCrypto::RemoteControlPairingAvailabilityVerify(owed) = deferred else {
        panic!("pairing availability should owe its own verification");
    };
    assert_eq!(owed.source_interface(), SOURCE);
    let verified = match owed.verify() {
        RemoteControlPairingAvailabilityVerification::Verified(verified) => verified,
        RemoteControlPairingAvailabilityVerification::Invalid(_) => {
            panic!("the signed pairing availability should verify")
        }
    };

    let mut observed = None;
    engine.resume_remote_control_pairing_availability(
        verified,
        AttachedInterfaces::new(&interfaces),
        &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::RemoteControlPairingAvailabilityObserved(
                observation,
            )) = reaction
            {
                observed = Some((observation.observed_at(), observation.source_interface()));
            }
        },
    );
    assert_eq!(observed, Some((OBSERVED_AT, SOURCE)));
    assert_eq!(engine.route_count(), 1);
}

#[test]
fn deferred_verification_names_a_structurally_valid_tampered_availability_as_invalid() {
    let interfaces = [routable_descriptor(SOURCE)];
    let mut engine = configured_engine();
    let mut wire = availability_wire(canonical_outer_header());
    let last = wire.len() - 1;
    wire[last] ^= 1;
    let mut deferred = DeferredCrypto::default();

    assert_eq!(
        engine.ingest_packet_with(
            InboundPacket {
                arrived_at: OBSERVED_AT,
                source_interface: SOURCE,
                bytes: &mut wire,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            Some(&mut deferred),
        ),
        IngestPacketOutcome::OwesRemoteControlPairingAvailabilityVerify,
    );
    let DeferredCrypto::RemoteControlPairingAvailabilityVerify(owed) = deferred else {
        panic!("pairing availability should owe its own verification");
    };
    match owed.verify() {
        RemoteControlPairingAvailabilityVerification::Invalid(invalid) => {
            assert_eq!(invalid.source_interface(), SOURCE)
        }
        RemoteControlPairingAvailabilityVerification::Verified(_) => {
            panic!("the tampered pairing availability must not verify")
        }
    }
    assert_eq!(engine.route_count(), 0);
}

#[test]
fn direct_pairing_availability_does_not_settle_network_path_discovery() {
    let interfaces = [routable_descriptor(SOURCE)];
    let attached = AttachedInterfaces::new(&interfaces);
    let mut engine = configured_engine();
    let (payload, payload_len) = availability_payload();
    let destination = RemoteControlPairingAvailability::parse(&payload[..payload_len])
        .unwrap()
        .pairing_endpoint()
        .destination_hash();
    let mut path_request_wire = [0u8; BROADCAST_MTU];
    assert!(matches!(
        engine.write_commanded_path_request_with_interfaces(
            CommandId(7),
            &RequestPath {
                destination,
                id: PathRequestId::new([0xD4; 16]),
            },
            OBSERVED_AT,
            attached,
            &mut path_request_wire,
        ),
        PathRequestWriteOutcome::Written { .. }
    ));

    let mut wire = availability_wire(canonical_outer_header());
    let wake = engine.ingest_packet_into(
        InboundPacket {
            arrived_at: OBSERVED_AT,
            source_interface: SOURCE,
            bytes: &mut wire,
        },
        IngestIo {
            interfaces: attached,
            now: OBSERVED_AT,
            fill_entropy: &mut |_| {},
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |_| {},
        },
    );

    assert_eq!(wake.path_request_timeouts, WakeSchedule::Unchanged);
    assert_eq!(
        engine
            .pop_settled_path_request(&destination)
            .map(|pending| pending.command_id),
        Some(CommandId(7)),
    );
}

#[test]
fn pairing_route_activity_cannot_extend_the_advertised_deadline_or_enter_persistence() {
    let interfaces = [routable_descriptor(SOURCE)];
    let attached = AttachedInterfaces::new(&interfaces);
    let mut engine = configured_engine();
    let mut wire = availability_wire(canonical_outer_header());
    let mut endpoint = None;
    engine.ingest_packet_into(
        InboundPacket {
            arrived_at: OBSERVED_AT,
            source_interface: SOURCE,
            bytes: &mut wire,
        },
        IngestIo {
            interfaces: attached,
            now: OBSERVED_AT,
            fill_entropy: &mut |_| {},
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if let EngineReaction::Journaled(
                    Journaled::RemoteControlPairingAvailabilityObserved(observation),
                ) = reaction
                {
                    endpoint = Some(observation.endpoint());
                }
            },
        },
    );
    let destination = endpoint.unwrap().destination_hash();
    let route = engine.route_snapshot(destination, attached).unwrap();
    match route.retention {
        RouteRetention::Network => panic!("pairing availability installed a network route"),
        RouteRetention::Ephemeral { expires_after } => {
            assert_eq!(expires_after.duration(), DurationMillis(60_000));
        }
    }
    assert_eq!(route.expires_at, InstantMillis(68_000));
    assert_eq!(engine.persisted_route_rows().count(), 0);
    assert_eq!(engine.persisted_route_destinations().count(), 0);
    assert!(engine.persisted_route_row(&destination).is_none());

    engine
        .routing_table
        .mark_responsiveness(&destination, RouteResponsiveness::Responsive);
    engine
        .routing_table
        .note_relayed(&destination, InstantMillis(67_999));
    let before = engine.cull_expired_routes(InstantMillis(67_999), attached, &mut |_| {});
    assert_eq!(engine.route_count(), 1);
    assert_eq!(
        before.expired_routes,
        WakeSchedule::At(InstantMillis(68_000))
    );

    let mut removed = None;
    let at = engine.cull_expired_routes(InstantMillis(68_000), attached, &mut |reaction| {
        if let EngineReaction::Journaled(Journaled::RouteRemoved { destination, cause }) = reaction
        {
            removed = Some((destination, cause));
        }
    });
    assert_eq!(removed, Some((destination, RouteRemovalCause::Expired)),);
    assert_eq!(engine.route_count(), 0);
    assert_eq!(at.expired_routes, WakeSchedule::Idle);
}

#[test]
fn every_nonzero_hop_pairing_availability_is_ignored_before_verification() {
    let interfaces = [routable_descriptor(SOURCE)];
    let (payload, payload_len) = availability_payload();
    for hops in 1..=u8::MAX {
        let expected = if wire_hop_count_is_valid(hops) {
            IgnoreReason::HopLimitReached
        } else {
            IgnoreReason::Malformed
        };
        let mut engine = configured_engine();
        let mut wire = availability_wire_with_payload(
            WirePacketHeader {
                hops,
                ..canonical_outer_header()
            },
            &payload[..payload_len],
        );
        let mut deferred = DeferredCrypto::default();

        assert_eq!(
            engine.ingest_packet_with(
                InboundPacket {
                    arrived_at: OBSERVED_AT,
                    source_interface: SOURCE,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                Some(&mut deferred),
            ),
            IngestPacketOutcome::Ignored(expected),
        );
        assert!(matches!(deferred, DeferredCrypto::Empty));
        assert_eq!(engine.route_count(), 0);
    }
}

#[test]
fn invalid_outer_shapes_are_rejected_before_pairing_crypto_is_deferred() {
    let interfaces = [routable_descriptor(SOURCE)];
    for header in [
        WirePacketHeader {
            propagation: PropagationType::Transport,
            transport_id: Some(TransportId::new([0xD4; 16])),
            ..canonical_outer_header()
        },
        WirePacketHeader {
            context: WireContext::CacheRequest,
            ..canonical_outer_header()
        },
    ] {
        let mut engine = configured_engine();
        let mut wire = availability_wire(header);
        let mut deferred = DeferredCrypto::default();
        let outcome = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: OBSERVED_AT,
                source_interface: SOURCE,
                bytes: &mut wire,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            Some(&mut deferred),
        );
        assert!(matches!(outcome, IngestPacketOutcome::Ignored(_)));
        assert!(matches!(deferred, DeferredCrypto::Empty));
        assert_eq!(engine.route_count(), 0);
    }
}
