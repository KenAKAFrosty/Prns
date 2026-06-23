use crate::engine::egress::firable_on;
use crate::engine::{
    Directive, EgressDirective, EngineReaction, EngineState, FanTarget, InstantMillis,
    WakeSchedules,
};
use crate::interfaces::{InterfaceConfig, InterfaceId, InterfaceKind};
use crate::routing::announce::defaults::{
    MAX_ANNOUNCE_REBROADCASTS, REBROADCAST_RETRANSMIT_INTERVAL_MS,
};
use crate::routing::announce::schedule::ScheduledAnnounceQueue as _;
use crate::storage::StorageLayout;
use crate::wire::BROADCAST_MTU;

impl<S: StorageLayout> EngineState<S> {
    /// Fire every scheduled announce due at `now`: serialize each once onto a scratch buffer, then
    /// fan it across the interfaces it fires on. A `directed_to` entry answers only its one target;
    /// otherwise the flood fan-out ([`firable_on`]) — source-withheld, mode-gated, transport-only.
    /// A dedicated 1:1 interface earns its own [`Directive::SendAnnounce`]; a whole fleet collapses
    /// to one [`Directive::BroadcastAnnounce`] the supervisor fans across its peers, so a shared lane
    /// never carries a frame per member. Then advance the fired entries — each re-emits until
    /// [`MAX_ANNOUNCE_REBROADCASTS`], re-armed [`REBROADCAST_RETRANSMIT_INTERVAL_MS`] out, then
    /// drops. Returns the scheduled-announce lane's new soonest deadline as a [`WakeSchedules`] delta.
    pub fn fire_due_scheduled_announces(
        &mut self,
        now: InstantMillis,
        view: &[InterfaceConfig],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        if let Some(via) = self.transport_id {
            let scheduled = &self.scheduled_announces;
            let routing = &self.routing_table;
            for entry in scheduled.iter().filter(|s| s.due_at.0 <= now.0) {
                let Some(retained) = routing.retained_announce_for(&entry.destination) else {
                    continue;
                };
                let emit_hops = retained.hops;
                let source = entry.source_interface;
                let directed_to = entry.directed_to;
                let mut buf = [0u8; BROADCAST_MTU];
                let directive = EgressDirective::ReemitAnnounce {
                    announce: retained.announce.clone(),
                    emit_hops,
                    via,
                    target: source,
                    path_response: directed_to.is_some(),
                };
                let Ok(written) = directive.to_wire(&mut buf) else {
                    continue;
                };
                let bytes = &buf[..written];
                let next_hop_mode = view.iter().find(|c| c.id == source).map(|c| c.mode);
                let mut fleets_emitted: u16 = 0;
                for descriptor in view {
                    let eligible = match directed_to {
                        Some(target) => {
                            descriptor.id == target && descriptor.capabilities.allows_transport()
                        }
                        None => firable_on(descriptor, source, next_hop_mode),
                    };
                    if !eligible {
                        continue;
                    }
                    match descriptor
                        .id
                        .kind()
                        .and_then(InterfaceKind::supervisor_kind)
                    {
                        Some(supervisor) => {
                            let bit = 1u16 << (supervisor as u8);
                            if fleets_emitted & bit == 0 {
                                fleets_emitted |= bit;
                                let fan = fleet_announce_fan(view, supervisor, source, directed_to);
                                if fleet_fan_selects_any(view, supervisor, fan) {
                                    sink(EngineReaction::Directive(Directive::BroadcastAnnounce {
                                        supervisor,
                                        fan,
                                        bytes,
                                        hops: emit_hops,
                                    }));
                                }
                            }
                        }
                        None => sink(EngineReaction::Directive(Directive::SendAnnounce {
                            target: descriptor.id,
                            bytes,
                            hops: emit_hops,
                        })),
                    }
                }
            }
        }
        self.scheduled_announces.advance_due_retransmits(
            now,
            REBROADCAST_RETRANSMIT_INTERVAL_MS,
            MAX_ANNOUNCE_REBROADCASTS,
        );
        WakeSchedules {
            scheduled_announces: self.scheduled_announces_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }
}

/// The fan a fleet's announce broadcast carries, reconstructed from the same rule `firable_on`
/// applies per member: a directed answer reaches only its target; a flood reaches every member
/// except the source it arrived on — unless that source interface permits a same-interface repeat,
/// in which case it rejoins the fan. A source that is not a member of this fleet excludes nothing.
/// Sound because a supervisor's members are uniform, so the per-member verdict differs only by the
/// source-withhold the [`FanTarget`] captures.
fn fleet_announce_fan(
    view: &[InterfaceConfig],
    supervisor: InterfaceKind,
    source: InterfaceId,
    directed_to: Option<InterfaceId>,
) -> FanTarget {
    if let Some(target) = directed_to {
        return FanTarget::Only(target);
    }
    if source.kind() != supervisor.member_kind() {
        return FanTarget::All;
    }
    let source_repeats = view
        .iter()
        .find(|c| c.id == source)
        .is_some_and(|c| c.capabilities.allows_same_interface_repeat());
    if source_repeats {
        FanTarget::All
    } else {
        FanTarget::AllExcept(source)
    }
}

/// Whether a fleet broadcast's `fan` selects at least one current member of `supervisor`'s fleet. A
/// flood whose only would-be recipient is the source it arrived on (`AllExcept` selecting nobody on
/// a single-member fleet) is a no-op that would still occupy the supervisor's one shared lane, so the
/// caller withholds it rather than queue a frame that reaches nobody.
fn fleet_fan_selects_any(
    view: &[InterfaceConfig],
    supervisor: InterfaceKind,
    fan: FanTarget,
) -> bool {
    let Some(member_kind) = supervisor.member_kind() else {
        return false;
    };
    view.iter()
        .filter(|descriptor| descriptor.id.kind() == Some(member_kind))
        .any(|descriptor| match fan {
            FanTarget::All => true,
            FanTarget::Only(target) => descriptor.id == target,
            FanTarget::AllExcept(excluded) => descriptor.id != excluded,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::LaneWake;
    use crate::interfaces::{InboundPacket, InterfaceId, InterfaceKind, InterfaceMode};
    use crate::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::wire::{DestinationType, PacketType, PropagationType, WirePacketHeader};

    #[test]
    fn a_fleet_flood_to_a_lone_source_member_selects_nobody() {
        let source = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x42, 0, 0, 0, 0, 0, 0]);
        let other = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x77, 0, 0, 0, 0, 0, 0]);

        let lone = [routable_descriptor(source)];
        assert!(
            !fleet_fan_selects_any(
                &lone,
                InterfaceKind::BluetoothAuto,
                FanTarget::AllExcept(source)
            ),
            "a flood whose fleet's only member is the source it arrived on reaches nobody"
        );

        let pair = [routable_descriptor(source), routable_descriptor(other)];
        assert!(
            fleet_fan_selects_any(
                &pair,
                InterfaceKind::BluetoothAuto,
                FanTarget::AllExcept(source)
            ),
            "with a second peer present the flood reaches it"
        );
        assert!(
            fleet_fan_selects_any(&lone, InterfaceKind::BluetoothAuto, FanTarget::All),
            "an unconditional flood reaches the lone member"
        );
        assert!(
            !fleet_fan_selects_any(
                &[routable_descriptor(InterfaceId::new([0xFE; 8]))],
                InterfaceKind::BluetoothAuto,
                FanTarget::All
            ),
            "a flood selects nobody when the view holds no member of the fleet's kind"
        );
    }

    #[test]
    fn a_fresh_drive_is_deterministic_and_emits_nothing() {
        let mut left: EngineState<Cap> = EngineState::<Cap>::default();
        let mut right: EngineState<Cap> = EngineState::<Cap>::default();

        let (left_out, left_bytes) = tick_capture(&mut left, InstantMillis(1_000), &[]);
        let (right_out, right_bytes) = tick_capture(&mut right, InstantMillis(1_000), &[]);

        assert_eq!(observable_state(&left), observable_state(&right));
        assert_eq!(left_out, right_out);
        assert_eq!(left_out.egress_directive_count, 0);
        assert!(left_bytes.is_empty());
        assert_eq!(left_bytes, right_bytes);
    }

    #[test]
    fn accepted_announces_schedule_a_rebroadcast_and_tick_emits_them() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let view = [routable_descriptor(InterfaceId::new([0xFE; 8]))];

        let arrival = InstantMillis(1_000);
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(1));
        assert_eq!(state.scheduled_announce_count(), 1);

        let (tick_out, emitted) = tick_capture(
            &mut state,
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            &view,
        );
        assert_eq!(tick_out.egress_directive_count, 1);
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
        assert_eq!(header.destination, original.destination);
        let original_payload = WirePacketHeader::parse(&raw).unwrap().1;
        assert_eq!(payload, original_payload);
    }

    #[test]
    fn a_rebroadcast_reproduces_the_rns_1_3_1_retransmitted_wire() {
        let mut heard = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let mut state = transporting_node();
        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut heard,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let (_, emitted) = tick_capture(
            &mut state,
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            &transporting_view(),
        );
        assert_eq!(
            emitted,
            std::vec![hx(RNS_1_3_1_RETRANSMITTED_ANNOUNCE)],
            "our retransmission must be byte-identical to the reference's own",
        );
    }

    #[test]
    fn a_directed_scheduled_announce_fires_only_to_its_target_interface() {
        use crate::engine::{AnnounceIngest, IngestPacketOutcome};

        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let IngestPacketOutcome::Announce(AnnounceIngest::Accepted(accepted)) = state
            .ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0u8; 8]),
                    bytes: &mut raw,
                },
                TEST_ENTROPY,
                &transporting_view(),
            )
        else {
            panic!("the announce is accepted");
        };

        let target = InterfaceId::new([0xAA; 8]);
        state.scheduled_announces.schedule_directed(
            accepted.destination,
            InstantMillis(2_000),
            target,
            accepted.hops,
        );

        let view = [
            routable_descriptor(target),
            routable_descriptor(InterfaceId::new([0xBB; 8])),
        ];
        let mut targets = std::vec::Vec::new();
        state.fire_due_scheduled_announces(InstantMillis(2_000), &view, &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction {
                targets.push(target);
            }
        });
        assert_eq!(
            targets,
            std::vec![target],
            "a directed answer reaches only its target, where a flood would reach both interfaces",
        );
    }

    fn rebroadcast_fan_for(
        state: &mut EngineState<Cap>,
        view: &[InterfaceConfig],
    ) -> std::vec::Vec<InterfaceId> {
        let mut raw = hx(RAW_ANNOUNCE);
        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let mut targets = std::vec::Vec::new();
        let _ = state.fire_due_scheduled_announces(
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            view,
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction
                {
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
        let view = [repeating_descriptor(source), routable_descriptor(other)];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, &view),
            std::vec![source, other],
        );
    }

    #[test]
    fn a_cross_interface_only_source_is_left_out_of_its_own_rebroadcast_fan() {
        let source = InterfaceId::new([0u8; 8]);
        let other = InterfaceId::new([0xFE; 8]);
        let view = [routable_descriptor(source), routable_descriptor(other)];

        let mut state = transporting_node();
        assert_eq!(rebroadcast_fan_for(&mut state, &view), std::vec![other]);
    }

    #[test]
    fn our_own_repeat_echoed_back_is_deduplicated() {
        use crate::engine::{AnnounceIngest, IngestPacketOutcome};

        let source = InterfaceId::new([0u8; 8]);
        let view = [repeating_descriptor(source)];
        let mut state = transporting_node();
        let fan = rebroadcast_fan_for(&mut state, &view);
        assert_eq!(fan, std::vec![source]);

        let mut echo = hx(RAW_ANNOUNCE);
        echo[1] += 1;
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(5_000),
                source_interface: source,
                bytes: &mut echo,
            },
            TEST_ENTROPY,
            &view,
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
        let view = [repeating_descriptor(source)];
        let mut state = transporting_node();
        let fan = rebroadcast_fan_for(&mut state, &view);
        assert_eq!(fan, std::vec![source]);
        assert_eq!(
            state.scheduled_announce_count(),
            1,
            "after one emission the entry is re-armed for its second rebroadcast",
        );

        let mut echo = hx(RAW_ANNOUNCE);
        echo[1] += 2;
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(5_000),
                source_interface: source,
                bytes: &mut echo,
            },
            TEST_ENTROPY,
            &view,
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
        let view = [routable_descriptor(source), leaf];

        let mut state = transporting_node();
        assert_eq!(rebroadcast_fan_for(&mut state, &view), std::vec![]);
    }

    fn moded(mode: InterfaceMode, descriptor: InterfaceConfig) -> InterfaceConfig {
        InterfaceConfig { mode, ..descriptor }
    }

    #[test]
    fn an_access_point_egress_interface_is_withheld_from_the_rebroadcast_fan() {
        let source = InterfaceId::new([0u8; 8]);
        let ap = InterfaceId::new([0xFE; 8]);
        let view = [
            repeating_descriptor(source),
            moded(InterfaceMode::AccessPoint, routable_descriptor(ap)),
        ];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, &view),
            std::vec![source],
            "an access-point interface never carries an announce rebroadcast",
        );
    }

    #[test]
    fn a_roaming_egress_interface_is_withheld_toward_a_roaming_learned_route() {
        let source = InterfaceId::new([0u8; 8]);
        let roaming_out = InterfaceId::new([0xFE; 8]);
        let other = InterfaceId::new([0xAB; 8]);
        let view = [
            moded(InterfaceMode::Roaming, repeating_descriptor(source)),
            moded(InterfaceMode::Roaming, routable_descriptor(roaming_out)),
            routable_descriptor(other),
        ];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, &view),
            std::vec![other],
            "a roaming interface withholds a roaming-learned route; a full interface carries it",
        );
    }

    #[test]
    fn a_roaming_egress_interface_carries_a_full_learned_route() {
        let source = InterfaceId::new([0u8; 8]);
        let roaming_out = InterfaceId::new([0xFE; 8]);
        let view = [
            repeating_descriptor(source),
            moded(InterfaceMode::Roaming, routable_descriptor(roaming_out)),
        ];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, &view),
            std::vec![source, roaming_out],
        );
    }

    #[test]
    fn a_boundary_egress_carries_a_boundary_learned_route_where_a_roaming_egress_will_not() {
        let source = InterfaceId::new([0u8; 8]);
        let boundary_out = InterfaceId::new([0xFE; 8]);
        let roaming_out = InterfaceId::new([0xAB; 8]);
        let view = [
            moded(InterfaceMode::Boundary, repeating_descriptor(source)),
            moded(InterfaceMode::Boundary, routable_descriptor(boundary_out)),
            moded(InterfaceMode::Roaming, routable_descriptor(roaming_out)),
        ];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, &view),
            std::vec![source, boundary_out],
            "boundary carries a boundary-learned route; roaming withholds the same route",
        );
    }

    #[test]
    fn scheduled_announces_are_not_emitted_before_their_due_time() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let view = [routable_descriptor(InterfaceId::new([0xFE; 8]))];
        let (tick_out, emitted) = tick_capture(&mut state, InstantMillis(arrival.0 - 1), &view);
        assert_eq!(tick_out.egress_directive_count, 0);
        assert!(emitted.is_empty());
        assert_eq!(state.scheduled_announce_count(), 1);
    }

    #[test]
    fn same_inputs_produce_byte_identical_emissions_on_two_engines() {
        let mut raw = hx(RAW_ANNOUNCE);
        let now = InstantMillis(5_000);
        let arrival = InstantMillis(1_000);

        let mut left = transporting_node();
        let mut right = transporting_node();

        let view = [routable_descriptor(InterfaceId::new([0xFE; 8]))];
        for state in [&mut left, &mut right] {
            let _ = state.ingest_packet(
                InboundPacket {
                    arrived_at: arrival,
                    source_interface: InterfaceId::new([0u8; 8]),
                    bytes: &mut raw,
                },
                TEST_ENTROPY,
                &transporting_view(),
            );
        }
        let (left_tick, left_bytes) = tick_capture(&mut left, now, &view);
        let (right_tick, right_bytes) = tick_capture(&mut right, now, &view);

        assert_eq!(observable_state(&left), observable_state(&right));
        assert_eq!(left_tick, right_tick);
        assert_eq!(left_bytes, right_bytes);
        assert_eq!(left_bytes.len(), 1);
    }

    #[test]
    fn fire_due_scheduled_announces_emits_then_re_arms_until_the_cap() {
        fn fire(
            state: &mut EngineState<Cap>,
            now: InstantMillis,
            view: &[InterfaceConfig],
        ) -> (std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>, LaneWake) {
            let mut sent = std::vec::Vec::new();
            let delta = state.fire_due_scheduled_announces(now, view, &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce {
                    target, bytes, ..
                }) = reaction
                {
                    sent.push((target, bytes.to_vec()));
                }
            });
            (sent, delta.scheduled_announces)
        }

        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let target = InterfaceId::new([0xFE; 8]);
        let view = [routable_descriptor(target)];

        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let first_due = InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
        let (sent, lane) = fire(&mut state, first_due, &view);
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
            lane,
            LaneWake::At(InstantMillis(
                first_due.0 + REBROADCAST_RETRANSMIT_INTERVAL_MS
            )),
            "the lane is re-armed one retransmit interval out",
        );
        let (header, _) = WirePacketHeader::parse(&sent[0].1).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        let original = WirePacketHeader::parse(&hx(RAW_ANNOUNCE)).unwrap().0;
        assert_eq!(
            header.hops,
            original.hops + 1,
            "the rebroadcast bumps the hop count"
        );
        let first_bytes = sent[0].1.clone();

        let second_due = InstantMillis(first_due.0 + REBROADCAST_RETRANSMIT_INTERVAL_MS);
        let (sent, lane) = fire(&mut state, second_due, &view);
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
        assert_eq!(lane, LaneWake::Idle, "no rebroadcasts remain after the cap");
    }

    #[test]
    fn an_ignored_echo_that_cancels_a_rebroadcast_reports_the_emptied_lane() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let target = InterfaceId::new([0xFE; 8]);
        let view = [routable_descriptor(target)];

        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let first_due = InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
        let mut rebroadcast = std::vec::Vec::new();
        let _ = state.fire_due_scheduled_announces(first_due, &view, &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { bytes, .. }) = reaction {
                rebroadcast = bytes.to_vec();
            }
        });
        assert_eq!(state.scheduled_announce_count(), 1);
        assert!(!rebroadcast.is_empty(), "the fire emitted a rebroadcast to echo back");

        let echo = |state: &mut EngineState<Cap>, now: u64| -> LaneWake {
            let mut bytes = rebroadcast.clone();
            state
                .ingest_packet_into(
                    InboundPacket {
                        arrived_at: InstantMillis(now),
                        source_interface: InterfaceId::new([0u8; 8]),
                        bytes: &mut bytes,
                    },
                    TEST_ENTROPY,
                    &transporting_view(),
                    InstantMillis(now),
                    &mut |bytes: &mut [u8]| bytes.fill(0),
                    &mut |_| false,
                    &mut |_| {},
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
            LaneWake::Idle,
            "an ignored echo that empties the queue reports Idle, not a stale Unchanged",
        );
        assert_eq!(
            second,
            state.scheduled_announces_wake(),
            "the ingest delta agrees with a full wake recompute (no reactor drift)",
        );
    }
}
