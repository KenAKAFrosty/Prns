use crate::engine::egress::firable_on;
use crate::engine::{
    Directive, EgressDirective, EngineReaction, EngineState, InstantMillis, Journaled, LaneWake,
    WakeSchedules,
};
use crate::interfaces::InterfaceConfig;
use crate::routing::announce::defaults::{
    jitter_offset_for, JitterSeed, DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
};
use crate::routing::announce::held_cache::HeldAnnounces as _;
use crate::routing::announce::schedule::RebroadcastQueue as _;
use crate::routing::announce::{AnnounceAcceptanceDecision, AnnounceAcceptanceInput};
use crate::routing::storage::EngineStorage;
use crate::routing::UpsertRouteOutcome;
use crate::wire::{DestinationType, MTU};

#[must_use]
pub struct TickOutput<'a, S: EngineStorage> {
    state: &'a mut EngineState<S>,
    now: InstantMillis,
    recovered_from_held_count: usize,
}

impl<'a, S: EngineStorage> TickOutput<'a, S> {
    /// The number of announces this tick re-emits — one per destination whose
    /// rebroadcast is due, independent of how many interfaces each fans to.
    pub fn egress_directive_count(&self) -> usize {
        let now = self.now;
        self.state
            .pending_rebroadcasts
            .as_slice()
            .iter()
            .filter(|scheduled| scheduled.due_at.0 <= now.0)
            .count()
    }

    pub const fn recovered_from_held_count(&self) -> usize {
        self.recovered_from_held_count
    }

    /// Resolve this tick's rebroadcasts against the descriptor `view` the runtime
    /// holds, yielding one [`EgressDirective`] per (due destination × interface the
    /// engine decides to fire on). The fan-out call is the engine's alone
    /// (`firable_on`); the runtime takes each named target to its handle and sends.
    pub fn egress_directives<'v>(
        &'v self,
        view: &'v [InterfaceConfig],
    ) -> impl Iterator<Item = EgressDirective<'v>> + 'v {
        self.state.due_rebroadcast_directives(self.now, view)
    }

    pub fn commit(mut self) {
        self.commit_in_place();
    }

    fn commit_in_place(&mut self) {
        self.state.pending_rebroadcasts.drain_due(self.now);
    }
}

impl<S: EngineStorage> Drop for TickOutput<'_, S> {
    fn drop(&mut self) {
        self.commit_in_place();
    }
}

impl<S: EngineStorage> EngineState<S> {
    pub fn tick(
        &mut self,
        now: InstantMillis,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
    ) -> TickOutput<'_, S> {
        self.tick_count = self.tick_count.saturating_add(1);
        let mut recovered_from_held_count = 0;
        let _ = self.recover_held_announces(jitter, interfaces, &mut |reaction| {
            if matches!(
                reaction,
                EngineReaction::Journaled(Journaled::AnnounceHeard { .. })
            ) {
                recovered_from_held_count += 1;
            }
        });
        TickOutput {
            state: self,
            now,
            recovered_from_held_count,
        }
    }

    /// Retry every held announce against the now-unblocked routing arena. Each that lands
    /// is journaled `AnnounceHeard` — a hold defers the hearing, it never drops it — and,
    /// if we transport and an interface can carry it, scheduled for rebroadcast. Drains the
    /// whole cache, so the held lane ends `Idle`; returns that and the rebroadcast lane's
    /// new soonest deadline as a [`WakeSchedules`] delta.
    pub fn recover_held_announces(
        &mut self,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        use crate::routing::announce::held_cache::HoldReason;
        while let Some(held) = self.held_announces_cache.take_next() {
            match held.reason() {
                HoldReason::RoutingArenaPressure => {
                    let announce = held.announce();
                    let arrival = held.arrived_at();
                    let received_hops = held.received_hops();
                    let source_interface = held.source_interface();
                    let decision = AnnounceAcceptanceInput {
                        packet_hops: received_hops,
                        announce_id: announce.announce_id,
                        destination_is_self_or_upstream: self
                            .upstream_app_destinations
                            .lookup(&announce.destination, DestinationType::Single)
                            .is_some(),
                        existing_route: self
                            .routing_table
                            .existing_route_for(&announce.destination),
                        arrived_at: arrival,
                    }
                    .determine_acceptance();
                    if matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
                        let outcome = self.routing_table.upsert_route(
                            received_hops,
                            arrival,
                            source_interface,
                            held.next_hop(),
                            &announce,
                        );
                        if matches!(
                            outcome,
                            UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated
                        ) {
                            sink(EngineReaction::Journaled(Journaled::AnnounceHeard {
                                destination: announce.destination,
                                hops: received_hops,
                                source_interface,
                            }));
                            if self.transport_id.is_some()
                                && interfaces
                                    .iter()
                                    .any(|descriptor| descriptor.capabilities.allows_transport())
                            {
                                let offset = jitter_offset_for(
                                    jitter,
                                    &announce.destination,
                                    DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                                );
                                self.pending_rebroadcasts.schedule(
                                    announce.destination,
                                    InstantMillis(arrival.0.saturating_add(offset)),
                                    source_interface,
                                    received_hops,
                                );
                            }
                        }
                    }
                }
            }
        }
        WakeSchedules {
            held_announces: LaneWake::Idle,
            rebroadcast_announces: self.rebroadcast_lane(),
            ..WakeSchedules::UNCHANGED
        }
    }

    /// One [`EgressDirective`] per (rebroadcast due at `now` × interface the engine elects
    /// to fire it on). The fan-out decision is the engine's alone ([`firable_on`]); the
    /// caller takes each named target to its handle. Shared by [`tick`](Self::tick)'s
    /// [`TickOutput`] and the reactor's
    /// [`fire_due_announce_rebroadcasts`](Self::fire_due_announce_rebroadcasts).
    fn due_rebroadcast_directives<'v>(
        &'v self,
        now: InstantMillis,
        view: &'v [InterfaceConfig],
    ) -> impl Iterator<Item = EgressDirective<'v>> + 'v {
        self.pending_rebroadcasts
            .as_slice()
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
                ))
            })
            .flat_map(move |(announce, emit_hops, via, source)| {
                view.iter()
                    .filter(move |descriptor| firable_on(descriptor, source))
                    .map(move |descriptor| EgressDirective::ReemitAnnounce {
                        announce: announce.clone(),
                        emit_hops,
                        via,
                        target: descriptor.id,
                    })
            })
    }

    /// Fire every announce rebroadcast due at `now`: serialize each onto a scratch buffer
    /// lent to `sink` as a [`Directive::SendAnnounce`], then clear the fired entries. The
    /// sink-shaped face of a rebroadcast tick — the reactor's timer edge drains it here,
    /// the legacy runtime drains the same work through [`TickOutput`]. Returns the
    /// rebroadcast lane's new soonest deadline as a [`WakeSchedules`] delta.
    pub fn fire_due_announce_rebroadcasts(
        &mut self,
        now: InstantMillis,
        view: &[InterfaceConfig],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        for egress in self.due_rebroadcast_directives(now, view) {
            let mut buf = [0u8; MTU];
            if let Ok(written) = egress.to_wire(&mut buf) {
                sink(EngineReaction::Directive(Directive::SendAnnounce {
                    target: egress.target(),
                    bytes: &buf[..written],
                    hops: egress.emit_hops(),
                }));
            }
        }
        self.pending_rebroadcasts.drain_due(now);
        WakeSchedules {
            rebroadcast_announces: self.rebroadcast_lane(),
            ..WakeSchedules::UNCHANGED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::announce::{Announce, AnnounceId};
    use crate::routing::storage::FixedInline;
    use crate::wire::{DestinationType, PacketType, PropagationType, WirePacketHeader, MTU};

    #[test]
    fn tick_advances_count_deterministically() {
        let mut left: EngineState<Cap> = EngineState::<Cap>::default();
        let mut right: EngineState<Cap> = EngineState::<Cap>::default();

        let (left_out, left_bytes) = tick_capture(&mut left, InstantMillis(1_000), &[]);
        let (right_out, right_bytes) = tick_capture(&mut right, InstantMillis(1_000), &[]);

        assert_eq!(observable_state(&left), observable_state(&right));
        assert_eq!(left.tick_count(), 1);
        assert_eq!(left_out, right_out);
        assert_eq!(left_out.egress_directive_count, 0);
        assert!(left_bytes.is_empty());
        assert_eq!(left_bytes, right_bytes);
    }

    #[test]
    fn tick_retries_a_held_entry_and_discards_it_when_the_arena_is_still_full() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state =
            EngineState::<FixedInline<4, 64, 8, 4, 512, 64, 8, 8, 128, 8, 8, 8, 8, 16>>::default();
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.held_announce_count(), 1);

        let (out, _bytes) = tick_capture(&mut state, InstantMillis(2_000), &[]);
        assert_eq!(out.recovered_from_held_count, 0);
        assert_eq!(state.held_announce_count(), 0);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn tick_drains_the_entire_held_cache_in_one_pass() {
        use crate::engine::egress::write_announce_wire_packet;
        use crate::routing::announce::expand_name;

        let mut state =
            EngineState::<FixedInline<4, 64, 8, 4, 512, 64, 8, 8, 128, 8, 8, 8, 8, 16>>::default();

        let key = fixed_secret_key();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&key);
        let announce2 = Announce::build_signed(
            &identity,
            expand_name("personal", &["other"]).unwrap(),
            AnnounceId::from_wire([0x55; 10]),
            None,
            b"hello-personal",
        )
        .unwrap();
        let mut buf2 = [0u8; MTU];
        let n2 = write_announce_wire_packet(&announce2, 0, &mut buf2).unwrap();

        let mut raw1 = hx(RAW_ANNOUNCE);
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw1,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_001),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut buf2[..n2],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            state.held_announce_count(),
            2,
            "both distinct destinations parked under arena pressure"
        );

        let (out, _bytes) = tick_capture(&mut state, InstantMillis(2_000), &[]);
        assert_eq!(
            state.held_announce_count(),
            0,
            "one tick drains the entire held cache, not just one entry"
        );
        assert_eq!(
            out.recovered_from_held_count, 0,
            "arena still full → both discard"
        );
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
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        let (tick_out, emitted) = tick_capture(
            &mut state,
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            &view,
        );
        assert_eq!(tick_out.egress_directive_count, 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);

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
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

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
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        let tick_out = state.tick(
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            TEST_ENTROPY,
            view,
        );
        tick_out
            .egress_directives(view)
            .map(|directive| directive.target())
            .collect()
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
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);
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
    fn pending_rebroadcasts_are_not_emitted_before_their_due_time() {
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
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        let view = [routable_descriptor(InterfaceId::new([0xFE; 16]))];
        let (tick_out, emitted) = tick_capture(&mut state, InstantMillis(arrival.0 - 1), &view);
        assert_eq!(tick_out.egress_directive_count, 0);
        assert!(emitted.is_empty());
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);
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
    fn held_retry_that_fails_does_not_schedule_a_rebroadcast() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state =
            EngineState::<FixedInline<4, 64, 8, 4, 16, 4, 8, 8, 128, 8, 8, 8, 8, 16>>::default();
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.held_announce_count(), 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);

        let (tick_out, bytes) = tick_capture(&mut state, InstantMillis(2_000), &[]);
        assert_eq!(tick_out.recovered_from_held_count, 0);
        assert_eq!(tick_out.egress_directive_count, 0);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);
        assert!(bytes.is_empty());
    }

    #[test]
    fn fire_due_announce_rebroadcasts_emits_the_directive_then_clears_the_entry() {
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
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        let mut sent: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)> = std::vec::Vec::new();
        let delta = state.fire_due_announce_rebroadcasts(
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            &view,
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce {
                    target, bytes, ..
                }) = reaction
                {
                    sent.push((target, bytes.to_vec()));
                }
            },
        );

        assert_eq!(
            state.pending_announce_rebroadcast_count(),
            0,
            "firing clears the due entry",
        );
        assert_eq!(
            delta.rebroadcast_announces,
            LaneWake::Idle,
            "the only rebroadcast fired, so the lane delta reports it clear",
        );
        assert_eq!(
            sent.len(),
            1,
            "one rebroadcast directive for the lone interface"
        );
        assert_eq!(
            sent[0].0, target,
            "the rebroadcast names the firable interface"
        );
        let (header, _) = WirePacketHeader::parse(&sent[0].1).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        let original = WirePacketHeader::parse(&hx(RAW_ANNOUNCE)).unwrap().0;
        assert_eq!(
            header.hops,
            original.hops + 1,
            "the rebroadcast bumps the hop count",
        );
    }

    #[test]
    fn recover_held_announces_is_silent_when_nothing_recovers() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state =
            EngineState::<FixedInline<4, 64, 8, 4, 16, 4, 8, 8, 128, 8, 8, 8, 8, 16>>::default();
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.held_announce_count(), 1);

        let mut journaled = 0usize;
        let delta =
            state.recover_held_announces(TEST_ENTROPY, &transporting_view(), &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::AnnounceHeard { .. }) = reaction {
                    journaled += 1;
                }
            });

        assert_eq!(
            journaled, 0,
            "a hold that fails to recover journals nothing"
        );
        assert_eq!(
            delta.held_announces,
            LaneWake::Idle,
            "draining the cache leaves the held lane idle",
        );
        assert_eq!(
            delta.rebroadcast_announces,
            LaneWake::Idle,
            "nothing recovered, so nothing was scheduled to rebroadcast",
        );
        assert_eq!(
            state.held_announce_count(),
            0,
            "the retry still drains the held cache",
        );
    }
}
