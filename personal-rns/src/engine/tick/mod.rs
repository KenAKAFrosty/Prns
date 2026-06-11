use crate::engine::egress::firable_on;
use crate::engine::{
    Directive, EgressDirective, EngineReaction, EngineState, InstantMillis, WakeSchedules,
};
use crate::interfaces::InterfaceConfig;
use crate::routing::announce::defaults::{
    MAX_ANNOUNCE_REBROADCASTS, REBROADCAST_RETRANSMIT_INTERVAL_MS,
};
use crate::routing::announce::schedule::ScheduledAnnounceQueue as _;
use crate::routing::storage::EngineStorage;
use crate::wire::MTU;

impl<S: EngineStorage> EngineState<S> {
    /// One [`EgressDirective`] per (scheduled announce due at `now` × interface it fires on):
    /// a `directed_to` entry answers only its one target, else the engine's flood fan-out
    /// ([`firable_on`]). The caller takes each named target to its handle. The reactor's
    /// [`fire_due_scheduled_announces`](Self::fire_due_scheduled_announces) serializes
    /// and drains it.
    fn due_scheduled_announce_directives<'v>(
        &'v self,
        now: InstantMillis,
        view: &'v [InterfaceConfig],
    ) -> impl Iterator<Item = EgressDirective<'v>> + 'v {
        self.scheduled_announces
            .iter()
            .filter(move |scheduled| scheduled.due_at.0 <= now.0)
            .filter_map(move |scheduled| {
                let via = self.transport_id?;
                let retained = self
                    .routing_table
                    .retained_announce_for(&scheduled.destination)?;
                Some((
                    retained.announce,
                    retained.hops,
                    via,
                    scheduled.source_interface,
                    scheduled.directed_to,
                ))
            })
            .flat_map(move |(announce, emit_hops, via, source, directed_to)| {
                view.iter()
                    .filter(move |descriptor| match directed_to {
                        Some(target) => {
                            descriptor.id == target && descriptor.capabilities.allows_transport()
                        }
                        None => firable_on(descriptor, source),
                    })
                    .map(move |descriptor| EgressDirective::ReemitAnnounce {
                        announce: announce.clone(),
                        emit_hops,
                        via,
                        target: descriptor.id,
                    })
            })
    }

    /// Fire every scheduled announce due at `now`: serialize each onto a scratch buffer
    /// lent to `sink` as a [`Directive::SendAnnounce`], then advance the fired entries — the
    /// reactor's timer edge drives this directly, reading and re-arming in the one pass. Each
    /// due entry re-emits until [`MAX_ANNOUNCE_REBROADCASTS`], re-armed
    /// [`REBROADCAST_RETRANSMIT_INTERVAL_MS`] out, then drops. Returns the scheduled-announce
    /// lane's new soonest deadline as a [`WakeSchedules`] delta.
    pub fn fire_due_scheduled_announces(
        &mut self,
        now: InstantMillis,
        view: &[InterfaceConfig],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        for egress in self.due_scheduled_announce_directives(now, view) {
            let mut buf = [0u8; MTU];
            if let Ok(written) = egress.to_wire(&mut buf) {
                sink(EngineReaction::Directive(Directive::SendAnnounce {
                    target: egress.target(),
                    bytes: &buf[..written],
                    hops: egress.emit_hops(),
                }));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::LaneWake;
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::wire::{DestinationType, PacketType, PropagationType, WirePacketHeader};

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
        let view = [routable_descriptor(InterfaceId::new([0xFE; 16]))];

        let arrival = InstantMillis(1_000);
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 16]),
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
                source_interface: InterfaceId::new([0u8; 16]),
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
                    source_interface: InterfaceId::new([0u8; 16]),
                    bytes: &mut raw,
                },
                TEST_ENTROPY,
                &transporting_view(),
            )
        else {
            panic!("the announce is accepted");
        };

        let target = InterfaceId::new([0xAA; 16]);
        state.scheduled_announces.schedule_directed(
            accepted.destination,
            InstantMillis(2_000),
            target,
            accepted.hops,
        );

        let view = [
            routable_descriptor(target),
            routable_descriptor(InterfaceId::new([0xBB; 16])),
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
                source_interface: InterfaceId::new([0u8; 16]),
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
        let source = InterfaceId::new([0u8; 16]);
        let other = InterfaceId::new([0xFE; 16]);
        let view = [repeating_descriptor(source), routable_descriptor(other)];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, &view),
            std::vec![source, other],
        );
    }

    #[test]
    fn a_cross_interface_only_source_is_left_out_of_its_own_rebroadcast_fan() {
        let source = InterfaceId::new([0u8; 16]);
        let other = InterfaceId::new([0xFE; 16]);
        let view = [routable_descriptor(source), routable_descriptor(other)];

        let mut state = transporting_node();
        assert_eq!(rebroadcast_fan_for(&mut state, &view), std::vec![other]);
    }

    #[test]
    fn our_own_repeat_echoed_back_is_deduplicated() {
        use crate::engine::{AnnounceIngest, IngestPacketOutcome};

        let source = InterfaceId::new([0u8; 16]);
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
        let source = InterfaceId::new([0u8; 16]);
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

        let source = InterfaceId::new([0u8; 16]);
        let mut leaf = routable_descriptor(InterfaceId::new([0xFE; 16]));
        leaf.capabilities.egress = EgressCapability::Enabled(TransportCapability::NoTransport);
        let view = [routable_descriptor(source), leaf];

        let mut state = transporting_node();
        assert_eq!(rebroadcast_fan_for(&mut state, &view), std::vec![]);
    }

    #[test]
    fn scheduled_announces_are_not_emitted_before_their_due_time() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let view = [routable_descriptor(InterfaceId::new([0xFE; 16]))];
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

        let view = [routable_descriptor(InterfaceId::new([0xFE; 16]))];
        for state in [&mut left, &mut right] {
            let _ = state.ingest_packet(
                InboundPacket {
                    arrived_at: arrival,
                    source_interface: InterfaceId::new([0u8; 16]),
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
        let target = InterfaceId::new([0xFE; 16]);
        let view = [routable_descriptor(target)];

        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 16]),
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
}
