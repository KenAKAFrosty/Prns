use super::*;
use crate::engine::test_support::*;
use crate::engine::{
    CommandId, IngestIo, PathRequestId, PathRequestWriteOutcome, RequestPath, RouteRemovalCause,
    WakeSchedule, PATH_REQUEST_TIMEOUT_MS,
};
use crate::interfaces::InterfaceDescriptor;
use crate::interfaces::{InboundPacket, InterfaceId, InterfaceMode};
use crate::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
use crate::routing::links::LinkMode;
use crate::wire::{
    DestinationHash, DestinationType, PacketType, PropagationType, WirePacketHeader,
};

#[test]
fn a_fresh_drive_is_deterministic_and_emits_nothing() {
    let mut left: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
    let mut right: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();

    let left_bytes = tick_capture(
        &mut left,
        InstantMillis(1_000),
        AttachedInterfaces::new(&[]),
    );
    let right_bytes = tick_capture(
        &mut right,
        InstantMillis(1_000),
        AttachedInterfaces::new(&[]),
    );

    assert_eq!(observable_state(&left), observable_state(&right));
    assert!(left_bytes.is_empty());
    assert_eq!(left_bytes, right_bytes);
}

#[test]
fn accepted_announces_schedule_a_rebroadcast_and_tick_emits_them() {
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let mut state = transporting_node();
    state.protocol.local_hop_count_override =
        crate::engine::LocalHopCountOverride::override_with(5).unwrap();
    let interfaces = [routable_descriptor(InterfaceId::new([0xFE; 8]))];

    let arrival = InstantMillis(1_000);
    let out = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(out, rns_1_4_0_announce_accepted(1));
    assert_eq!(state.scheduled_announce_count(), 1);

    let emitted = tick_capture(
        &mut state,
        InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        AttachedInterfaces::new(&interfaces),
    );
    assert_eq!(
        state.scheduled_announce_count(),
        1,
        "the first emission re-arms the entry for its second rebroadcast",
    );

    assert_eq!(emitted.len(), 1);
    let wire = &emitted[0];
    let (header, payload) = WirePacketHeader::parse(wire).unwrap();
    assert_eq!(header.packet_type, PacketType::Announce);
    assert_eq!(header.destination_type, DestinationType::Single);
    assert_eq!(header.propagation, PropagationType::Transport);
    assert_eq!(header.transport_id, Some(TEST_TRANSPORT_ID));
    let original = WirePacketHeader::parse(&raw).unwrap().0;
    assert_eq!(header.hops, original.hops + 1);
    assert_eq!(header.address, original.address);
    let original_payload = WirePacketHeader::parse(&raw).unwrap().1;
    assert_eq!(payload, original_payload);
}

#[test]
fn a_rebroadcast_reproduces_the_rns_1_4_0_retransmitted_wire() {
    let mut heard = bytes_from_hex(RNS_1_4_0_RATCHETED_ANNOUNCE);
    let mut state = transporting_node();
    let arrival = InstantMillis(1_000);
    let _ = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut heard,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(state.scheduled_announce_count(), 1);

    let emitted = tick_capture(
        &mut state,
        InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        AttachedInterfaces::new(&transporting_interfaces()),
    );
    assert_eq!(
        emitted,
        std::vec![bytes_from_hex(RNS_1_4_0_RETRANSMITTED_ANNOUNCE)],
        "our retransmission must be byte-identical to the reference's own",
    );
}

#[test]
fn a_directed_scheduled_announce_fires_only_to_its_target_interface() {
    use crate::engine::{AnnounceIngest, IngestPacketOutcome};

    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let mut state = transporting_node();
    let IngestPacketOutcome::Announce(AnnounceIngest::Accepted(accepted)) = state
        .ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        )
    else {
        panic!("the announce is accepted");
    };

    let target = InterfaceId::new([0xAA; 8]);
    let _ = state.scheduled_announces.schedule_directed(
        accepted.destination,
        InstantMillis(2_000),
        target,
        accepted.hops,
    );

    let interfaces = [
        routable_descriptor(target),
        routable_descriptor(InterfaceId::new([0xBB; 8])),
    ];
    let mut targets = std::vec::Vec::new();
    state.fire_due_scheduled_announces(
        InstantMillis(2_000),
        AttachedInterfaces::new(&interfaces),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction {
                targets.push(target);
            }
        },
    );
    assert_eq!(
        targets,
        std::vec![target],
        "a directed answer reaches only its target, where a flood would reach both interfaces",
    );
}

fn rebroadcast_fan_for(
    state: &mut EngineState<TestStorageLayout>,
    interfaces: AttachedInterfaces<'_>,
) -> std::vec::Vec<InterfaceId> {
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let arrival = InstantMillis(1_000);
    let _ = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(state.scheduled_announce_count(), 1);

    let mut targets = std::vec::Vec::new();
    let _ = state.fire_due_scheduled_announces(
        InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        interfaces,
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction {
                targets.push(target);
            }
        },
    );
    targets
}

#[test]
fn a_same_interface_repeat_source_joins_its_own_rebroadcast_fan() {
    let source = InterfaceId::new([0u8; 8]);
    let other = InterfaceId::new([0xFE; 8]);
    let interfaces = [repeating_descriptor(source), routable_descriptor(other)];

    let mut state = transporting_node();
    assert_eq!(
        rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
        std::vec![source, other],
    );
}

#[test]
fn a_cross_interface_only_source_is_left_out_of_its_own_rebroadcast_fan() {
    let source = InterfaceId::new([0u8; 8]);
    let other = InterfaceId::new([0xFE; 8]);
    let interfaces = [routable_descriptor(source), routable_descriptor(other)];

    let mut state = transporting_node();
    assert_eq!(
        rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
        std::vec![other]
    );
}

#[test]
fn a_bluetooth_peer_announce_rebroadcasts_to_usb_device_transport() {
    let source = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x42, 0, 0, 0, 0, 0, 0]);
    let usb = InterfaceId::new([
        InterfaceKind::UsbAutoDevice as u8,
        b'i',
        b'o',
        b's',
        b'-',
        b'u',
        b's',
        b'b',
    ]);
    let interfaces = [
        routable_descriptor(source),
        crate::interfaces::usb_auto::device_descriptor(usb),
    ];

    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let mut state = transporting_node();
    let arrival = InstantMillis(1_000);
    let out = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    assert_eq!(out, rns_1_4_0_announce_accepted(1));
    assert_eq!(state.scheduled_announce_count(), 1);

    let mut targets = std::vec::Vec::new();
    let _ = state.fire_due_scheduled_announces(
        InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        AttachedInterfaces::new(&interfaces),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction {
                targets.push(target);
            }
        },
    );

    assert_eq!(
        targets,
        std::vec![usb],
        "a transport-enabled iPad must forward a BLE-learned announce over USB",
    );
}

#[test]
fn a_scheduled_announce_emits_once_for_a_supervised_interface_fleet() {
    let first = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x41, 0, 0, 0, 0, 0, 0]);
    let second = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x42, 0, 0, 0, 0, 0, 0]);
    let interfaces = [routable_descriptor(first), routable_descriptor(second)];
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let mut state = transporting_node();
    let arrival = InstantMillis(1_000);
    let _ = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: InterfaceId::new([InterfaceKind::Loopback as u8; 8]),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );

    let mut fleets = std::vec::Vec::new();
    let _ = state.fire_due_scheduled_announces(
        InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        AttachedInterfaces::new(&interfaces),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounceToFleet {
                supervisor, ..
            }) = reaction
            {
                fleets.push(supervisor);
            }
        },
    );

    assert_eq!(fleets, std::vec![InterfaceKind::BluetoothAuto]);
}

#[test]
fn our_own_repeat_echoed_back_is_deduplicated() {
    use crate::engine::{AnnounceIngest, IngestPacketOutcome};

    let source = InterfaceId::new([0u8; 8]);
    let interfaces = [repeating_descriptor(source)];
    let mut state = transporting_node();
    let fan = rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces));
    assert_eq!(fan, std::vec![source]);

    let mut echo = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    echo[1] += 1;
    let out = state.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(5_000),
            source_interface: source,
            bytes: &mut echo,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    assert_eq!(
        out,
        IngestPacketOutcome::Announce(AnnounceIngest::Ignored),
        "the repeat coming home is the same announce: dedup eats it, no loop",
    );
    assert_eq!(state.route_count(), 1);
    assert_eq!(
        state.scheduled_announce_count(),
        1,
        "the echo is absorbed as a same-distance peer rebroadcast — counted, not looped into a fresh schedule",
    );
}

#[test]
fn an_onward_announce_echo_cancels_the_pending_retransmit() {
    let source = InterfaceId::new([0u8; 8]);
    let interfaces = [repeating_descriptor(source)];
    let mut state = transporting_node();
    let fan = rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces));
    assert_eq!(fan, std::vec![source]);
    assert_eq!(
        state.scheduled_announce_count(),
        1,
        "after one emission the entry is re-armed for its second rebroadcast",
    );

    let mut echo = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    echo[1] += 2;
    let _ = state.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(5_000),
            source_interface: source,
            bytes: &mut echo,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    assert_eq!(
        state.scheduled_announce_count(),
        0,
        "hearing our own rebroadcast one hop onward retires the pending retransmit",
    );
}

#[test]
fn an_interface_that_cannot_transport_never_joins_a_rebroadcast_fan() {
    use crate::interfaces::{EgressCapability, TransportCapability};

    let source = InterfaceId::new([0u8; 8]);
    let mut leaf = routable_descriptor(InterfaceId::new([0xFE; 8]));
    leaf.capabilities.egress = EgressCapability::Enabled(TransportCapability::NoTransport);
    let interfaces = [routable_descriptor(source), leaf];

    let mut state = transporting_node();
    assert_eq!(
        rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
        std::vec![]
    );
}

#[test]
fn a_local_client_announce_can_leave_on_a_transmit_only_interface() {
    use crate::interfaces::{EgressCapability, TransportCapability};

    let source = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
    let egress = InterfaceId::new([0xFE; 8]);
    let mut transmit_only = routable_descriptor(egress);
    transmit_only.capabilities.egress = EgressCapability::Enabled(TransportCapability::NoTransport);
    let interfaces = [routable_descriptor(source), transmit_only];
    let mut state = transporting_node();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let arrival = InstantMillis(1_000);
    let _ = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );

    let mut targets = std::vec::Vec::new();
    let _ = state.fire_due_scheduled_announces(
        InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        AttachedInterfaces::new(&interfaces),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction {
                targets.push(target);
            }
        },
    );

    assert_eq!(targets, std::vec![egress]);
}

fn moded(mode: InterfaceMode, descriptor: InterfaceDescriptor) -> InterfaceDescriptor {
    InterfaceDescriptor { mode, ..descriptor }
}

#[test]
fn an_access_point_egress_interface_is_withheld_from_the_rebroadcast_fan() {
    let source = InterfaceId::new([0u8; 8]);
    let ap = InterfaceId::new([0xFE; 8]);
    let interfaces = [
        repeating_descriptor(source),
        moded(InterfaceMode::AccessPoint, routable_descriptor(ap)),
    ];

    let mut state = transporting_node();
    assert_eq!(
        rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
        std::vec![source],
        "an access-point interface never carries an announce rebroadcast",
    );
}

#[test]
fn a_local_clients_announce_is_also_withheld_from_access_point_egress() {
    let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
    let ap = InterfaceId::new([0xFE; 8]);
    let interfaces = [
        routable_descriptor(app),
        moded(InterfaceMode::AccessPoint, routable_descriptor(ap)),
    ];
    let mut state = transporting_node();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let arrival = InstantMillis(1_000);
    let _ = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: app,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    assert_eq!(state.scheduled_announce_count(), 1);

    let mut targets = std::vec::Vec::new();
    let _ = state.fire_due_scheduled_announces(
        InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        AttachedInterfaces::new(&interfaces),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction {
                targets.push(target);
            }
        },
    );
    assert!(targets.is_empty());
}

#[test]
fn a_scheduled_local_client_announce_uses_the_hop_count_override_at_external_egress() {
    let local_client = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
    let external = InterfaceId::new([0xFE; 8]);
    let interfaces = [
        routable_descriptor(local_client),
        routable_descriptor(external),
    ];
    let mut state = transporting_node();
    state.protocol.local_hop_count_override =
        crate::engine::LocalHopCountOverride::override_with(5).unwrap();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let arrival = InstantMillis(1_000);
    let out = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: local_client,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    assert_eq!(out, rns_1_4_0_announce_accepted(0));

    let mut emitted = std::vec::Vec::new();
    let _ = state.fire_due_scheduled_announces(
        InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        AttachedInterfaces::new(&interfaces),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce {
                target,
                bytes,
                #[cfg(feature = "runtime-metrics")]
                origin,
                ..
            }) = reaction
            {
                #[cfg(feature = "runtime-metrics")]
                assert_eq!(origin, crate::engine::AnnounceOrigin::SharedClient);
                emitted.push((target, WirePacketHeader::parse(bytes).unwrap().0.hops));
            }
        },
    );

    assert_eq!(emitted, std::vec![(external, 5)]);
}

#[test]
fn a_roaming_egress_interface_is_withheld_toward_a_roaming_learned_route() {
    let source = InterfaceId::new([0u8; 8]);
    let roaming_out = InterfaceId::new([0xFE; 8]);
    let other = InterfaceId::new([0xAB; 8]);
    let interfaces = [
        moded(InterfaceMode::Roaming, repeating_descriptor(source)),
        moded(InterfaceMode::Roaming, routable_descriptor(roaming_out)),
        routable_descriptor(other),
    ];

    let mut state = transporting_node();
    assert_eq!(
        rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
        std::vec![other],
        "a roaming interface withholds a roaming-learned route; a full interface carries it",
    );
}

#[test]
fn a_roaming_egress_interface_carries_a_full_learned_route() {
    let source = InterfaceId::new([0u8; 8]);
    let roaming_out = InterfaceId::new([0xFE; 8]);
    let interfaces = [
        repeating_descriptor(source),
        moded(InterfaceMode::Roaming, routable_descriptor(roaming_out)),
    ];

    let mut state = transporting_node();
    assert_eq!(
        rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
        std::vec![source, roaming_out],
    );
}

#[test]
fn a_boundary_egress_carries_a_boundary_learned_route_where_a_roaming_egress_will_not() {
    let source = InterfaceId::new([0u8; 8]);
    let boundary_out = InterfaceId::new([0xFE; 8]);
    let roaming_out = InterfaceId::new([0xAB; 8]);
    let interfaces = [
        moded(InterfaceMode::Boundary, repeating_descriptor(source)),
        moded(InterfaceMode::Boundary, routable_descriptor(boundary_out)),
        moded(InterfaceMode::Roaming, routable_descriptor(roaming_out)),
    ];

    let mut state = transporting_node();
    assert_eq!(
        rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
        std::vec![source, boundary_out],
        "boundary carries a boundary-learned route; roaming withholds the same route",
    );
}

#[test]
fn scheduled_announces_are_not_emitted_before_their_due_time() {
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let mut state = transporting_node();
    let arrival = InstantMillis(1_000);
    let _ = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(state.scheduled_announce_count(), 1);

    let interfaces = [routable_descriptor(InterfaceId::new([0xFE; 8]))];
    let emitted = tick_capture(
        &mut state,
        InstantMillis(arrival.0 - 1),
        AttachedInterfaces::new(&interfaces),
    );
    assert!(emitted.is_empty());
    assert_eq!(state.scheduled_announce_count(), 1);
}

#[test]
fn same_inputs_produce_byte_identical_emissions_on_two_engines() {
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let now = InstantMillis(5_000);
    let arrival = InstantMillis(1_000);

    let mut left = transporting_node();
    let mut right = transporting_node();

    let interfaces = [routable_descriptor(InterfaceId::new([0xFE; 8]))];
    for state in [&mut left, &mut right] {
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
    }
    let left_bytes = tick_capture(&mut left, now, AttachedInterfaces::new(&interfaces));
    let right_bytes = tick_capture(&mut right, now, AttachedInterfaces::new(&interfaces));

    assert_eq!(observable_state(&left), observable_state(&right));
    assert_eq!(left_bytes, right_bytes);
    assert_eq!(left_bytes.len(), 1);
}

#[test]
fn fire_due_scheduled_announces_emits_then_re_arms_until_the_cap() {
    fn fire(
        state: &mut EngineState<TestStorageLayout>,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
    ) -> (
        std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
        WakeSchedule,
    ) {
        let mut sent = std::vec::Vec::new();
        let delta = state.fire_due_scheduled_announces(now, interfaces, &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { target, bytes, .. }) =
                reaction
            {
                sent.push((target, bytes.to_vec()));
            }
        });
        (sent, delta.scheduled_announces)
    }

    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let mut state = transporting_node();
    let target = InterfaceId::new([0xFE; 8]);
    let interfaces = [routable_descriptor(target)];

    let arrival = InstantMillis(1_000);
    let _ = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(state.scheduled_announce_count(), 1);

    let first_due = InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
    let (sent, schedule) = fire(&mut state, first_due, AttachedInterfaces::new(&interfaces));
    assert_eq!(sent.len(), 1, "one directive for the lone interface");
    assert_eq!(
        sent[0].0, target,
        "the rebroadcast names the firable interface"
    );
    assert_eq!(
        state.scheduled_announce_count(),
        1,
        "the first emission re-arms the entry rather than clearing it",
    );
    assert_eq!(
        schedule,
        WakeSchedule::At(InstantMillis(
            first_due.0 + REBROADCAST_RETRANSMIT_INTERVAL_MS
        )),
        "the schedule is re-armed one retransmit interval out",
    );
    let (header, _) = WirePacketHeader::parse(&sent[0].1).unwrap();
    assert_eq!(header.packet_type, PacketType::Announce);
    let original = WirePacketHeader::parse(&bytes_from_hex(RNS_1_4_0_ANNOUNCE))
        .unwrap()
        .0;
    assert_eq!(
        header.hops,
        original.hops + 1,
        "the rebroadcast bumps the hop count"
    );
    let first_bytes = sent[0].1.clone();

    let second_due = InstantMillis(first_due.0 + REBROADCAST_RETRANSMIT_INTERVAL_MS);
    let (sent, schedule) = fire(&mut state, second_due, AttachedInterfaces::new(&interfaces));
    assert_eq!(sent.len(), 1, "the second and final emission");
    assert_eq!(
        sent[0].1, first_bytes,
        "the retransmit re-emits the same pinned announce, byte for byte",
    );
    assert_eq!(
        state.scheduled_announce_count(),
        0,
        "reaching the rebroadcast cap drops the entry",
    );
    assert_eq!(
        schedule,
        WakeSchedule::Idle,
        "no rebroadcasts remain after the cap"
    );
}

#[test]
fn an_ignored_echo_that_cancels_a_rebroadcast_reports_the_emptied_lane() {
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let mut state = transporting_node();
    let target = InterfaceId::new([0xFE; 8]);
    let interfaces = [routable_descriptor(target)];

    let arrival = InstantMillis(1_000);
    let _ = state.ingest_packet_with(
        InboundPacket {
            arrived_at: arrival,
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(state.scheduled_announce_count(), 1);

    let first_due = InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
    let mut rebroadcast = std::vec::Vec::new();
    let _ = state.fire_due_scheduled_announces(
        first_due,
        AttachedInterfaces::new(&interfaces),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { bytes, .. }) = reaction {
                rebroadcast = bytes.to_vec();
            }
        },
    );
    assert_eq!(state.scheduled_announce_count(), 1);
    assert!(
        !rebroadcast.is_empty(),
        "the fire emitted a rebroadcast to echo back"
    );

    let echo = |state: &mut EngineState<TestStorageLayout>, now: u64| -> WakeSchedule {
        let mut bytes = rebroadcast.clone();
        state
            .ingest_packet_into(
                InboundPacket {
                    arrived_at: InstantMillis(now),
                    source_interface: InterfaceId::new([0u8; 8]),
                    bytes: &mut bytes,
                },
                IngestIo {
                    interfaces: AttachedInterfaces::new(&transporting_interfaces()),
                    now: InstantMillis(now),
                    fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                    should_prove: &mut |_| false,
                    should_accept_resource:
                        &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                    sink: &mut |_| {},
                },
            )
            .scheduled_announces
    };

    let echo_at = first_due.0 + 1;
    let _ = echo(&mut state, echo_at);
    assert_eq!(
        state.scheduled_announce_count(),
        1,
        "the first echo only counts the peer rebroadcast",
    );

    let second = echo(&mut state, echo_at + 1);
    assert_eq!(
        state.scheduled_announce_count(),
        0,
        "the second echo reaches the peer cap and cancels the pending rebroadcast",
    );
    assert_eq!(
        second,
        WakeSchedule::Idle,
        "an ignored echo that empties the queue reports Idle, not a stale Unchanged",
    );
    assert_eq!(
        second,
        state.scheduled_announces_wake(),
        "the ingest delta agrees with a full wake recompute (no manifold drift)",
    );
}

#[test]
fn settle_timed_out_path_requests_closes_each_expired_request_once_past_its_deadline() {
    let mut engine = EngineState::<TestStorageLayout>::default();
    let issued_at = InstantMillis(1_000);
    let mut buf = [0u8; BROADCAST_MTU];
    let outcome = engine.write_commanded_path_request(
        CommandId(9),
        &RequestPath {
            destination: DestinationHash::new([0x44; 16]),
            id: PathRequestId::new([0x55; 16]),
        },
        issued_at,
        &mut buf,
    );
    assert!(matches!(outcome, PathRequestWriteOutcome::Written { .. }));

    let mut settled: std::vec::Vec<(CommandId, Settlement)> = std::vec::Vec::new();

    engine.settle_timed_out_path_requests(issued_at, &mut |reaction| {
        if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) = reaction {
            settled.push((id, settlement));
        }
    });
    assert!(settled.is_empty(), "before the deadline, nothing settles");

    engine.settle_timed_out_path_requests(
        InstantMillis(issued_at.0 + PATH_REQUEST_TIMEOUT_MS + 1),
        &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settled.push((id, settlement));
            }
        },
    );
    assert_eq!(
        settled,
        std::vec![(
            CommandId(9),
            Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
        )],
        "past the deadline the request settles Timeout, exactly once",
    );
}

#[test]
fn the_cull_journals_an_orphan_as_route_interface_gone() {
    let source = InterfaceId::new([0u8; 8]);
    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&[routable_descriptor(source)]),
        &mut |_| {},
        None,
    );
    assert_eq!(engine.route_count(), 1);
    let destination = DestinationHash::new(
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
            .try_into()
            .unwrap(),
    );
    let _ = engine
        .scheduled_announces
        .schedule(destination, InstantMillis(9_000), source, 1);

    let without_source = [routable_descriptor(InterfaceId::new([0xEE; 8]))];
    let mut journal = std::vec::Vec::new();
    let delta = engine.cull_expired_routes(
        InstantMillis(2_000),
        AttachedInterfaces::new(&without_source),
        &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::RouteRemoved {
                destination,
                cause: RouteRemovalCause::InterfaceGone,
            }) = reaction
            {
                journal.push(destination);
            }
        },
    );
    assert_eq!(
        journal,
        std::vec![DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap()
        )],
        "the orphan's removal names its cause",
    );
    assert_eq!(engine.route_count(), 0);
    assert_eq!(engine.scheduled_announce_count(), 0);
    assert_eq!(delta.scheduled_announces, crate::engine::WakeSchedule::Idle);
    assert_eq!(
        delta.expired_routes,
        crate::engine::WakeSchedule::Idle,
        "nothing is left to wake for",
    );
}

#[test]
fn expiring_a_route_cancels_its_scheduled_announce_and_wake() {
    let source = InterfaceId::new([0xA1; 8]);
    let interfaces = [routable_descriptor(source)];
    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    let destination = DestinationHash::new(
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
            .try_into()
            .unwrap(),
    );
    let _ = engine
        .scheduled_announces
        .schedule(destination, InstantMillis(u64::MAX), source, 1);
    let mut causes = std::vec::Vec::new();

    let delta = engine.cull_expired_routes(
        InstantMillis(u64::MAX),
        AttachedInterfaces::new(&interfaces),
        &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::RouteRemoved { cause, .. }) = reaction {
                causes.push(cause);
            }
        },
    );

    assert_eq!(causes, std::vec![RouteRemovalCause::Expired]);
    assert_eq!(engine.route_count(), 0);
    assert_eq!(engine.scheduled_announce_count(), 0);
    assert_eq!(delta.scheduled_announces, WakeSchedule::Idle);
}

#[test]
fn a_dropped_route_marks_its_interface_so_the_destination_count_recomputes() {
    let source = InterfaceId::new([0u8; 8]);
    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&[routable_descriptor(source)]),
        &mut |_| {},
        None,
    );

    let mut on_insert = std::vec::Vec::new();
    engine
        .take_dirty_interfaces()
        .drain(|interface| on_insert.push(interface));
    assert_eq!(
        on_insert,
        std::vec![source],
        "learning a route marks the interface it arrived on",
    );
    assert_eq!(engine.interface_counts(source).destinations, 1);

    let without_source = [routable_descriptor(InterfaceId::new([0xEE; 8]))];
    engine.cull_expired_routes(
        InstantMillis(2_000),
        AttachedInterfaces::new(&without_source),
        &mut |_| {},
    );

    let mut on_cull = std::vec::Vec::new();
    engine
        .take_dirty_interfaces()
        .drain(|interface| on_cull.push(interface));
    assert_eq!(
        on_cull,
        std::vec![source],
        "dropping the route re-marks the interface, so the stale count never lingers silently",
    );
    assert_eq!(engine.interface_counts(source).destinations, 0);
}

#[test]
fn route_culling_drops_reverse_and_transported_rows_with_a_missing_interface() {
    use crate::routing::links::transported::TransportedLink;
    use crate::routing::links::LinkId;
    use crate::routing::reverse_routes::ReverseRouteEntry;

    let attached = InterfaceId::new([0xA1; 8]);
    let other = InterfaceId::new([0xC3; 8]);
    let missing = InterfaceId::new([0xB2; 8]);
    let interfaces = [routable_descriptor(attached), routable_descriptor(other)];
    let proof_destination = DestinationHash::new([0xD4; 16]);
    let mut engine = EngineState::<TestStorageLayout>::default();
    engine.reverse_routes.remember(
        ReverseRouteEntry {
            proof_destination,
            received_interface: attached,
            outbound_interface: missing,
            expires_at: InstantMillis(30_000),
        },
        InstantMillis(1_000),
    );
    engine
        .transported_links
        .track(TransportedLink {
            link_id: LinkId::new([0x5C; 16]),
            destination: DestinationHash::new([0xDD; 16]),
            mode: LinkMode::Aes256Cbc,
            next_hop: None,
            next_hop_interface: attached,
            received_interface: missing,
            taken_hops: 1,
            remaining_hops: 1,
            validated_by_proof: false,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(30_000),
        })
        .unwrap();

    let _ = engine.cull_expired_routes(
        InstantMillis(2_000),
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
    );

    assert_eq!(
        engine
            .reverse_routes
            .take(&proof_destination, InstantMillis(2_000)),
        None,
    );
    assert!(engine.transported_links.is_empty());
}

fn overdue_transport_recovery_emissions(
    additional_route_hops: u8,
    taken_hops: u8,
) -> std::vec::Vec<InterfaceId> {
    use crate::routing::links::transported::TransportedLink;
    use crate::routing::links::LinkId;

    let received = InterfaceId::new([0xA1; 8]);
    let away = InterfaceId::new([0xB2; 8]);
    let interfaces = [routable_descriptor(received), routable_descriptor(away)];
    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    raw[1] += additional_route_hops;
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: received,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    let destination = DestinationHash::new(
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
            .try_into()
            .unwrap(),
    );
    engine
        .transported_links
        .track(TransportedLink {
            link_id: LinkId::new([0x5C; 16]),
            destination,
            mode: LinkMode::Aes256Cbc,
            next_hop: None,
            next_hop_interface: away,
            received_interface: received,
            taken_hops,
            remaining_hops: 1,
            validated_by_proof: false,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(7_000),
        })
        .unwrap();

    let mut sent = std::vec::Vec::new();
    let _ = engine.fire_due_link_deadlines(
        InstantMillis(7_000),
        AttachedInterfaces::new(&interfaces),
        &mut |bytes: &mut [u8]| bytes.fill(0x5A),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                sent.push(target);
            }
        },
    );
    sent
}

#[test]
fn overdue_transport_recovery_requires_a_neighbor_route_or_initiator() {
    assert_eq!(
        overdue_transport_recovery_emissions(2, 1),
        std::vec![InterfaceId::new([0xB2; 8])],
    );
    assert!(overdue_transport_recovery_emissions(2, 2).is_empty());
    assert_eq!(
        overdue_transport_recovery_emissions(0, 2),
        std::vec![InterfaceId::new([0xB2; 8])],
    );
}

#[test]
fn an_unproved_transported_link_to_a_neighbor_marks_the_route_unresponsive() {
    use crate::routing::links::transported::TransportedLink;
    use crate::routing::links::LinkId;

    let source = InterfaceId::new([0xA1; 8]);
    let interfaces = [routable_descriptor(source)];
    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    let destination = DestinationHash::new(
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
            .try_into()
            .unwrap(),
    );
    assert_eq!(
        engine
            .routing_table
            .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unknown,
        "a freshly learned route is unconfirmed",
    );

    engine
        .transported_links
        .track(TransportedLink {
            link_id: LinkId::new([0x5C; 16]),
            destination,
            mode: LinkMode::Aes256Cbc,
            next_hop: None,
            next_hop_interface: source,
            received_interface: source,
            taken_hops: 1,
            remaining_hops: 1,
            validated_by_proof: false,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(7_000),
        })
        .unwrap();

    let _ = engine.fire_due_link_deadlines(
        InstantMillis(7_000),
        AttachedInterfaces::new(&interfaces),
        &mut |bytes: &mut [u8]| bytes.fill(0),
        &mut |_| {},
    );

    assert_eq!(
        engine
            .routing_table
            .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unresponsive,
        "the neighbor link never proved, so its route is marked unresponsive",
    );
}

#[test]
fn an_unproved_neighbor_link_fires_a_path_request_away_from_the_received_lane() {
    use crate::routing::links::transported::TransportedLink;
    use crate::routing::links::LinkId;
    use crate::routing::path_requests::PATH_REQUEST_DESTINATION;

    let received = InterfaceId::new([0xA1; 8]);
    let away = InterfaceId::new([0xB2; 8]);
    let interfaces = [routable_descriptor(received), routable_descriptor(away)];

    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: received,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    let destination = DestinationHash::new(
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
            .try_into()
            .unwrap(),
    );

    engine
        .transported_links
        .track(TransportedLink {
            link_id: LinkId::new([0x5C; 16]),
            destination,
            mode: LinkMode::Aes256Cbc,
            next_hop: None,
            next_hop_interface: away,
            received_interface: received,
            taken_hops: 1,
            remaining_hops: 1,
            validated_by_proof: false,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(7_000),
        })
        .unwrap();

    let mut sent = std::vec::Vec::new();
    let _ = engine.fire_due_link_deadlines(
        InstantMillis(7_000),
        AttachedInterfaces::new(&interfaces),
        &mut |bytes: &mut [u8]| bytes.fill(0x5A),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { target, bytes }) = reaction {
                sent.push((target, bytes.to_vec()));
            }
        },
    );

    assert_eq!(
        engine
            .routing_table
            .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unresponsive,
    );
    assert_eq!(
        sent.len(),
        1,
        "the request fires on the one lane that wasn't the dead link's",
    );
    assert_eq!(
        sent[0].0, away,
        "never back out the interface the failed link arrived on",
    );
    let (header, payload) = WirePacketHeader::parse(&sent[0].1).unwrap();
    assert_eq!(
        DestinationHash::from_address(header.address),
        PATH_REQUEST_DESTINATION
    );
    assert_eq!(header.destination_type, DestinationType::Plain);
    assert_eq!(
        &payload[..16],
        destination.as_bytes(),
        "and it asks for the destination whose link just died",
    );
}

#[test]
fn an_unproved_link_recovers_when_the_initiator_is_the_neighbor_too() {
    use crate::routing::links::transported::TransportedLink;
    use crate::routing::links::LinkId;

    let received = InterfaceId::new([0xA1; 8]);
    let away = InterfaceId::new([0xB2; 8]);
    let interfaces = [routable_descriptor(received), routable_descriptor(away)];

    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: received,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    let destination = DestinationHash::new(
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
            .try_into()
            .unwrap(),
    );

    engine
        .transported_links
        .track(TransportedLink {
            link_id: LinkId::new([0x5C; 16]),
            destination,
            mode: LinkMode::Aes256Cbc,
            next_hop: None,
            next_hop_interface: away,
            received_interface: received,
            taken_hops: 1,
            remaining_hops: 4,
            validated_by_proof: false,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(7_000),
        })
        .unwrap();

    let mut sent = std::vec::Vec::new();
    let _ = engine.fire_due_link_deadlines(
        InstantMillis(7_000),
        AttachedInterfaces::new(&interfaces),
        &mut |bytes: &mut [u8]| bytes.fill(0x5A),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                sent.push(target);
            }
        },
    );

    assert_eq!(
        engine
            .routing_table
            .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unresponsive,
        "a far destination still recovers when its link initiator is our neighbor",
    );
    assert_eq!(sent, std::vec![away]);
}

#[test]
fn an_unproved_link_from_a_local_client_rediscovers_everywhere_without_a_mark() {
    use crate::routing::links::transported::TransportedLink;
    use crate::routing::links::LinkId;

    let received = InterfaceId::new([0xA1; 8]);
    let away = InterfaceId::new([0xB2; 8]);
    let interfaces = [routable_descriptor(received), routable_descriptor(away)];

    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: received,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    let destination = DestinationHash::new(
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
            .try_into()
            .unwrap(),
    );

    engine
        .transported_links
        .track(TransportedLink {
            link_id: LinkId::new([0x5C; 16]),
            destination,
            mode: LinkMode::Aes256Cbc,
            next_hop: None,
            next_hop_interface: away,
            received_interface: received,
            taken_hops: 0,
            remaining_hops: 1,
            validated_by_proof: false,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(7_000),
        })
        .unwrap();

    let mut sent = std::vec::Vec::new();
    let _ = engine.fire_due_link_deadlines(
        InstantMillis(7_000),
        AttachedInterfaces::new(&interfaces),
        &mut |bytes: &mut [u8]| bytes.fill(0x5A),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                sent.push(target);
            }
        },
    );

    assert_eq!(
        sent,
        std::vec![received, away],
        "a local client's dead link re-requests on every interface, its own included",
    );
    assert_eq!(
        engine
            .routing_table
            .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unknown,
        "the route itself is not the suspect when the local client's request died",
    );
}

#[test]
fn a_boundary_arrival_interface_rediscovers_without_the_unresponsive_mark() {
    use crate::routing::links::transported::TransportedLink;
    use crate::routing::links::LinkId;

    let received = InterfaceId::new([0xA1; 8]);
    let away = InterfaceId::new([0xB2; 8]);
    let learn_view = [routable_descriptor(received), routable_descriptor(away)];
    let fire_view = [
        moded(InterfaceMode::Boundary, routable_descriptor(received)),
        routable_descriptor(away),
    ];

    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: received,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&learn_view),
        &mut |_| {},
        None,
    );
    let destination = DestinationHash::new(
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
            .try_into()
            .unwrap(),
    );

    engine
        .transported_links
        .track(TransportedLink {
            link_id: LinkId::new([0x5C; 16]),
            destination,
            mode: LinkMode::Aes256Cbc,
            next_hop: None,
            next_hop_interface: away,
            received_interface: received,
            taken_hops: 1,
            remaining_hops: 1,
            validated_by_proof: false,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(7_000),
        })
        .unwrap();

    let mut sent = std::vec::Vec::new();
    let _ = engine.fire_due_link_deadlines(
        InstantMillis(7_000),
        AttachedInterfaces::new(&fire_view),
        &mut |bytes: &mut [u8]| bytes.fill(0x5A),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                sent.push(target);
            }
        },
    );

    assert_eq!(
        sent,
        std::vec![away],
        "the re-request still fires away from the dead link's lane",
    );
    assert_eq!(
        engine
            .routing_table
            .existing_route_for(&destination, AttachedInterfaces::new(&fire_view))
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unknown,
        "a boundary interface's routes are not marked unresponsive over one silent link",
    );
}

#[test]
fn a_may_return_departure_holds_the_bounced_peers_routes_through_the_grace() {
    use crate::engine::{Departure, DEPARTED_INTERFACE_GRACE_MS};

    let source = InterfaceId::new([0xA1; 8]);
    let other = InterfaceId::new([0xB2; 8]);
    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&[routable_descriptor(source), routable_descriptor(other)]),
        &mut |_| {},
        None,
    );
    assert_eq!(engine.route_count(), 1);

    engine.interface_departed(source, Departure::MayReturn, InstantMillis(2_000));
    let without_source = [routable_descriptor(other)];
    engine.cull_expired_routes(
        InstantMillis(2_001),
        AttachedInterfaces::new(&without_source),
        &mut |_| {},
    );
    assert_eq!(
        engine.route_count(),
        1,
        "within the grace the bounced peer's route holds",
    );
    assert_eq!(
        engine.route_expiry_wake(AttachedInterfaces::new(&without_source)),
        WakeSchedule::At(InstantMillis(2_000 + DEPARTED_INTERFACE_GRACE_MS)),
        "the wake names the grace deadline",
    );

    let mut journal = std::vec::Vec::new();
    engine.cull_expired_routes(
        InstantMillis(2_000 + DEPARTED_INTERFACE_GRACE_MS),
        AttachedInterfaces::new(&without_source),
        &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::RouteRemoved {
                destination,
                cause: RouteRemovalCause::InterfaceGone,
            }) = reaction
            {
                journal.push(destination);
            }
        },
    );
    assert_eq!(
        engine.route_count(),
        0,
        "past the grace the orphan finally culls",
    );
    assert_eq!(journal.len(), 1, "and its removal still names its cause");
}

#[cfg(feature = "std")]
#[test]
fn a_growable_hosts_route_index_rebuilds_for_departure_warmth() {
    use crate::engine::{Departure, DEPARTED_INTERFACE_GRACE_MS};
    use crate::storage::GrowableHeap;

    let source = InterfaceId::new([0xA1; 8]);
    let other = InterfaceId::new([0xB2; 8]);
    let attached = [routable_descriptor(source), routable_descriptor(other)];
    let mut engine = EngineState::<GrowableHeap>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&attached),
        &mut |_| {},
        None,
    );
    assert!(matches!(
        engine.route_expiry_wake(AttachedInterfaces::new(&attached)),
        WakeSchedule::At(_)
    ));

    engine.interface_departed(source, Departure::MayReturn, InstantMillis(2_000));
    let without_source = [routable_descriptor(other)];
    assert_eq!(
        engine.route_expiry_wake(AttachedInterfaces::new(&without_source)),
        WakeSchedule::At(InstantMillis(2_000 + DEPARTED_INTERFACE_GRACE_MS))
    );
}

#[test]
fn a_forgotten_departure_culls_the_routes_at_once() {
    use crate::engine::Departure;

    let source = InterfaceId::new([0xA1; 8]);
    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&[routable_descriptor(source)]),
        &mut |_| {},
        None,
    );
    assert_eq!(engine.route_count(), 1);

    engine.interface_departed(source, Departure::Forgotten, InstantMillis(2_000));
    let without_source = [routable_descriptor(InterfaceId::new([0xEE; 8]))];
    engine.cull_expired_routes(
        InstantMillis(2_001),
        AttachedInterfaces::new(&without_source),
        &mut |_| {},
    );
    assert_eq!(
        engine.route_count(),
        0,
        "a deliberate forget keeps the reference's eager cull",
    );
}

#[test]
fn a_returned_interface_resumes_normal_route_aging() {
    use crate::engine::{Departure, DEPARTED_INTERFACE_GRACE_MS};

    let source = InterfaceId::new([0xA1; 8]);
    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: source,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&[routable_descriptor(source)]),
        &mut |_| {},
        None,
    );

    engine.interface_departed(source, Departure::MayReturn, InstantMillis(2_000));
    let interfaces = [routable_descriptor(source)];
    engine.cull_expired_routes(
        InstantMillis(2_000 + DEPARTED_INTERFACE_GRACE_MS + 1),
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
    );
    assert_eq!(
        engine.route_count(),
        1,
        "back among the attached interfaces, the stale grace entry is ignored and mode expiry governs",
    );
}

#[test]
fn a_recently_requested_destination_holds_off_the_overdue_links_path_request() {
    use crate::routing::links::transported::TransportedLink;
    use crate::routing::links::LinkId;

    let received = InterfaceId::new([0xA1; 8]);
    let away = InterfaceId::new([0xB2; 8]);
    let interfaces = [routable_descriptor(received), routable_descriptor(away)];

    let mut engine = EngineState::<TestStorageLayout>::default();
    let mut raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let _ = engine.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: received,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&interfaces),
        &mut |_| {},
        None,
    );
    let destination = DestinationHash::new(
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
            .try_into()
            .unwrap(),
    );

    engine
        .transported_links
        .track(TransportedLink {
            link_id: LinkId::new([0x5C; 16]),
            destination,
            mode: LinkMode::Aes256Cbc,
            next_hop: None,
            next_hop_interface: away,
            received_interface: received,
            taken_hops: 1,
            remaining_hops: 1,
            validated_by_proof: false,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(7_000),
        })
        .unwrap();

    let asked_well_within_the_throttle_window = InstantMillis(2_000);
    engine
        .recent_path_requests
        .mark_seen_at(destination, asked_well_within_the_throttle_window);

    let mut sent = std::vec::Vec::new();
    let _ = engine.fire_due_link_deadlines(
        InstantMillis(7_000),
        AttachedInterfaces::new(&interfaces),
        &mut |bytes: &mut [u8]| bytes.fill(0x5A),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                sent.push(target);
            }
        },
    );

    assert_eq!(
        engine
            .routing_table
            .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unknown,
        "the throttle holds off the unresponsive mark too, not only the resend",
    );
    assert!(
        sent.is_empty(),
        "a path request inside the minimum interval suppresses the re-request",
    );
}
