use crate::engine::directives::{EngineDirective, EngineDirectives as _};
use crate::engine::{EgressDirective, EngineState, InstantMillis};
use crate::interfaces::{InterfaceDescriptor, InterfaceId, MAX_REGISTERED_INTERFACES};
use crate::routing::announce::defaults::{
    jitter_offset_for, JitterSeed, DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
};
use crate::routing::announce::held_cache::HeldAnnounces as _;
use crate::routing::announce::schedule::RebroadcastQueue as _;
use crate::routing::announce::{AnnounceAcceptanceDecision, AnnounceAcceptanceInput};
use crate::routing::storage::EngineStorage;
use crate::routing::UpsertRouteOutcome;
use crate::wire::DestinationType;
use heapless::Vec as HeaplessVec;

#[must_use]
pub struct TickOutput<'a, S: EngineStorage> {
    state: &'a mut EngineState<S>,
    now: InstantMillis,
    recovered_from_held_count: usize,
}

impl<'a, S: EngineStorage> TickOutput<'a, S> {
    pub fn egress_directive_count(&self) -> usize {
        self.state.directives.len()
    }

    pub const fn recovered_from_held_count(&self) -> usize {
        self.recovered_from_held_count
    }

    pub fn egress_directives(&self) -> impl Iterator<Item = EgressDirective<'_>> + '_ {
        let state = &*self.state;
        state.directives.iter().filter_map(move |directive| {
            let EngineDirective::ReemitAnnounce {
                destination,
                fire_on,
            } = directive;
            let via = state.transport_id?;
            let retained = state.routing_table.retained_announce_for(destination)?;
            Some(EgressDirective::ReemitAnnounce {
                announce: retained.announce,
                emit_hops: retained.hops,
                via,
                fire_on: fire_on.as_slice(),
            })
        })
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
        interfaces: &[InterfaceDescriptor],
    ) -> TickOutput<'_, S> {
        self.tick_count = self.tick_count.saturating_add(1);

        let mut recovered_from_held_count = 0;
        while let Some(held) = self.held_announces_cache.take_next() {
            use crate::routing::announce::held_cache::HoldReason;
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
                            recovered_from_held_count += 1;
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
                                );
                            }
                        }
                    }
                }
            }
        }

        // Materialize this tick's directives from the due rebroadcasts. Indexed (not
        // iterated) so the read of `pending_rebroadcasts` doesn't overlap the write to
        // `directives` — both are fields of `self`.
        self.directives.clear();
        for index in 0..self.pending_rebroadcasts.as_slice().len() {
            let scheduled = self.pending_rebroadcasts.as_slice()[index];
            if scheduled.due_at > now {
                continue;
            }
            let mut fire_on: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES> =
                HeaplessVec::new();
            for descriptor in interfaces {
                let firable = if descriptor.id == scheduled.source_interface {
                    descriptor.capabilities.allows_same_interface_repeat()
                } else {
                    descriptor.capabilities.allows_transport()
                };
                if firable {
                    let _ = fire_on.push(descriptor.id);
                }
            }
            if fire_on.is_empty() {
                continue;
            }
            self.directives.push(EngineDirective::ReemitAnnounce {
                destination: scheduled.destination,
                fire_on,
            });
        }

        TickOutput {
            state: self,
            now,
            recovered_from_held_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::interfaces::InboundPacket;
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
        let mut state = EngineState::<
            FixedInline<4, 64, 8, 4, 512, 64, 8, 8, 8, 128, 8, 8, 8, 8, 16>,
        >::default();
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

        let mut state = EngineState::<
            FixedInline<4, 64, 8, 4, 512, 64, 8, 8, 8, 128, 8, 8, 8, 8, 16>,
        >::default();

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
        let mut heard = hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE);
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
        view: &[InterfaceDescriptor],
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
            .egress_directives()
            .flat_map(|directive| {
                let EgressDirective::ReemitAnnounce { fire_on, .. } = directive;
                fire_on.to_vec()
            })
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
            EngineState::<FixedInline<4, 64, 8, 4, 16, 4, 8, 8, 8, 128, 8, 8, 8, 8, 16>>::default();
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
}
