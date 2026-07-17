use super::*;
use crate::interfaces::{AttachedInterfaces, InterfaceCommonPolicy};

/// RNS 1.3.5 `Transport.path_request_handler`; only the transport form carries the requester's transport id.
struct PathRequest {
    destination: DestinationHash,
    requester_transport_id: Option<TransportId>,
    id: PathRequestIdBytes,
}

/// The reference's two non-answering cases (Transport.py:2866): one malformed, one well-formed policy.
#[derive(Debug, PartialEq, Eq)]
enum PathRequestError {
    NoDestination,
    NoId,
}

impl PathRequest {
    fn parse(payload: &[u8]) -> Result<Self, PathRequestError> {
        let destination = payload
            .get(..TRUNCATED_HASH_BYTE_LEN)
            .and_then(DestinationHash::from_slice)
            .ok_or(PathRequestError::NoDestination)?;
        let (requester_transport_id, id_region) = if payload.len() > TRUNCATED_HASH_BYTE_LEN * 2 {
            (
                TransportId::from_slice(
                    &payload[TRUNCATED_HASH_BYTE_LEN..TRUNCATED_HASH_BYTE_LEN * 2],
                ),
                &payload[TRUNCATED_HASH_BYTE_LEN * 2..],
            )
        } else if payload.len() > TRUNCATED_HASH_BYTE_LEN {
            (None, &payload[TRUNCATED_HASH_BYTE_LEN..])
        } else {
            return Err(PathRequestError::NoId);
        };
        let used = id_region.len().min(TRUNCATED_HASH_BYTE_LEN);
        let mut id = PathRequestIdBytes::default();
        id[..used].copy_from_slice(&id_region[..used]);
        Ok(Self {
            destination,
            requester_transport_id,
            id,
        })
    }

    /// RNS 1.3.5 `Transport.path_request`: the path loops if the requester is the very next hop we would answer with.
    fn loops_back_through_requester(&self, next_hop: NextHop) -> bool {
        matches!((next_hop, self.requester_transport_id), (NextHop::Via(via), Some(id)) if via == id)
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;
    use crate::interfaces::wire_limits::MAX_WIRE_FRAME_LEN;

    #[kani::proof]
    fn path_request_parse_never_panics_for_any_wire_payload() {
        let payload: [u8; MAX_WIRE_FRAME_LEN] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= payload.len());
        let _ = PathRequest::parse(&payload[..len]);
    }
}

fn path_response_grace_ms(
    source_interface: InterfaceId,
    interfaces: AttachedInterfaces<'_>,
) -> u64 {
    let roaming = interfaces
        .descriptor_for(source_interface)
        .is_some_and(|descriptor| descriptor.mode == InterfaceMode::Roaming);
    if roaming {
        PATH_REQUEST_GRACE_MS + PATH_REQUEST_ROAMING_GRACE_MS
    } else {
        PATH_REQUEST_GRACE_MS
    }
}

fn request_echoes_into_its_own_roaming_segment(
    route_learned_on: InterfaceId,
    source_interface: InterfaceId,
    interfaces: AttachedInterfaces<'_>,
) -> bool {
    route_learned_on == source_interface
        && interfaces
            .descriptor_for(source_interface)
            .is_some_and(|descriptor| descriptor.mode == InterfaceMode::Roaming)
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn ingest_path_request<'p>(
        &mut self,
        data: &DataPacket<'_>,
        source_interface: InterfaceId,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
    ) -> IngestPacketOutcome<'p> {
        let Ok(request) = PathRequest::parse(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };

        if self
            .seen_path_requests
            .observe(request.destination, request.id)
            == PathRequestNovelty::Duplicate
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate);
        }

        if self
            .upstream_app_destinations
            .lookup(&request.destination, DestinationType::Single)
            .is_some()
        {
            return IngestPacketOutcome::AnswerPathRequest {
                destination: request.destination,
            };
        }

        let from_local_client = source_interface.kind() == Some(InterfaceKind::LocalClient);
        let held_route = (self.network_transport_enabled() || from_local_client)
            .then(|| {
                self.routing_table
                    .forwarding_route_for(&request.destination)
            })
            .flatten();
        let Some(route) = held_route else {
            return self.forward_unrouted_path_request(
                &request,
                source_interface,
                from_local_client,
                now,
                interfaces,
            );
        };

        if request_echoes_into_its_own_roaming_segment(
            route.receiving_interface,
            source_interface,
            interfaces,
        ) {
            return IngestPacketOutcome::Ignored(IgnoreReason::LoopPrevented);
        }

        if request.loops_back_through_requester(route.next_hop) {
            return IngestPacketOutcome::Ignored(IgnoreReason::LoopPrevented);
        }

        if self.routing_table.responsiveness_of(&request.destination)
            == Some(RouteResponsiveness::Unresponsive)
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::RouteUnresponsive);
        }

        let due_at = if from_local_client {
            now
        } else {
            InstantMillis(
                now.0
                    .saturating_add(path_response_grace_ms(source_interface, interfaces)),
            )
        };
        self.scheduled_announces.schedule_directed(
            request.destination,
            due_at,
            source_interface,
            route.hops.0,
        );
        IngestPacketOutcome::ScheduledPathResponse {
            destination: request.destination,
        }
    }

    fn forward_unrouted_path_request<'p>(
        &mut self,
        request: &PathRequest,
        source_interface: InterfaceId,
        from_local_client: bool,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
    ) -> IngestPacketOutcome<'p> {
        let source_descriptor = interfaces.descriptor_for(source_interface);
        let forwards_recursively = self.network_transport_enabled()
            && source_descriptor.is_some_and(|descriptor| {
                descriptor.mode.recursively_forwards_unknown_paths()
                    || descriptor.common.forwarding.recursive_path_requests
            });
        if forwards_recursively
            && self
                .interface_path_request_limits
                .record_and_should_limit_with_policy(
                    source_interface,
                    now,
                    source_descriptor.map_or(
                        InterfaceCommonPolicy::RNS_DEFAULT.ingress_control,
                        |descriptor| descriptor.common.ingress_control,
                    ),
                )
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::RateLimited);
        }
        let has_local_client = interfaces
            .iter()
            .any(|descriptor| descriptor.id.kind() == Some(InterfaceKind::LocalClient));
        let outcome = if from_local_client || forwards_recursively {
            IngestPacketOutcome::ForwardRecursivePathRequest {
                destination: request.destination,
                id: request.id,
            }
        } else if has_local_client {
            IngestPacketOutcome::RelayPathRequestToLocalClients {
                destination: request.destination,
                id: request.id,
            }
        } else {
            return IngestPacketOutcome::Ignored(IgnoreReason::NotForUs);
        };
        let expires_at = InstantMillis(now.0.saturating_add(RECURSIVE_PATH_REQUEST_TIMEOUT_MS));
        match self
            .recursive_path_requests
            .begin(request.destination, source_interface, expires_at)
        {
            RecursiveOutcome::AlreadyInFlight => {
                IngestPacketOutcome::Ignored(IgnoreReason::Superseded)
            }
            RecursiveOutcome::Opened => outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{Directive, EngineReaction, IngestIo};
    use crate::interfaces::InterfaceDescriptor;
    use crate::routing::ingress::testkit::iface;
    use crate::wire::HEADER_MIN_LEN;

    #[test]
    fn path_request_parse_names_its_two_non_answering_cases() {
        let dest = [0x11; TRUNCATED_HASH_BYTE_LEN];
        let id = [0x55; TRUNCATED_HASH_BYTE_LEN];
        let tid = [0x7a; TRUNCATED_HASH_BYTE_LEN];

        assert_eq!(
            PathRequest::parse(&dest[..8]).err(),
            Some(PathRequestError::NoDestination),
        );
        assert_eq!(
            PathRequest::parse(&dest).err(),
            Some(PathRequestError::NoId)
        );

        let leaf = [&dest[..], &id[..]].concat();
        let parsed = PathRequest::parse(&leaf).unwrap();
        assert_eq!(parsed.requester_transport_id, None);
        assert_eq!(parsed.id, id);

        let transport = [&dest[..], &tid[..], &id[..]].concat();
        let parsed = PathRequest::parse(&transport).unwrap();
        assert_eq!(parsed.requester_transport_id, TransportId::from_slice(&tid));
        assert_eq!(parsed.id, id);
    }

    #[test]
    fn a_path_request_for_a_local_destination_owes_an_answer() {
        let mut state = personal_node_announcer();
        let local = personal_node_destination();

        let mut buf = [0u8; BROADCAST_MTU];
        let n = crate::engine::write_path_request_wire_packet(local, None, &[0x55; 16], &mut buf)
            .unwrap();
        let mut wire = buf[..n].to_vec();
        assert_eq!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::AnswerPathRequest { destination: local },
        );
    }

    #[test]
    fn a_leaf_ignores_a_path_request_for_a_stranger() {
        let mut leaf: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let mut buf = [0u8; BROADCAST_MTU];
        let n = crate::engine::write_path_request_wire_packet(
            DestinationHash::new([0x44; 16]),
            None,
            &[0x55; 16],
            &mut buf,
        )
        .unwrap();
        let mut wire = buf[..n].to_vec();
        assert_eq!(
            leaf.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    #[test]
    fn write_path_response_announce_emits_a_path_response_a_peer_learns_as_a_route() {
        use crate::engine::PathResponseWriteOutcome;
        use crate::routing::announce::Announce;

        let mut b = personal_node_announcer();
        let local = personal_node_destination();
        let mut buf = [0u8; BROADCAST_MTU];
        let PathResponseWriteOutcome::Written { wire_len, .. } = b
            .write_path_response_for_upstream(
                &local,
                InstantMillis(500),
                &mut test_fill_entropy,
                &mut buf,
            )
        else {
            panic!("a local destination is answerable");
        };

        let (header, payload) = WirePacketHeader::parse(&buf[..wire_len]).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.context, WireContext::PathResponse);
        assert_eq!(DestinationHash::from_address(header.address), local);
        assert_eq!(
            Announce::from_wire(&header, payload).unwrap().destination,
            local
        );

        let mut a: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let mut wire = buf[..wire_len].to_vec();
        assert!(matches!(
            a.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_200),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));
        assert_eq!(a.route_count(), 1);
    }

    #[test]
    fn a_path_response_for_a_destination_we_do_not_hold_is_refused() {
        use crate::engine::PathResponseWriteOutcome;
        let mut b = personal_node_announcer();
        let mut buf = [0u8; BROADCAST_MTU];
        assert!(matches!(
            b.write_path_response_for_upstream(
                &DestinationHash::new([0x44; 16]),
                InstantMillis(500),
                &mut test_fill_entropy,
                &mut buf,
            ),
            PathResponseWriteOutcome::NotUpstream,
        ));
    }

    fn relay_holding_a_cached_route() -> (EngineState<TestStorageLayout>, DestinationHash) {
        let cached = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );
        let mut relay = transporting_node();
        let mut announce = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        assert!(matches!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xB2),
                    bytes: &mut announce,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));
        (relay, cached)
    }

    fn discovering_descriptor(id: InterfaceId, mode: InterfaceMode) -> InterfaceDescriptor {
        InterfaceDescriptor {
            mode,
            ..routable_descriptor(id)
        }
    }

    fn stranger_path_request(id: [u8; 16]) -> std::vec::Vec<u8> {
        let mut buf = [0u8; BROADCAST_MTU];
        let n = crate::engine::write_path_request_wire_packet(
            DestinationHash::new([0x44; 16]),
            None,
            &id,
            &mut buf,
        )
        .unwrap();
        buf[..n].to_vec()
    }

    #[test]
    fn a_transport_node_on_a_gateway_interface_forwards_an_unknown_path_request() {
        let stranger = DestinationHash::new([0x44; 16]);
        let source = iface(0xA1);
        let mut relay = transporting_node();
        let interfaces = [discovering_descriptor(source, InterfaceMode::Gateway)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ForwardRecursivePathRequest {
                destination: stranger,
                id: [0x55; 16],
            },
        );
        assert_eq!(
            relay
                .recursive_path_requests
                .begin(stranger, source, InstantMillis(2_000)),
            RecursiveOutcome::AlreadyInFlight
        );
    }

    #[test]
    fn a_flooded_discover_interface_stops_forwarding_path_requests() {
        let source = iface(0xA1);
        let mut relay = transporting_node();
        let interfaces = [discovering_descriptor(source, InterfaceMode::Gateway)];
        let mut forwarded = 0;
        let mut dropped_after_forwarding = false;
        for dest_byte in 1..=8u8 {
            let mut buf = [0u8; BROADCAST_MTU];
            let n = crate::engine::write_path_request_wire_packet(
                DestinationHash::new([dest_byte; 16]),
                None,
                &[dest_byte; 16],
                &mut buf,
            )
            .unwrap();
            let mut wire = buf[..n].to_vec();
            match relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000 + u64::from(dest_byte)),
                    source_interface: source,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ) {
                IngestPacketOutcome::ForwardRecursivePathRequest { .. } => forwarded += 1,
                IngestPacketOutcome::Ignored(IgnoreReason::RateLimited) if forwarded > 0 => {
                    dropped_after_forwarding = true
                }
                _ => {}
            }
        }

        assert!(
            forwarded >= 1,
            "the first unknown-destination requests are forwarded"
        );
        assert!(
            dropped_after_forwarding,
            "once the interface floods unknown-destination requests, the recursive forward is dropped",
        );
    }

    #[test]
    fn a_second_discovery_for_the_same_stranger_is_not_forwarded_again() {
        let source = iface(0xA1);
        let mut relay = transporting_node();
        let interfaces = [discovering_descriptor(source, InterfaceMode::Gateway)];

        let mut first = stranger_path_request([0x55; 16]);
        assert!(matches!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut first,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ForwardRecursivePathRequest { .. },
        ));

        let mut second = stranger_path_request([0x66; 16]);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: source,
                    bytes: &mut second,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::Superseded),
        );
    }

    #[test]
    fn a_transport_node_does_not_discover_on_a_full_mode_interface() {
        let source = iface(0xA1);
        let mut relay = transporting_node();
        let interfaces = [routable_descriptor(source)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    #[test]
    fn a_local_clients_unknown_path_request_fans_out_to_the_network() {
        let stranger = DestinationHash::new([0x44; 16]);
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
        let uplink = iface(0xB2);
        let mut relay = transporting_node();
        let interfaces = [routable_descriptor(app), routable_descriptor(uplink)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: app,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ForwardRecursivePathRequest {
                destination: stranger,
                id: [0x55; 16],
            },
            "a local client's request for an unheard destination fans out so the network can answer",
        );
        assert_eq!(
            relay
                .recursive_path_requests
                .begin(stranger, app, InstantMillis(2_000)),
            RecursiveOutcome::AlreadyInFlight,
            "the asking client is remembered so the answer is steered back to it",
        );
    }

    #[test]
    fn a_leaf_shared_instance_relays_path_requests_across_its_local_boundary() {
        let destination = DestinationHash::new([0x44; 16]);
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"nomadnet");
        let network = iface(0xB2);
        let interfaces = [routable_descriptor(app), routable_descriptor(network)];

        for (source, expected_target, tag) in
            [(app, network, [0x55; 16]), (network, app, [0x66; 16])]
        {
            let mut leaf = shared_instance_leaf();
            let mut wire = stranger_path_request(tag);
            let mut sent = None;
            leaf.ingest_packet_into(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut wire,
                },
                IngestIo {
                    interfaces: AttachedInterfaces::new(&interfaces),
                    now: InstantMillis(1_000),
                    fill_entropy: &mut |_| {},
                    should_prove: &mut |_| false,
                    should_accept_resource: &mut |_| false,
                    sink: &mut |reaction| {
                        if let EngineReaction::Directive(Directive::Send { target, bytes }) =
                            reaction
                        {
                            sent = Some((target, bytes.to_vec()));
                        }
                    },
                },
            );

            let (target, wire) = sent.expect("the request crosses the local boundary");
            assert_eq!(target, expected_target);
            let (_, payload) = WirePacketHeader::parse(&wire).unwrap();
            let request = PathRequest::parse(payload).unwrap();
            assert_eq!(request.destination, destination);
            assert_eq!(request.requester_transport_id, None);
            assert!(!leaf.network_transport_enabled());
        }
    }

    #[test]
    fn path_request_egress_control_stops_the_seventh_request_in_a_burst() {
        let source = iface(0xA1);
        let target = iface(0xB2);
        let mut source_descriptor = discovering_descriptor(source, InterfaceMode::Gateway);
        source_descriptor.common.ingress_control.enabled = false;
        let mut target_descriptor = routable_descriptor(target);
        target_descriptor.common.path_request_egress =
            crate::interfaces::PathRequestEgressControl {
                enabled: true,
                frequency: crate::interfaces::FrequencyMilliHertz::new(5_000),
            };
        let interfaces = [source_descriptor, target_descriptor];
        let mut relay = transporting_node();
        let mut sent = 0;

        for byte in 1..=7u8 {
            let mut buf = [0u8; BROADCAST_MTU];
            let n = crate::engine::write_path_request_wire_packet(
                DestinationHash::new([byte; 16]),
                None,
                &[byte; 16],
                &mut buf,
            )
            .unwrap();
            relay.ingest_packet_into(
                InboundPacket {
                    arrived_at: InstantMillis(1_000 + u64::from(byte)),
                    source_interface: source,
                    bytes: &mut buf[..n],
                },
                IngestIo {
                    interfaces: AttachedInterfaces::new(&interfaces),
                    now: InstantMillis(1_000 + u64::from(byte)),
                    fill_entropy: &mut |_| {},
                    should_prove: &mut |_| false,
                    should_accept_resource: &mut |_| false,
                    sink: &mut |reaction| {
                        if matches!(
                            reaction,
                            EngineReaction::Directive(Directive::Send {
                                target: emitted_target,
                                ..
                            }) if emitted_target == target
                        ) {
                            sent += 1;
                        }
                    },
                },
            );
        }

        assert_eq!(sent, 6);
    }

    #[test]
    fn a_network_request_for_an_unheld_destination_is_offered_to_local_clients_only() {
        let stranger = DestinationHash::new([0x44; 16]);
        let uplink = iface(0xA1);
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"nomadnet");
        let mut relay = transporting_node();
        let interfaces = [routable_descriptor(uplink), routable_descriptor(app)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: uplink,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::RelayPathRequestToLocalClients {
                destination: stranger,
                id: [0x55; 16],
            },
            "a network request a plain shared instance can't answer is offered to its apps",
        );
        assert_eq!(
            relay
                .recursive_path_requests
                .begin(stranger, uplink, InstantMillis(2_000)),
            RecursiveOutcome::AlreadyInFlight,
        );
    }

    #[test]
    fn a_full_mode_request_with_no_local_clients_is_still_ignored() {
        let uplink = iface(0xA1);
        let other = iface(0xB2);
        let mut relay = transporting_node();
        let interfaces = [routable_descriptor(uplink), routable_descriptor(other)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: uplink,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
            "with no apps sharing the instance, an unanswerable full-mode request stays silent",
        );
    }

    #[test]
    fn a_leaf_does_not_discover_even_on_a_gateway_interface() {
        let source = iface(0xA1);
        let mut leaf: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let interfaces = [discovering_descriptor(source, InterfaceMode::AccessPoint)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            leaf.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    #[test]
    fn an_answering_path_response_is_steered_back_to_the_interface_that_asked() {
        use crate::engine::{Directive, EngineReaction, PathResponseWriteOutcome};

        let mut b = personal_node_announcer();
        let local = personal_node_destination();
        let mut buf = [0u8; BROADCAST_MTU];
        let PathResponseWriteOutcome::Written { wire_len, .. } = b
            .write_path_response_for_upstream(
                &local,
                InstantMillis(500),
                &mut test_fill_entropy,
                &mut buf,
            )
        else {
            panic!("a local destination is answerable");
        };

        let requester = iface(0xA1);
        let mut a = transporting_node();
        assert_eq!(
            a.recursive_path_requests
                .begin(local, requester, InstantMillis(60_000)),
            RecursiveOutcome::Opened
        );

        let mut wire = buf[..wire_len].to_vec();
        assert!(matches!(
            a.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_200),
                    source_interface: iface(0xB2),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));

        let interfaces = [
            routable_descriptor(requester),
            routable_descriptor(iface(0xB2)),
        ];
        let mut targets = std::vec::Vec::new();
        a.fire_due_scheduled_announces(
            InstantMillis(1_200),
            AttachedInterfaces::new(&interfaces),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction
                {
                    targets.push(target);
                }
            },
        );
        assert_eq!(targets, std::vec![requester]);
    }

    fn path_request_wire(destination: DestinationHash) -> std::vec::Vec<u8> {
        let mut buf = [0u8; BROADCAST_MTU];
        let n =
            crate::engine::write_path_request_wire_packet(destination, None, &[0x55; 16], &mut buf)
                .unwrap();
        buf[..n].to_vec()
    }

    fn path_request_wire_with(body: &[u8]) -> std::vec::Vec<u8> {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            address: PATH_REQUEST_DESTINATION.to_address(),
            context: WireContext::None,
        };
        let mut wire = std::vec![0u8; HEADER_MIN_LEN];
        header.write(&mut wire).unwrap();
        wire.extend_from_slice(body);
        wire
    }

    #[test]
    fn a_transport_form_request_answers_and_dedups_on_the_id_not_the_transport_id() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let transport_id = [0x7a; 16];
        let id = [0x55; 16];
        let mut body = std::vec::Vec::new();
        body.extend_from_slice(cached.as_bytes());
        body.extend_from_slice(&transport_id);
        body.extend_from_slice(&id);

        let mut wire = path_request_wire_with(&body);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
            "the 48-byte transport form is parsed and answered",
        );

        let mut same_id_other_transport = std::vec::Vec::new();
        same_id_other_transport.extend_from_slice(cached.as_bytes());
        same_id_other_transport.extend_from_slice(&[0xCC; 16]);
        same_id_other_transport.extend_from_slice(&id);
        let mut wire = path_request_wire_with(&same_id_other_transport);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::Duplicate),
            "a different transport id but the same id is the same request — deduped",
        );
    }

    #[test]
    fn an_unresponsive_route_is_withheld_then_vouched_for_again_once_it_recovers() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let transport_id = [0x7a; 16];

        relay
            .routing_table
            .mark_responsiveness(&cached, RouteResponsiveness::Unresponsive);

        let mut withheld = std::vec::Vec::new();
        withheld.extend_from_slice(cached.as_bytes());
        withheld.extend_from_slice(&transport_id);
        withheld.extend_from_slice(&[0x11; 16]);
        let mut wire = path_request_wire_with(&withheld);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::RouteUnresponsive),
            "an unresponsive route is withheld so a node with a live path answers instead",
        );

        relay
            .routing_table
            .mark_responsiveness(&cached, RouteResponsiveness::Responsive);

        let mut recovered = std::vec::Vec::new();
        recovered.extend_from_slice(cached.as_bytes());
        recovered.extend_from_slice(&transport_id);
        recovered.extend_from_slice(&[0x22; 16]);
        let mut wire = path_request_wire_with(&recovered);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
            "once marked responsive again, we vouch for the route once more",
        );
    }

    #[test]
    fn a_request_whose_requester_is_our_next_hop_is_declined() {
        let cached = DestinationHash::new(
            bytes_from_hex("c3cfae69b36bb6e3bbfd96a3b5867a59")
                .try_into()
                .unwrap(),
        );
        let mut relay = transporting_node();
        let mut announce = bytes_from_hex(RNS_1_3_5_RETRANSMITTED_ANNOUNCE);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: iface(0xB2),
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let request = |requester: [u8; 16], id: u8| {
            let mut body = std::vec::Vec::new();
            body.extend_from_slice(cached.as_bytes());
            body.extend_from_slice(&requester);
            body.extend_from_slice(&[id; 16]);
            path_request_wire_with(&body)
        };

        let mut loops_back = request([0x7a; 16], 0x01);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut loops_back,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::LoopPrevented),
            "the requester is the via we'd route through — answering would loop",
        );

        let mut other_requester = request([0xCC; 16], 0x02);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: iface(0xA1),
                    bytes: &mut other_requester,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
            "a different requester gets the cached path",
        );
    }

    #[test]
    fn an_idless_path_request_is_ignored() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let mut wire = path_request_wire_with(cached.as_bytes());
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::Malformed),
            "a bare destination carries no id — the reference ignores it",
        );
    }

    #[test]
    fn a_transport_node_answers_a_path_request_from_its_cache() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let mut wire = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
        );

        let scheduled = relay.scheduled_announces.iter().next().unwrap();
        assert_eq!(scheduled.destination, cached);
        assert_eq!(
            scheduled.due_at,
            InstantMillis(1_000 + PATH_REQUEST_GRACE_MS),
            "the cache answer waits out the grace before firing",
        );
        assert_eq!(
            scheduled.directed_to,
            Some(iface(0xA1)),
            "it is directed at the requester, not flooded",
        );
    }

    #[test]
    fn a_cache_answer_at_the_numeric_time_limit_saturates_its_deadline() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let mut wire = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(u64::MAX),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
        );
        assert_eq!(
            relay.scheduled_announces.iter().next().unwrap().due_at,
            InstantMillis(u64::MAX),
        );
    }

    #[test]
    fn a_roaming_requester_earns_the_extra_grace() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let requester = iface(0xA1);
        let roaming_view = [InterfaceDescriptor {
            mode: InterfaceMode::Roaming,
            ..routable_descriptor(requester)
        }];
        let mut wire = path_request_wire(cached);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: requester,
                bytes: &mut wire,
            },
            &mut |_| {},
            AttachedInterfaces::new(&roaming_view),
            &mut |_| {},
            None,
        );
        assert_eq!(
            relay.scheduled_announces.iter().next().unwrap().due_at,
            InstantMillis(1_000 + PATH_REQUEST_GRACE_MS + PATH_REQUEST_ROAMING_GRACE_MS),
        );
    }

    #[test]
    fn a_path_request_on_the_roaming_interface_the_route_lives_on_is_not_answered() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let learned_on = iface(0xB2);
        let roaming_view = [discovering_descriptor(learned_on, InterfaceMode::Roaming)];
        let mut wire = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: learned_on,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&roaming_view),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::LoopPrevented),
            "a roaming interface does not answer for a route that lives on it",
        );
        assert_eq!(
            relay.scheduled_announces.iter().next().unwrap().directed_to,
            None,
            "the suppressed request scheduled no directed answer; the flood rebroadcast stands",
        );
    }

    #[test]
    fn the_same_interface_answers_when_it_is_not_in_roaming_mode() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let learned_on = iface(0xB2);
        let full_view = [routable_descriptor(learned_on)];
        let mut wire = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: learned_on,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&full_view),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
            "the same-interface suppression is roaming-only; a Full interface still answers",
        );
    }

    #[test]
    fn a_flood_schedule_supersedes_a_directed_answer_for_the_same_destination() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let mut wire = path_request_wire(cached);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: iface(0xA1),
                bytes: &mut wire,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(
            relay.scheduled_announces.iter().next().unwrap().directed_to,
            Some(iface(0xA1)),
        );

        relay
            .scheduled_announces
            .schedule(cached, InstantMillis(1_100), iface(0xEE), 2);
        assert_eq!(relay.scheduled_announces.scheduled_count(), 1);
        assert_eq!(
            relay.scheduled_announces.iter().next().unwrap().directed_to,
            None,
            "a fresher announce reclaims the entry as a flood — the grace answer is cancelled",
        );
    }

    #[test]
    fn the_cache_answer_fires_to_the_requester_only_after_the_grace_deadline() {
        use crate::engine::{Directive, EngineReaction};
        use crate::wire::{PropagationType, WireContext};

        let (mut relay, cached) = relay_holding_a_cached_route();
        let requester = iface(0xA1);
        let interfaces = [
            routable_descriptor(requester),
            routable_descriptor(iface(0xEE)),
        ];

        let mut wire = path_request_wire(cached);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: requester,
                bytes: &mut wire,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );

        let mut early = std::vec::Vec::new();
        relay.fire_due_scheduled_announces(
            InstantMillis(1_000 + PATH_REQUEST_GRACE_MS - 1),
            AttachedInterfaces::new(&interfaces),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction
                {
                    early.push(target);
                }
            },
        );
        assert!(early.is_empty(), "nothing fires before the grace deadline");

        let mut fired = std::vec::Vec::new();
        relay.fire_due_scheduled_announces(
            InstantMillis(1_000 + PATH_REQUEST_GRACE_MS),
            AttachedInterfaces::new(&interfaces),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce {
                    target, bytes, ..
                }) = reaction
                {
                    fired.push((target, bytes.to_vec()));
                }
            },
        );
        assert_eq!(fired.len(), 1, "exactly one answer, to the one requester");
        assert_eq!(fired[0].0, requester);
        let (header, _) = WirePacketHeader::parse(&fired[0].1).unwrap();
        assert_eq!(DestinationHash::from_address(header.address), cached);
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.propagation,
            PropagationType::Transport,
            "a transport retransmission of the cached announce, directed at the asker",
        );
        assert_eq!(
            header.context,
            WireContext::PathResponse,
            "the directed answer is tagged PATH_RESPONSE so the requester takes it \
             as a terminal path response instead of re-flooding it as a fresh announce",
        );
    }

    #[test]
    fn a_leaf_with_a_route_but_no_transport_role_does_not_answer_from_cache() {
        let cached = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );
        let mut leaf: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let mut announce = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = leaf.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: iface(0xB2),
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let mut wire = path_request_wire(cached);
        assert_eq!(
            leaf.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
            "without a transport role a node never answers from cache, even holding the route",
        );
    }

    #[test]
    fn a_nontransport_shared_instance_answers_its_local_client_from_cache() {
        let cached = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
        let uplink = iface(0xB2);
        let mut shared: EngineState<TestStorageLayout> =
            EngineState::<TestStorageLayout>::default();
        let mut announce = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = shared.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: uplink,
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let mut wire = path_request_wire(cached);
        assert_eq!(
            shared.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: app,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&[routable_descriptor(app), routable_descriptor(uplink)]),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ScheduledPathResponse { destination: cached },
            "a non-transport shared instance answers its local client from cache, like RNS is_from_local_client",
        );
    }

    #[test]
    fn a_nontransport_shared_instance_forwards_its_local_clients_request_for_a_stranger() {
        let stranger = DestinationHash::new([0x44; 16]);
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
        let uplink = iface(0xB2);
        let mut shared: EngineState<TestStorageLayout> =
            EngineState::<TestStorageLayout>::default();
        let interfaces = [routable_descriptor(app), routable_descriptor(uplink)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            shared.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: app,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ForwardRecursivePathRequest {
                destination: stranger,
                id: [0x55; 16],
            },
            "a non-transport shared instance forwards its local client's unknown-path request to the network",
        );
    }

    #[test]
    fn a_nontransport_shared_instance_offers_a_network_request_to_its_local_clients() {
        let stranger = DestinationHash::new([0x44; 16]);
        let uplink = iface(0xA1);
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"nomadnet");
        let mut shared: EngineState<TestStorageLayout> =
            EngineState::<TestStorageLayout>::default();
        let interfaces = [routable_descriptor(uplink), routable_descriptor(app)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            shared.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: uplink,
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::RelayPathRequestToLocalClients {
                destination: stranger,
                id: [0x55; 16],
            },
            "a non-transport shared instance offers a network request it can't answer to its apps",
        );
    }

    #[test]
    fn a_transport_node_with_no_route_does_not_forward_the_request() {
        let mut relay = transporting_node();
        let mut wire = path_request_wire(DestinationHash::new([0x44; 16]));
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    #[test]
    fn a_duplicate_path_request_is_not_answered_twice() {
        let (mut relay, cached) = relay_holding_a_cached_route();

        let mut first = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut first,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
        );

        let mut echo = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: iface(0xB2),
                    bytes: &mut echo,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::Duplicate),
            "the same (destination, id) is a duplicate, not answered again",
        );
    }
}
