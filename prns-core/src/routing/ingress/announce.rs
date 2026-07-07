use super::*;
use crate::routing::warmth::WarmestOf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceIngest {
    Accepted(AcceptedAnnounce),
    Ignored,
    Held,
    HeldDropped {
        destination: DestinationHash,
        cause: HeldDropCause,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedAnnounce {
    pub destination: DestinationHash,
    pub hops: u8,
    pub rebroadcast: RebroadcastDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebroadcastDecision {
    Scheduled,
    NotATransportNode,
    NoTransportInterfaces,
    TerminalPathResponse,
    RateBlocked,
}

/// Owns a copy of the payload because the arrival slot is recycled before the crypto pool finishes the verify, so borrowing it would dangle.
/// Only the host reactor defers verification, so this copy is never built on the embedded inbound stack, which verifies inline.
pub struct AnnounceVerifyOwed {
    pub payload: HeaplessVec<u8, BROADCAST_MTU>,
    pub header: WirePacketHeader,
    pub received_hops: u8,
    pub source_interface: InterfaceId,
    pub arrived_at: InstantMillis,
    pub next_hop: NextHop,
    pub is_path_response: bool,
}

impl<S: StorageLayout> EngineState<S> {
    fn destination_announce_limit_blocks_rebroadcast(
        &mut self,
        source_interface: InterfaceId,
        destination: DestinationHash,
        now: InstantMillis,
        interfaces: &[InterfaceDescriptor],
    ) -> bool {
        let Some(limit) = interfaces
            .iter()
            .find(|descriptor| descriptor.id == source_interface)
            .and_then(|descriptor| descriptor.announce_rate_limit)
        else {
            return false;
        };
        self.destination_announce_limits
            .observe(destination, now, limit)
            == DestinationAnnounceVerdict::Blocked
    }

    fn schedule_rebroadcast(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        interfaces: &[InterfaceDescriptor],
        fill_entropy: &mut impl FnMut(&mut [u8]),
    ) -> RebroadcastDecision {
        let destination = arrival.announce.destination;
        let awaiting_requester = self.recursive_path_requests.take_requester(&destination);
        if arrival.is_path_response {
            let Some(requesting_interface) = awaiting_requester else {
                return RebroadcastDecision::TerminalPathResponse;
            };
            self.scheduled_announces.schedule_directed(
                destination,
                arrival.arrived_at,
                requesting_interface,
                arrival.hops,
            );
            return RebroadcastDecision::Scheduled;
        }
        if self.transport_id.is_none() {
            return RebroadcastDecision::NotATransportNode;
        }
        if !interfaces
            .iter()
            .any(|descriptor| descriptor.capabilities.allows_transport())
        {
            return RebroadcastDecision::NoTransportInterfaces;
        }
        if self.destination_announce_limit_blocks_rebroadcast(
            arrival.receiving_interface,
            destination,
            arrival.arrived_at,
            interfaces,
        ) {
            return RebroadcastDecision::RateBlocked;
        }
        let offset = jitter_offset(fill_entropy, DEFAULT_REBROADCAST_JITTER_WINDOW_MS);
        self.scheduled_announces.schedule(
            destination,
            InstantMillis(arrival.arrived_at.0.saturating_add(offset)),
            arrival.receiving_interface,
            arrival.hops,
        );
        RebroadcastDecision::Scheduled
    }

    pub(crate) fn ingest_announce(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        fill_entropy: &mut impl FnMut(&mut [u8]),
        interfaces: &[InterfaceDescriptor],
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> AnnounceIngest {
        let &AnnounceArrival {
            ref announce,
            hops: received_hops,
            arrived_at,
            receiving_interface: source_interface,
            ..
        } = arrival;
        if self.transport_id.is_some() {
            self.scheduled_announces.absorb_echo(
                &announce.destination,
                received_hops,
                arrived_at,
                MAX_PEER_EMISSIONS,
            );
        }

        let decision = determine_acceptance(AnnounceAcceptanceInput {
            packet_hops: received_hops,
            announce_id: announce.announce_id,
            destination_is_self_or_upstream: self
                .upstream_app_destinations
                .lookup(&announce.destination, DestinationType::Single)
                .is_some(),
            existing_route: self
                .routing_table
                .existing_route_for(&announce.destination, interfaces),
            arrived_at,
        });

        if !matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
            return AnnounceIngest::Ignored;
        }

        let previous_interface = self
            .routing_table
            .path_row(&announce.destination)
            .map(|entry| entry.receiving_interface);
        let warmth = WarmestOf(&self.tunnels, &self.departed_interfaces);
        let dirty = &mut self.dirty_interfaces;
        let outcome = self.routing_table.upsert_route_with_warmth(
            arrival,
            interfaces,
            &warmth,
            &mut |removed| {
                dirty.mark(removed.receiving_interface);
                on_removed(removed);
            },
        );
        match outcome {
            UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated => {
                self.mark_interface_dirty(source_interface);
                if let Some(previous) = previous_interface {
                    self.mark_interface_dirty(previous);
                }

                let rebroadcast = self.schedule_rebroadcast(arrival, interfaces, fill_entropy);
                AnnounceIngest::Accepted(AcceptedAnnounce {
                    destination: announce.destination,
                    hops: received_hops,
                    rebroadcast,
                })
            }
            UpsertRouteOutcome::Dropped(
                DropCause::PayloadArenaFull | DropCause::RoutingTableFull,
            ) => AnnounceIngest::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::routing::ingress::testkit::iface;
    use crate::storage::TestFixedStorage;
    use crate::wire::HEADER_MIN_LEN;

    #[test]
    fn a_path_response_is_learned_as_a_route_but_never_rebroadcast() {
        let mut relay = transporting_node();
        let mut response = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        response[HEADER_MIN_LEN - 1] = WireContext::PathResponse.to_byte();

        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xA1),
                    bytes: &mut response,
                },
                &mut |_| {},
                &transporting_interfaces(),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                        .try_into()
                        .unwrap()
                ),
                hops: 1,
                rebroadcast: RebroadcastDecision::TerminalPathResponse,
            })),
        );
        assert_eq!(relay.route_count(), 1, "the path response is learned");
        assert_eq!(
            relay.scheduled_announce_count(),
            0,
            "a path response is never re-flooded",
        );
    }

    #[test]
    fn the_same_announce_without_the_path_response_tag_is_scheduled() {
        let mut relay = transporting_node();
        let mut announce = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        assert!(matches!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xA1),
                    bytes: &mut announce,
                },
                &mut |_| {},
                &transporting_interfaces(),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        assert_eq!(relay.scheduled_announce_count(), 1);
    }

    #[test]
    fn a_destination_announcing_faster_than_the_interface_target_is_rate_blocked() {
        use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
        use crate::interfaces::AnnounceRateLimit;

        let mut announcer = personal_node_announcer();
        let destination = personal_node_destination();
        let command = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };
        let mut buf_a = [0u8; BROADCAST_MTU];
        let first_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(1_000),
                &mut |bytes: &mut [u8]| bytes.fill(0x11),
                &mut buf_a,
            )
            .written_len();
        let mut first = buf_a[..first_len].to_vec();
        let mut buf_b = [0u8; BROADCAST_MTU];
        let second_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(2_000),
                &mut |bytes: &mut [u8]| bytes.fill(0x22),
                &mut buf_b,
            )
            .written_len();
        let mut second = buf_b[..second_len].to_vec();

        let source = iface(0xB2);
        let rate_limited = [InterfaceDescriptor {
            announce_rate_limit: Some(AnnounceRateLimit {
                target_ms: 10_000,
                grace: 0,
                penalty_ms: 60_000,
            }),
            ..routable_descriptor(source)
        }];

        let mut relay = transporting_node();
        assert!(matches!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(10_000),
                    source_interface: source,
                    bytes: &mut first,
                },
                &mut |_| {},
                &rate_limited,
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        assert!(matches!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(11_000),
                    source_interface: source,
                    bytes: &mut second,
                },
                &mut |_| {},
                &rate_limited,
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::RateBlocked,
                ..
            })),
        ));
        assert_eq!(relay.route_count(), 1, "the route is still learned");
        assert_eq!(
            relay.scheduled_announce_count(),
            1,
            "only the first announce was scheduled to rebroadcast",
        );
    }

    #[test]
    fn a_destination_within_the_interface_target_is_not_rate_blocked() {
        use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
        use crate::interfaces::AnnounceRateLimit;

        let mut announcer = personal_node_announcer();
        let destination = personal_node_destination();
        let command = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };
        let mut buf_a = [0u8; BROADCAST_MTU];
        let first_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(1_000),
                &mut |bytes: &mut [u8]| bytes.fill(0x11),
                &mut buf_a,
            )
            .written_len();
        let mut first = buf_a[..first_len].to_vec();
        let mut buf_b = [0u8; BROADCAST_MTU];
        let second_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(2_000),
                &mut |bytes: &mut [u8]| bytes.fill(0x22),
                &mut buf_b,
            )
            .written_len();
        let mut second = buf_b[..second_len].to_vec();

        let source = iface(0xB2);
        let rate_limited = [InterfaceDescriptor {
            announce_rate_limit: Some(AnnounceRateLimit {
                target_ms: 10_000,
                grace: 0,
                penalty_ms: 60_000,
            }),
            ..routable_descriptor(source)
        }];

        let mut relay = transporting_node();
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(10_000),
                source_interface: source,
                bytes: &mut first,
            },
            &mut |_| {},
            &rate_limited,
            &mut |_| {},
            None,
        );
        assert!(matches!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(25_000),
                    source_interface: source,
                    bytes: &mut second,
                },
                &mut |_| {},
                &rate_limited,
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        assert_eq!(
            relay.scheduled_announce_count(),
            1,
            "one pending per destination — the second schedule replaces the first",
        );
    }

    #[test]
    fn an_echo_of_our_own_announce_takes_no_route() {
        let mut state = personal_node_announcer();
        let mut announce_buf = [0u8; BROADCAST_MTU];
        let announce_len = state
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(100),
                &mut test_fill_entropy,
                &mut announce_buf,
            )
            .written_len();

        let mut relayed = announce_buf[..announce_len].to_vec();
        relayed[1] = 1;
        assert_eq!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0xA1; 8]),
                    bytes: &mut relayed,
                },
                &mut |_| {},
                &transporting_interfaces(),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored),
            "a transport echoing our announce back must not become a route to ourselves",
        );
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn a_node_without_transport_interfaces_learns_the_route_but_owes_no_rebroadcast() {
        use crate::interfaces::{EgressCapability, TransportCapability};

        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state = transporting_node();
        let mut leaf = routable_descriptor(InterfaceId::new([0xEE; 8]));
        leaf.capabilities.egress = EgressCapability::Enabled(TransportCapability::NoTransport);

        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            &[leaf],
            &mut |_| {},
            None,
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                        .try_into()
                        .unwrap(),
                ),
                hops: 1,
                rebroadcast: RebroadcastDecision::NoTransportInterfaces,
            })),
        );
        assert_eq!(state.route_count(), 1);
        assert_eq!(state.scheduled_announce_count(), 0);
    }

    #[test]
    fn ingest_accepts_a_real_announce_then_rejects_its_replay() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state = transporting_node();

        let first = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );
        assert_eq!(first, rns_1_3_5_announce_accepted(1));
        assert_eq!(state.route_count(), 1);

        let second = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );
        assert_eq!(
            second,
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored)
        );
        assert_eq!(state.route_count(), 1);
    }

    #[test]
    fn deferred_announce_verify_matches_inline_accept_and_gates_forgeries() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state = transporting_node();
        let mut deferred = DeferredCrypto::default();
        let outcome = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            Some(&mut deferred),
        );
        assert_eq!(outcome, IngestPacketOutcome::OwesAnnounceVerify);
        assert_eq!(
            state.route_count(),
            0,
            "no route is learned before the verify resumes"
        );

        let DeferredCrypto::AnnounceVerify(owed) = deferred else {
            panic!("the obligation is captured for the pool");
        };
        let announce = Announce::from_wire_unverified(&owed.header, &owed.payload)
            .expect("the captured bytes re-parse");
        assert!(announce.signature_is_valid(), "the real announce verifies");

        let mut forged = owed.payload.to_vec();
        let pos = forged
            .windows(64)
            .position(|w| w == &announce.signature.0[..])
            .expect("the signature sits verbatim in the payload");
        forged[pos] ^= 0x01;
        let forged_announce = Announce::from_wire_unverified(&owed.header, &forged)
            .expect("a forged-signature announce still parses");
        assert!(
            !forged_announce.signature_is_valid(),
            "the forgery is rejected by the verify"
        );

        state.resume_announce(owed, &transporting_interfaces(), &mut |_| {}, &mut |_| {});
        assert_eq!(
            state.route_count(),
            1,
            "the resumed announce learns the route, matching the inline accept"
        );
    }

    #[test]
    fn received_hops_are_incremented_so_the_reach_boundary_matches_pathfinder_m() {
        let mut at_limit = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        at_limit[1] = 127;
        let mut state = transporting_node();
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut at_limit,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );
        assert_eq!(out, rns_1_3_5_announce_accepted(128));

        let mut beyond = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        beyond[1] = 128;
        let mut state = transporting_node();
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut beyond,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );
        assert_eq!(out, IngestPacketOutcome::Announce(AnnounceIngest::Ignored));
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn an_accepted_announce_is_retained_for_faithful_rebroadcast() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let pristine = raw.clone();
        let (header, payload) = WirePacketHeader::parse(&pristine).unwrap();
        let destination =
            DestinationHash::from_slice(&pristine[2..18]).expect("16-byte destination hash");

        let mut state = transporting_node();
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );
        assert_eq!(out, rns_1_3_5_announce_accepted(1));

        let stored = state
            .routing_table
            .stored_announce_for(&destination)
            .expect("the accepted announce is on hand");
        assert_eq!(stored.hops, header.hops + 1);
        let mut buf = [0u8; 500];
        let n = stored.announce.to_wire(&mut buf).unwrap();
        assert_eq!(&buf[..n], payload);
    }

    #[test]
    fn a_node_without_a_transport_id_learns_the_route_but_owes_no_rebroadcast() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();

        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                        .try_into()
                        .unwrap(),
                ),
                hops: 1,
                rebroadcast: RebroadcastDecision::NotATransportNode,
            })),
        );
        assert_eq!(state.route_count(), 1);
        assert_eq!(state.scheduled_announce_count(), 0);
    }

    #[test]
    fn a_relayed_announce_routes_via_its_transport_node_and_a_direct_one_routes_direct() {
        use crate::routing::NextHop;
        use crate::wire::PropagationType;

        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let destination = header.address;
        let relay = TransportId::new([0xBB; 16]);

        let relayed_header = WirePacketHeader {
            transport_id: Some(relay),
            propagation: PropagationType::Transport,
            hops: 1,
            ..header
        };
        let mut relayed = [0u8; BROADCAST_MTU];
        let header_len = relayed_header.write(&mut relayed).unwrap();
        relayed[header_len..header_len + payload.len()].copy_from_slice(payload);

        let mut state = transporting_node();
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut relayed[..header_len + payload.len()],
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );
        assert_eq!(out, rns_1_3_5_announce_accepted(2));
        assert_eq!(
            state
                .routing_table
                .stored_announce_for(&DestinationHash::from_address(destination))
                .unwrap()
                .next_hop,
            NextHop::Via(relay),
            "a relayed announce's next hop is the transport node that stamped it",
        );

        let mut direct = raw.clone();
        let mut fresh = transporting_node();
        let _ = fresh.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut direct,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );
        assert_eq!(
            fresh
                .routing_table
                .stored_announce_for(&DestinationHash::from_address(destination))
                .unwrap()
                .next_hop,
            NextHop::Direct,
            "an unrelayed announce is reachable directly",
        );
    }

    #[test]
    fn an_announce_whose_app_data_can_never_fit_is_ignored() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state =
            EngineState::<TestFixedStorage<4, 64, 8, 8, 8, 128, 8, 8, 8, 8, 16, 16>>::default();

        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored),
            "an app_data larger than the whole arena has no eviction that can admit it",
        );
        assert_eq!(state.route_count(), 0);
    }

    fn flood_announce(seed: u8, hops: u8) -> std::vec::Vec<u8> {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[seed.wrapping_add(1); 64]);
        let app = [seed; 4];
        let announce = Announce::build_signed(
            &signer,
            crate::routing::announce::DottedNameHash::new([0u8; 10]),
            crate::routing::announce::AnnounceId::from_wire([seed; 10]),
            None,
            &app,
        )
        .expect("a built announce");
        let mut buf = [0u8; BROADCAST_MTU];
        let n = crate::engine::write_announce_wire_packet(&announce, hops, &mut buf)
            .expect("announce serializes");
        buf[..n].to_vec()
    }

    #[test]
    fn a_flood_of_unknown_announces_is_held_then_drip_released_lowest_hop_first() {
        use crate::engine::{EngineReaction, Journaled};

        let source = InterfaceId::new([0xEE; 8]);
        let interfaces = transporting_interfaces();
        let mut relay = transporting_node();

        let mut accepted = 0usize;
        let mut held = 0usize;
        for i in 0..8u8 {
            let mut wire = flood_announce(i, 10 - i);
            match relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000 + u64::from(i) * 5),
                    source_interface: source,
                    bytes: &mut wire,
                },
                &mut |_| {},
                &interfaces,
                &mut |_| {},
                None,
            ) {
                IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)) => accepted += 1,
                IngestPacketOutcome::Announce(AnnounceIngest::Held) => held += 1,
                other => panic!("a valid announce is accepted or held, got {other:?}"),
            }
        }
        assert!(
            accepted >= 1,
            "the announces under the burst threshold are processed normally",
        );
        assert!(
            held >= 1,
            "the flood past the threshold is parked, not processed"
        );
        assert_eq!(relay.held_announces.len(), held);
        assert_eq!(
            relay.route_count(),
            accepted,
            "a held announce has not become a route yet",
        );

        let mut released_hops = std::vec::Vec::new();
        for step in 0..(held as u64 + 4) {
            if relay.held_announces.is_empty() {
                break;
            }
            let now = InstantMillis(1_000 + 15_000 + step * 5_000);
            relay.fire_due_held_announces(
                now,
                &interfaces,
                &mut |bytes: &mut [u8]| bytes.fill(0xE7),
                &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::AnnounceHeard { hops, .. }) =
                        reaction
                    {
                        released_hops.push(hops);
                    }
                },
            );
        }

        assert!(
            relay.held_announces.is_empty(),
            "once the burst subsides every held announce drips out",
        );
        assert_eq!(released_hops.len(), held, "each is released exactly once");
        assert_eq!(
            relay.route_count(),
            accepted + held,
            "and each held announce becomes a route on release",
        );
        let mut ascending = released_hops.clone();
        ascending.sort_unstable();
        assert_eq!(
            released_hops, ascending,
            "they drip lowest-hop first, not in arrival order",
        );
    }

    #[test]
    fn a_high_hop_flood_can_never_fill_the_held_queue() {
        let source = InterfaceId::new([0xEE; 8]);
        let interfaces = transporting_interfaces();
        let mut relay = transporting_node();

        // Wire hops 200 -> received_hops 201, past MAX_HOP_COUNT. A stream this size
        // trips the interface's burst limiter, so later announces reach the hold path;
        // none may be parked, since an announce past the hop maximum can never route.
        for i in 0..16u8 {
            let mut wire = flood_announce(i, 200);
            let out = relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000 + u64::from(i) * 5),
                    source_interface: source,
                    bytes: &mut wire,
                },
                &mut |_| {},
                &interfaces,
                &mut |_| {},
                None,
            );
            assert!(
                !matches!(out, IngestPacketOutcome::Announce(AnnounceIngest::Held)),
                "an announce past the hop maximum is never held",
            );
        }
        assert!(
            relay.held_announces.is_empty(),
            "a doomed announce can never consume a hold slot",
        );
        assert_eq!(relay.route_count(), 0);
    }

    #[test]
    fn a_signature_forgery_spray_does_not_inflate_the_ingress_limiter() {
        let source = InterfaceId::new([0xEE; 8]);
        let interfaces = transporting_interfaces();
        let mut relay = transporting_node();

        for i in 0..32u8 {
            let mut wire = flood_announce(i, 5);
            *wire.last_mut().unwrap() ^= 0xFF;
            let out = relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000 + u64::from(i) * 2),
                    source_interface: source,
                    bytes: &mut wire,
                },
                &mut |_| {},
                &interfaces,
                &mut |_| {},
                None,
            );
            assert_eq!(
                out,
                IngestPacketOutcome::Ignored,
                "a forged-signature announce is dropped",
            );
        }
        assert!(relay.held_announces.is_empty());

        let mut real = flood_announce(200, 5);
        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: source,
                bytes: &mut real,
            },
            &mut |_| {},
            &interfaces,
            &mut |_| {},
            None,
        );
        assert!(
            matches!(out, IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_))),
            "a real announce after a forgery spray is processed, not held: garbage never reached the limiter",
        );
    }
}
