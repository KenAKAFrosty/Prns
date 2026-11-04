use super::*;
use crate::engine::{CommandId, PathFound, PathRequestId, RequestPath, Settlement};
use crate::routing::announce::{write_path_response_announce_wire_packet, Announce};
use crate::routing::links::transported::TransportedLink;
use crate::routing::links::{LinkId, LinkMode};
use crate::routing::path_requests::request_path::PathRequestWriteOutcome;
use crate::units::HopCount;

#[test]
fn unresponsive_cached_responses_keep_loop_and_interface_guards() {
    enum Guard {
        RoamingSameInterface,
        RequesterIsNextHop,
        RetiredInterface,
    }
    for guard in [
        Guard::RoamingSameInterface,
        Guard::RequesterIsNextHop,
        Guard::RetiredInterface,
    ] {
        let requester = iface(0xA1);
        let onward = iface(0xB2);
        let mut relay = transporting_node();
        let mut original = bytes_from_hex(RNS_1_4_2_RETRANSMITTED_ANNOUNCE);
        let destination =
            DestinationHash::from_address(WirePacketHeader::parse(&original).unwrap().0.address);
        let _ = relay.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: onward,
                bytes: &mut original,
            },
            AttachedInterfaces::new(&connected_route_interfaces()),
        );
        let _ = relay.scheduled_announces.cancel(&destination);
        relay
            .routing_table
            .mark_responsiveness(&destination, RouteResponsiveness::Unresponsive);
        let before = relay.routing_table.path_row(&destination).unwrap();
        let descriptors = [
            routable_descriptor(requester),
            discovering_descriptor(
                onward,
                if matches!(guard, Guard::RoamingSameInterface) {
                    InterfaceMode::Roaming
                } else {
                    InterfaceMode::Full
                },
            ),
        ];
        let view = if matches!(guard, Guard::RetiredInterface) {
            &descriptors[..1]
        } else {
            &descriptors[..]
        };
        let source = if matches!(guard, Guard::RoamingSameInterface) {
            onward
        } else {
            requester
        };
        let mut request = [0; BROADCAST_MTU];
        let len = write_path_request_wire_packet(
            destination,
            matches!(guard, Guard::RequesterIsNextHop).then_some(TEST_TRANSPORT_ID),
            &[0x66; 16],
            &mut request,
        )
        .unwrap();
        assert_eq!(
            relay.ingest_for_test(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut request[..len]
                },
                AttachedInterfaces::new(view)
            ),
            IngestPacketOutcome::Ignored(if matches!(guard, Guard::RetiredInterface) {
                IgnoreReason::NotForUs
            } else {
                IgnoreReason::LoopPrevented
            }),
        );
        assert_eq!(relay.scheduled_announce_count(), 0);
        assert_eq!(relay.routing_table.path_row(&destination).unwrap(), before);
    }
}

#[test]
fn cached_path_response_survives_transported_proof_expiry_without_renewing_evidence() {
    let requester = iface(0xA1);
    let onward = iface(0xB2);
    let descriptors = [routable_descriptor(requester), routable_descriptor(onward)];
    let interfaces = AttachedInterfaces::new(&descriptors);
    let (mut relay, destination) = relay_holding_a_cached_route();
    let _ = relay.scheduled_announces.cancel(&destination);
    relay
        .transported_links
        .track(TransportedLink {
            link_id: LinkId::new([0x5C; 16]),
            destination,
            route_evidence: relay
                .routing_table
                .route_evidence_handle_for(&destination)
                .unwrap(),
            mode: LinkMode::Aes256Cbc,
            next_hop: None,
            next_hop_interface: onward,
            received_interface: requester,
            taken_hops: 1,
            remaining_hops: 1,
            validated_by_proof: false,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(7_000),
        })
        .unwrap();
    let mut recovery_targets = std::vec::Vec::new();
    let _ = relay.fire_due_link_deadlines(
        InstantMillis(7_000),
        interfaces,
        &mut |bytes: &mut [u8]| bytes.fill(0x77),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                recovery_targets.push(target);
            }
        },
    );
    assert_eq!(recovery_targets, [onward]);
    let failed_row = relay.routing_table.path_row(&destination).unwrap();
    assert_eq!(failed_row.responsiveness, RouteResponsiveness::Unresponsive);

    // Its neighbor answers the recovery request with the original signed announce.
    // Equal-hop replay protection must continue to reject this as new route evidence.
    let original = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    let (header, payload) = WirePacketHeader::parse(&original).unwrap();
    let announce = Announce::from_wire(&header, payload).unwrap();
    let mut response = [0; BROADCAST_MTU];
    let response_len =
        write_path_response_announce_wire_packet(&announce, 0, &mut response).unwrap();
    let _ = relay.ingest_for_test(
        InboundPacket {
            arrived_at: InstantMillis(7_100),
            source_interface: onward,
            bytes: &mut response[..response_len],
        },
        interfaces,
    );
    assert_eq!(
        relay.routing_table.path_row(&destination).unwrap(),
        failed_row
    );

    // The leaf has retired its failed route. Full mode must still answer its next
    // request from the retained cache, as RNS1.5.4 Transport.path_request does.
    let mut leaf = EngineState::<TestStorageLayout>::default();
    let request_id = CommandId(88);
    let mut request = [0; BROADCAST_MTU];
    let PathRequestWriteOutcome::Written { wire_bytes, .. } = leaf
        .write_commanded_path_request_with_interfaces(
            request_id,
            &RequestPath {
                destination,
                id: PathRequestId::new([0x31; 16]),
            },
            InstantMillis(8_000),
            interfaces,
            &mut request,
        )
    else {
        panic!("request must fit");
    };
    assert_eq!(
        relay.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(8_000),
                source_interface: requester,
                bytes: &mut request[..wire_bytes],
            },
            interfaces,
        ),
        IngestPacketOutcome::ScheduledPathResponse { destination },
        "a retained unresponsive route must not silently strand Full-mode requesters",
    );
    let mut replies = std::vec::Vec::new();
    let now = InstantMillis(8_000 + PATH_REQUEST_GRACE_MS);
    let _ = relay.fire_due_scheduled_announces(now, interfaces, &mut |reaction| {
        if let EngineReaction::Directive(Directive::SendAnnounce { target, bytes, .. }) = reaction {
            replies.push((target, bytes.to_vec()));
        }
    });
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].0, requester);
    let (reply_header, reply_payload) = WirePacketHeader::parse(&replies[0].1).unwrap();
    assert_eq!(reply_header.context, WireContext::PathResponse);
    assert_eq!(
        reply_payload, payload,
        "the cached signed payload is unchanged"
    );
    let mut settlements = std::vec::Vec::new();
    crate::engine::drive_packet_to_quiescence(
        &mut leaf,
        InboundPacket {
            arrived_at: now,
            source_interface: requester,
            bytes: &mut replies[0].1,
        },
        IngestIo {
            interfaces,
            now,
            fill_random: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settlements.push((id, settlement));
                }
            },
        },
    );
    assert_eq!(
        settlements,
        [(
            request_id,
            Settlement::RequestPath(Ok(PathFound { hops: HopCount(2) }))
        )]
    );
    assert_eq!(leaf.routing_table.hop_count_to(&destination), Some(2));
    assert_eq!(
        relay.routing_table.path_row(&destination).unwrap(),
        failed_row,
        "answering a request does not mark a failed path healthy or extend its lifetime"
    );
}

#[test]
fn replacement_requester_does_not_wait_for_departed_interfaces_recursive_request() {
    for departure in [
        crate::routing::warmth::Departure::Forgotten,
        crate::routing::warmth::Departure::MayReturn,
    ] {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let cached_before = relay.routing_table.path_row(&cached).unwrap();
        relay.set_protocol_policy(crate::engine::EngineProtocolPolicy {
            recursive_path_request_default: RecursivePathRequestDefault::Enabled,
            ..Default::default()
        });
        let old_phone = iface(0xA1);
        let new_phone = iface(0xA2);
        let radio = iface(0xB2);
        let destination = DestinationHash::new([0x44; 16]);
        let mut local_request = [0; BROADCAST_MTU];
        let _ = relay.write_commanded_path_request(
            CommandId(77),
            &RequestPath {
                destination,
                id: PathRequestId::new([0x77; 16]),
            },
            InstantMillis(500),
            &mut local_request,
        );
        for (phone, tag, now) in [(old_phone, 1u8, 1_000), (new_phone, 2u8, 2_000)] {
            if phone == new_phone {
                relay.interface_departed(old_phone, departure, InstantMillis(1_500));
                let mut duplicate = [0; BROADCAST_MTU];
                let size =
                    write_path_request_wire_packet(destination, None, &[1; 16], &mut duplicate)
                        .unwrap();
                assert_eq!(
                    relay.ingest_for_test(
                        InboundPacket {
                            arrived_at: InstantMillis(1_600),
                            source_interface: new_phone,
                            bytes: &mut duplicate[..size],
                        },
                        AttachedInterfaces::new(&[
                            routable_descriptor(new_phone),
                            routable_descriptor(radio)
                        ])
                    ),
                    IngestPacketOutcome::Ignored(IgnoreReason::Duplicate),
                    "departure does not erase replay protection"
                );
            }
            let descriptors = [routable_descriptor(phone), routable_descriptor(radio)];
            let mut request = [0; BROADCAST_MTU];
            let size = write_path_request_wire_packet(destination, None, &[tag; 16], &mut request)
                .unwrap();
            assert_eq!(relay.ingest_for_test(InboundPacket {
                arrived_at: InstantMillis(now), source_interface: phone, bytes: &mut request[..size],
            }, AttachedInterfaces::new(&descriptors)), IngestPacketOutcome::ForwardRecursivePathRequest { destination, id: [tag; 16] },
            "the old requesting interface cannot receive the pending response; it must not suppress its replacement");
        }
        assert_eq!(
            relay.routing_table.path_row(&cached).unwrap(),
            cached_before,
            "retained signed route evidence is independent of a departed requester"
        );
        assert_eq!(
            relay
                .pop_timed_out_path_request(InstantMillis(60_000))
                .unwrap()
                .command_id,
            CommandId(77),
            "local caller obligations remain accountable"
        );
        assert_eq!(
            relay.recursive_path_requests.take_requester(&destination),
            Some(new_phone)
        );
    }
}
