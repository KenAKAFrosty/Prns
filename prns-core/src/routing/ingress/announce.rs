//! Announce ingress: acceptance, route learning, and the rebroadcast decision.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceIngest {
    Accepted(AcceptedAnnounce),
    Ignored,
    /// The interface is bursting and this announce was for an unknown destination,
    /// so it was parked in the held queue to be drip-released once the burst subsides
    /// — RNS `Interface.hold_announce` (Interfaces/Interface.py:228).
    Held,
}

/// The route an accepted announce just took — what an app needs to discover
/// the peer behind it.
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
    /// A path response is learned but never re-flooded — the answer is for the
    /// requester, not the network (RNS Transport.py:1884).
    TerminalPathResponse,
    /// The route is learned, but the destination is announcing faster than the
    /// receiving interface's rate target allows, so its rebroadcast is suppressed
    /// for a penalty window (RNS Transport.py:1835-1887).
    RateBlocked,
}

/// The whole obligation a deferred announce verify carries off the reactor: the
/// owned wire bytes and header to re-parse, plus the fields its `ingest_announce`
/// resume needs. The pool re-parses ([`Announce::from_wire_unverified`]) and runs
/// the Ed25519 verify; a valid verdict resumes the route ingest. Owns the payload
/// (the lane slot is released before the pool returns), so it is built only on the
/// reactor path, never on the embedded inbound stack.
pub struct AnnounceVerifyOwed {
    pub payload: HeaplessVec<u8, BROADCAST_MTU>,
    pub header: WirePacketHeader,
    pub received_hops: u8,
    pub source_interface: InterfaceId,
    pub arrived_at: InstantMillis,
    pub next_hop: NextHop,
    pub is_path_response: bool,
    pub jitter: JitterSeed,
}

impl<S: StorageLayout> EngineState<S> {
    /// Off (false) when the interface sets no target, which is the reference default (RNS Transport.py:1836).
    fn announce_rate_blocks_rebroadcast(
        &mut self,
        source_interface: InterfaceId,
        destination: DestinationHash,
        now: InstantMillis,
        interfaces: &[InterfaceConfig],
    ) -> bool {
        let Some(limit) = interfaces
            .iter()
            .find(|descriptor| descriptor.id == source_interface)
            .and_then(|descriptor| descriptor.announce_rate_limit)
        else {
            return false;
        };
        self.announce_rates.observe(destination, now, limit) == AnnounceRateVerdict::Blocked
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ingest_announce(
        &mut self,
        announce: Announce<'_>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        next_hop: NextHop,
        is_path_response: bool,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> AnnounceIngest {
        if self.transport_id.is_some() {
            self.scheduled_announces.absorb_echo(
                &announce.destination,
                received_hops,
                arrived_at,
                MAX_ANNOUNCE_REBROADCASTS,
            );
        }

        let decision = AnnounceAcceptanceInput {
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
        }
        .determine_acceptance();

        if !matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
            return AnnounceIngest::Ignored;
        }

        let previous_interface = self
            .routing_table
            .path_row(&announce.destination)
            .map(|entry| entry.receiving_interface);
        let dirty = &mut self.dirty_interfaces;
        let outcome = self.routing_table.upsert_route_with_tunnels(
            received_hops,
            arrived_at,
            source_interface,
            interfaces,
            &self.tunnels,
            next_hop,
            &announce,
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
                // An announce that answers a discovery we forwarded on a stranger's
                // behalf is steered straight back to the interface that asked. A path
                // response is otherwise terminal at us, so without this the answer the
                // stranger is waiting for would never reach them.
                let discovery_answer = self.discovery_path_requests.take(&announce.destination);
                let rebroadcast = if is_path_response {
                    if let Some(requesting_interface) = discovery_answer {
                        self.scheduled_announces.schedule_directed(
                            announce.destination,
                            arrived_at,
                            requesting_interface,
                            received_hops,
                        );
                        RebroadcastDecision::Scheduled
                    } else {
                        RebroadcastDecision::TerminalPathResponse
                    }
                } else if self.transport_id.is_none() {
                    RebroadcastDecision::NotATransportNode
                } else if !interfaces
                    .iter()
                    .any(|descriptor| descriptor.capabilities.allows_transport())
                {
                    RebroadcastDecision::NoTransportInterfaces
                } else if self.announce_rate_blocks_rebroadcast(
                    source_interface,
                    announce.destination,
                    arrived_at,
                    interfaces,
                ) {
                    RebroadcastDecision::RateBlocked
                } else {
                    let offset = jitter_offset_for(
                        jitter,
                        &announce.destination,
                        DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                    );
                    self.scheduled_announces.schedule(
                        announce.destination,
                        InstantMillis(arrived_at.0.saturating_add(offset)),
                        source_interface,
                        received_hops,
                    );
                    RebroadcastDecision::Scheduled
                };
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
        let mut response = hx(RAW_ANNOUNCE);
        // Tag the announce as a path response by flipping its context byte.
        response[HEADER_MIN_LEN - 1] = WireContext::PathResponse.to_byte();

        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xA1),
                    bytes: &mut response,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap()
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
        let mut announce = hx(RAW_ANNOUNCE);
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xA1),
                    bytes: &mut announce,
                },
                TEST_ENTROPY,
                &transporting_view(),
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
        use crate::routing::announce::AnnounceEntropy;

        // A peer mints two distinct announces for its own destination.
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
                AnnounceEntropy::new([0x11; AnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_a,
            )
            .written_len();
        let mut first = buf_a[..first_len].to_vec();
        let mut buf_b = [0u8; BROADCAST_MTU];
        let second_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(2_000),
                AnnounceEntropy::new([0x22; AnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_b,
            )
            .written_len();
        let mut second = buf_b[..second_len].to_vec();

        // The receiving interface caps a destination to one announce per 10s.
        let source = iface(0xB2);
        let rate_limited = [InterfaceConfig {
            announce_rate_limit: Some(AnnounceRateLimit {
                target_ms: 10_000,
                grace: 0,
                penalty_ms: 60_000,
            }),
            ..routable_descriptor(source)
        }];

        let mut relay = transporting_node();
        // First sighting: learned and scheduled to rebroadcast.
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(10_000),
                    source_interface: source,
                    bytes: &mut first,
                },
                TEST_ENTROPY,
                &rate_limited,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        // A second announce 1s later — far under the 10s target — is learned but
        // its rebroadcast is suppressed.
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(11_000),
                    source_interface: source,
                    bytes: &mut second,
                },
                TEST_ENTROPY,
                &rate_limited,
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
        use crate::routing::announce::AnnounceEntropy;

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
                AnnounceEntropy::new([0x11; AnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_a,
            )
            .written_len();
        let mut first = buf_a[..first_len].to_vec();
        let mut buf_b = [0u8; BROADCAST_MTU];
        let second_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(2_000),
                AnnounceEntropy::new([0x22; AnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_b,
            )
            .written_len();
        let mut second = buf_b[..second_len].to_vec();

        let source = iface(0xB2);
        let rate_limited = [InterfaceConfig {
            announce_rate_limit: Some(AnnounceRateLimit {
                target_ms: 10_000,
                grace: 0,
                penalty_ms: 60_000,
            }),
            ..routable_descriptor(source)
        }];

        let mut relay = transporting_node();
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(10_000),
                source_interface: source,
                bytes: &mut first,
            },
            TEST_ENTROPY,
            &rate_limited,
        );
        // A second announce a full target window later stays under the limit and
        // is scheduled like any other.
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(25_000),
                    source_interface: source,
                    bytes: &mut second,
                },
                TEST_ENTROPY,
                &rate_limited,
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
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();

        let mut relayed = announce_buf[..announce_len].to_vec();
        relayed[1] = 1;
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0xA1; 8]),
                    bytes: &mut relayed,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored),
            "a transport echoing our announce back must not become a route to ourselves",
        );
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn a_node_without_transport_interfaces_learns_the_route_but_owes_no_rebroadcast() {
        use crate::interfaces::{EgressCapability, TransportCapability};

        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let mut leaf = routable_descriptor(InterfaceId::new([0xEE; 8]));
        leaf.capabilities.egress = EgressCapability::Enabled(TransportCapability::NoTransport);

        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &[leaf],
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap(),
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
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();

        let first = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(first, raw_announce_accepted(1));
        assert_eq!(state.route_count(), 1);

        let second = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            second,
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored)
        );
        assert_eq!(state.route_count(), 1);
    }

    #[test]
    fn deferred_announce_verify_matches_inline_accept_and_gates_forgeries() {
        // The crypto-pool path: classify captures the obligation instead of
        // verifying; the pool verifies; a valid verdict resumes the route ingest.
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let mut owed = None;
        let outcome = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
            &mut |_| {},
            None,
            None,
            None,
            Some(&mut owed),
        );
        assert_eq!(outcome, IngestPacketOutcome::OwesAnnounceVerify);
        assert_eq!(
            state.route_count(),
            0,
            "no route is learned before the verify resumes"
        );

        let owed = owed.expect("the obligation is captured for the pool");
        let announce = Announce::from_wire_unverified(&owed.header, &owed.payload)
            .expect("the captured bytes re-parse");
        assert!(announce.signature_is_valid(), "the real announce verifies");

        // A forged signature still parses but fails the verify, so the reactor
        // never resumes it.
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

        // The valid verdict resumes into the same route ingest the inline path runs.
        state.resume_announce(owed, &transporting_view(), &mut |_| {});
        assert_eq!(
            state.route_count(),
            1,
            "the resumed announce learns the route, matching the inline accept"
        );
    }

    #[test]
    fn received_hops_are_incremented_so_the_reach_boundary_matches_pathfinder_m() {
        let mut at_limit = hx(RAW_ANNOUNCE);
        at_limit[1] = 127;
        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut at_limit,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(128));

        let mut beyond = hx(RAW_ANNOUNCE);
        beyond[1] = 128;
        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut beyond,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, IngestPacketOutcome::Announce(AnnounceIngest::Ignored));
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn an_accepted_announce_is_retained_for_faithful_rebroadcast() {
        let mut raw = hx(RAW_ANNOUNCE);
        let pristine = raw.clone();
        let (header, payload) = WirePacketHeader::parse(&pristine).unwrap();
        let destination =
            DestinationHash::from_slice(&pristine[2..18]).expect("16-byte destination hash");

        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(1));

        let retained = state
            .routing_table
            .retained_announce_for(&destination)
            .expect("the accepted announce is on hand");
        assert_eq!(retained.hops, header.hops + 1);
        let mut buf = [0u8; 500];
        let n = retained.announce.to_wire(&mut buf).unwrap();
        assert_eq!(&buf[..n], payload);
    }

    #[test]
    fn a_node_without_a_transport_id_learns_the_route_but_owes_no_rebroadcast() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();

        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap(),
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

        let raw = hx(RAW_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let destination = header.destination;
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
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut relayed[..header_len + payload.len()],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(2));
        assert_eq!(
            state
                .routing_table
                .retained_announce_for(&destination)
                .unwrap()
                .next_hop,
            NextHop::Via(relay),
            "a relayed announce's next hop is the transport node that stamped it",
        );

        let mut direct = raw.clone();
        let mut fresh = transporting_node();
        let _ = fresh.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut direct,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            fresh
                .routing_table
                .retained_announce_for(&destination)
                .unwrap()
                .next_hop,
            NextHop::Direct,
            "an unrelayed announce is reachable directly",
        );
    }

    #[test]
    fn an_announce_whose_app_data_can_never_fit_is_ignored() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = EngineState::<
            TestFixedStorage<4, 64, 8, 4, 512, 8, 8, 128, 8, 8, 8, 8, 16, 16>,
        >::default();

        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
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
        let n = crate::engine::egress::write_announce_wire_packet(&announce, hops, &mut buf)
            .expect("announce serializes");
        buf[..n].to_vec()
    }

    #[test]
    fn a_flood_of_unknown_announces_is_held_then_drip_released_lowest_hop_first() {
        use crate::engine::{EngineReaction, Journaled};

        let source = InterfaceId::new([0xEE; 8]);
        let view = transporting_view();
        let mut relay = transporting_node();

        let mut accepted = 0usize;
        let mut held = 0usize;
        for i in 0..8u8 {
            let mut wire = flood_announce(i, 10 - i);
            match relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000 + u64::from(i) * 5),
                    source_interface: source,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &view,
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
                &view,
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
}
