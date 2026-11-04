use super::*;
use crate::routing::links::transported::extra_link_proof_timeout_ms;
use crate::routing::timing::{first_hop_timeout_ms, link_establishment_timeout_ms};

#[test]
fn rns_1_5_4_link_deadline_vectors_keep_local_airtime_and_remote_hops_distinct() {
    // Captured from the pinned RNS 1.5.4 Transport.first_hop_timeout and
    // extra_link_proof_timeout, with Link.ESTABLISHMENT_TIMEOUT_PER_HOP.
    // Integer milliseconds round airtime upward, rather than shortening Python's timeout.
    for (bps, hops, first_hop, initiator, relay_extra) in [
        (None, 0, 6_000, 12_000, 0),
        (Some(1_000_000), 3, 6_004, 24_004, 4),
        (Some(696), 2, 11_748, 23_748, 5_748),
        (Some(250), 1, 22_000, 28_000, 16_000),
        (Some(5), 255, 806_000, 2_336_000, 800_000),
    ] {
        let bitrate = bps.map(BitrateBps::guess);
        assert_eq!(first_hop_timeout_ms(bitrate), first_hop);
        assert_eq!(link_establishment_timeout_ms(hops, bitrate), initiator);
        assert_eq!(bitrate.map_or(0, extra_link_proof_timeout_ms), relay_extra);
    }
}

pub(super) fn relay_request_and_proof(
    relay: &mut EngineState<TestStorageLayout>,
    responder: &mut EngineState<TestStorageLayout>,
    request: &mut [u8],
    interfaces: AttachedInterfaces<'_>,
    now: u64,
) -> (LinkId, std::vec::Vec<u8>) {
    let link_id = parse_link_request(request).unwrap().link_id;
    let outcome = relay.ingest_for_test(
        InboundPacket {
            arrived_at: InstantMillis(now),
            source_interface: arrival(),
            bytes: request,
        },
        interfaces,
    );
    let IngestPacketOutcome::TransportedLinkRequest { header, body, .. } = outcome else {
        panic!("the request must ride transport");
    };
    let mut forwarded = [0; BROADCAST_MTU];
    let header_len = header.write(&mut forwarded).unwrap();
    let wire_len = header_len + body.as_bytes().len();
    forwarded[header_len..wire_len].copy_from_slice(body.as_bytes());
    let (proofs, _, _) = reactions_of(responder, &forwarded[..wire_len], now + 1, 0x99);
    assert_eq!(proofs.len(), 1);
    (link_id, proofs[0].clone())
}

#[test]
fn a_valid_transported_proof_just_before_expiry_survives_the_scheduled_wake() {
    let onward = InterfaceId::new([0xB7; 8]);
    let mut outbound = routable_descriptor(onward);
    outbound.bitrate = BitrateBps::guess(696);
    let descriptors = [routable_descriptor(arrival()), outbound];
    let interfaces = AttachedInterfaces::new(&descriptors);
    let (mut relay, mut responder) = relay_that_routes_to_the_responder(onward);
    let mut initiator = EngineState::<TestStorageLayout>::new(second_secret_key());
    let mut request = transported_request_wire(&mut initiator);
    let (link_id, mut proof) =
        relay_request_and_proof(&mut relay, &mut responder, &mut request, interfaces, 1_100);
    let deadline = relay
        .transported_links
        .entry_for(&link_id)
        .unwrap()
        .proof_timeout;
    assert_eq!(deadline, InstantMillis(12_848));

    assert!(matches!(
        relay.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(deadline.0 - 1),
                source_interface: onward,
                bytes: &mut proof,
            },
            interfaces,
        ),
        IngestPacketOutcome::Forward(_)
    ));
    let mut emitted = 0;
    relay.fire_due_link_deadlines(
        deadline,
        interfaces,
        &mut test_fill_entropy,
        &mut |reaction| {
            if matches!(reaction, EngineReaction::Directive(Directive::Send { .. })) {
                emitted += 1;
            }
        },
    );
    assert_eq!(
        emitted, 0,
        "a proven link must not trigger failed-path recovery"
    );
    assert!(
        relay
            .transported_links
            .entry_for(&link_id)
            .unwrap()
            .validated_by_proof
    );
    assert!(relay.transported_links.earliest_deadline().unwrap() > deadline);
}

#[test]
fn a_late_transported_proof_cannot_revive_an_expired_attempt_or_credit_its_retry() {
    let onward = InterfaceId::new([0xB7; 8]);
    let descriptors = [routable_descriptor(arrival()), routable_descriptor(onward)];
    let interfaces = AttachedInterfaces::new(&descriptors);
    let (mut relay, mut responder) = relay_that_routes_to_the_responder(onward);
    let mut initiator = EngineState::<TestStorageLayout>::new(second_secret_key());
    let mut request = transported_request_wire(&mut initiator);
    let (expired_id, mut late_proof) =
        relay_request_and_proof(&mut relay, &mut responder, &mut request, interfaces, 1_100);
    let deadline = relay
        .transported_links
        .entry_for(&expired_id)
        .unwrap()
        .proof_timeout;
    relay.fire_due_link_deadlines(deadline, interfaces, &mut test_fill_entropy, &mut |_| {});
    assert!(relay.transported_links.entry_for(&expired_id).is_none());

    let mut retry = [0; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(8),
            &establish(),
            InstantMillis(deadline.0 + 1),
            EstablishLinkEntropy::new([0x44; EstablishLinkEntropy::LEN]),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut retry,
        )
        .dispatched();
    let (retry_id, mut retry_proof) = relay_request_and_proof(
        &mut relay,
        &mut responder,
        &mut retry[..dispatch.wire_bytes],
        interfaces,
        deadline.0 + 2,
    );
    assert_ne!(expired_id, retry_id);
    let pending_retry = *relay.transported_links.entry_for(&retry_id).unwrap();
    let pending_route = relay.routing_table.path_row(&peer_destination()).unwrap();

    assert_eq!(
        relay.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(deadline.0 + 4),
                source_interface: onward,
                bytes: &mut late_proof,
            },
            interfaces,
        ),
        IngestPacketOutcome::Ignored(IgnoreReason::UnknownLink)
    );
    assert!(relay.transported_links.entry_for(&expired_id).is_none());
    assert_eq!(
        relay.transported_links.entry_for(&retry_id),
        Some(&pending_retry)
    );
    assert_eq!(
        relay.routing_table.path_row(&peer_destination()),
        Some(pending_route)
    );
    assert!(matches!(
        relay.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(deadline.0 + 5),
                source_interface: onward,
                bytes: &mut retry_proof,
            },
            interfaces,
        ),
        IngestPacketOutcome::Forward(_)
    ));
    assert!(
        relay
            .transported_links
            .entry_for(&retry_id)
            .unwrap()
            .validated_by_proof
    );
}

#[test]
fn an_expired_initiators_proof_cannot_settle_or_confirm_a_new_attempt() {
    let mut initiator = neighbor_with_a_route();
    let mut request = [0; BROADCAST_MTU];
    let expired = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..expired.wire_bytes], 1_100, 0x99);
    let deadline = initiator.links.earliest_timeout_at().unwrap();
    let mut settlements = std::vec::Vec::new();
    initiator.fire_due_link_deadlines(
        deadline,
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut test_fill_entropy,
        &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settlements.push((id, settlement));
            }
        },
    );
    assert_eq!(
        settlements,
        [(
            CommandId(7),
            Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout))
        )]
    );
    assert!(initiator.links.phase_for(&expired.link_id).is_none());
    let retry = initiator
        .write_commanded_link_request(
            CommandId(8),
            &establish(),
            InstantMillis(deadline.0 + 1),
            EstablishLinkEntropy::new([0x44; EstablishLinkEntropy::LEN]),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    assert_ne!(expired.link_id, retry.link_id);
    let before = initiator
        .routing_table
        .path_row(&peer_destination())
        .unwrap();
    let (sent, settled, _) = reactions_of(&mut initiator, &proofs[0], deadline.0 + 2, 0xA5);
    assert!(sent.is_empty());
    assert!(settled.is_empty());
    assert!(initiator.links.phase_for(&expired.link_id).is_none());
    assert!(matches!(
        initiator.links.phase_for(&retry.link_id),
        Some(LinkPhase::Pending {
            command_id: CommandId(8),
            ..
        })
    ));
    assert_eq!(
        initiator.routing_table.path_row(&peer_destination()),
        Some(before)
    );
}
