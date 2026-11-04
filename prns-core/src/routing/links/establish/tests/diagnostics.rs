use super::*;
use crate::engine::{
    LinkHandshakeEvent, LinkHandshakeIngress, LinkHandshakePhase, LinkProofCompletion,
    LinkTimeoutRouteDisposition,
};
use std::cell::RefCell;

std::thread_local! { static EVENTS: RefCell<std::vec::Vec<LinkHandshakeEvent>> = const { RefCell::new(std::vec::Vec::new()) }; }
fn observe(event: LinkHandshakeEvent) -> bool {
    EVENTS.with(|events| events.borrow_mut().push(event));
    true
}
fn take() -> std::vec::Vec<LinkHandshakeEvent> {
    EVENTS.with(|events| core::mem::take(&mut *events.borrow_mut()))
}
fn endpoint() -> EngineState<TestStorageLayout> {
    take();
    let mut state = neighbor_with_a_route();
    let identity = state.held_identity_hashes()[0];
    state.set_non_routing_identity(&identity).unwrap();
    state.set_link_handshake_observer(Some(observe));
    state
}
fn start(state: &mut EngineState<TestStorageLayout>) -> (LinkRequestDispatch, std::vec::Vec<u8>) {
    let mut wire = [0; BROADCAST_MTU];
    let dispatch = state
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut wire,
        )
        .dispatched();
    (dispatch, wire[..dispatch.wire_bytes].to_vec())
}
fn expire(state: &mut EngineState<TestStorageLayout>, now: u64) {
    state.fire_due_link_deadlines(
        InstantMillis(now),
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut test_fill_entropy,
        &mut |_| {},
    );
}

#[test]
fn a_lost_request_and_a_lost_proof_have_distinct_positive_phase_evidence() {
    for deliver_request in [false, true] {
        let mut initiator = endpoint();
        let (dispatch, request) = start(&mut initiator);
        let mut responder = personal_node_announcer();
        responder.set_link_handshake_observer(Some(observe));
        if deliver_request {
            let (proofs, _, _) = reactions_of(&mut responder, &request, 1100, 0x99);
            assert_eq!(proofs.len(), 1); // Deliberately drop the proof.
        }
        expire(&mut initiator, 13001);
        let events = take();
        assert!(events
            .iter()
            .all(|event| event.link_id == Some(dispatch.link_id)));
        assert!(matches!(
            events[0].phase,
            LinkHandshakePhase::RequestPending {
                requested_at: InstantMillis(1000),
                timeout_at: InstantMillis(13001),
                ..
            }
        ));
        assert_eq!(
            events
                .iter()
                .any(|event| event.phase == LinkHandshakePhase::ProofPrepared),
            deliver_request
        );
        assert!(!events
            .iter()
            .any(|event| matches!(event.phase, LinkHandshakePhase::ProofIngress(_))));
        assert!(matches!(
            events.last().unwrap().phase,
            LinkHandshakePhase::InitiatorTimeout {
                route: LinkTimeoutRouteDisposition::Removed,
                ..
            }
        ));
        assert!(!initiator.routing_table.has_route(&peer_destination()));
    }
}

#[test]
fn successful_handshake_and_invalid_inline_proof_are_distinguished() {
    for forge in [false, true] {
        let mut initiator = endpoint();
        let (dispatch, request) = start(&mut initiator);
        let mut responder = personal_node_announcer();
        responder.set_link_handshake_observer(Some(observe));
        let (proofs, _, _) = reactions_of(&mut responder, &request, 1100, 0x99);
        let mut proof = proofs[0].clone();
        if forge {
            let header_len = WirePacketHeader::parse(&proof).unwrap().0.wire_len();
            proof[header_len] ^= 1; // Signature, preserving packet shape and Link ID.
            assert_eq!(
                initiator.ingest_for_test(
                    InboundPacket {
                        arrived_at: InstantMillis(1200),
                        source_interface: arrival(),
                        bytes: &mut proof,
                    },
                    AttachedInterfaces::new(&arrival_interfaces())
                ),
                IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid)
            );
            expire(&mut initiator, 13001);
        } else {
            let (rtts, settlements, _) = reactions_of(&mut initiator, &proof, 1200, 0xA5);
            assert_eq!(rtts.len(), 1);
            assert!(matches!(
                settlements.as_slice(),
                [(CommandId(7), Settlement::EstablishLink(Ok(_)))]
            ));
        }
        let events = take();
        assert!(events
            .iter()
            .all(|event| event.link_id == Some(dispatch.link_id)));
        assert_eq!(
            events.iter().any(|event| event.phase
                == LinkHandshakePhase::ProofCompletion(LinkProofCompletion::Activated)),
            !forge
        );
        assert_eq!(
            events.iter().any(|event| event.phase
                == LinkHandshakePhase::ProofIngress(LinkHandshakeIngress::Ignored(
                    IgnoreReason::ProofInvalid
                ))),
            forge
        );
    }
}

#[test]
fn deferred_request_observation_preserves_the_original_deadline_and_floor_inputs() {
    let mut initiator = endpoint();
    let interfaces = arrival_interfaces();
    let completed = initiator
        .prepare_establish_link(
            CommandId(7),
            establish(),
            InstantMillis(1000),
            Some(30_000),
            vector_establish_entropy(),
        )
        .unwrap()
        .fulfill();
    // Completion API intentionally has no clock; it must not pretend requested_at
    // is the completion time or extend the established protocol deadline.
    initiator.resume_establish_link(completed, AttachedInterfaces::new(&interfaces), &mut |_| {});
    let events = take();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].at, None);
    assert!(matches!(
        events[0].phase,
        LinkHandshakePhase::RequestPending {
            requested_at: InstantMillis(1000),
            timeout_at: InstantMillis(37000),
            hops: 1,
            first_hop_floor_ms: Some(30_000),
            computed_timeout_ms: 12_001,
            ..
        }
    ));
    expire(&mut initiator, 40_000);
    assert!(matches!(
        take()[0].phase,
        LinkHandshakePhase::InitiatorTimeout { .. }
    ));
}

#[test]
fn timeout_observation_reports_preserved_newer_route_evidence() {
    let mut initiator = endpoint();
    let (dispatch, _) = start(&mut initiator);
    let mut handle = initiator
        .routing_table
        .route_evidence_handle_for(&peer_destination())
        .unwrap();
    initiator
        .routing_table
        .apply_route_evidence(&mut handle, InstantMillis(2000));
    expire(&mut initiator, 13001);
    let events = take();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].link_id, Some(dispatch.link_id));
    assert!(matches!(
        events[1].phase,
        LinkHandshakePhase::InitiatorTimeout {
            route: LinkTimeoutRouteDisposition::Preserved,
            ..
        }
    ));
    assert!(initiator.routing_table.has_route(&peer_destination()));
}

#[test]
fn deferred_valid_proof_after_deadline_cannot_reactivate_and_reports_not_pending() {
    let mut initiator = endpoint();
    let (_, request) = start(&mut initiator);
    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request, 1100, 0x99);
    let mut proof = proofs[0].clone();
    let mut verification = None;
    initiator.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(1200),
            source_interface: arrival(),
            bytes: &mut proof,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(1200),
            fill_random: &mut test_fill_entropy,
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if let EngineReaction::Directive(Directive::Fulfill(OwedWork::Crypto(
                    CryptoOwed::LinkProofVerify(owed),
                ))) = reaction
                {
                    verification = Some(owed);
                }
            },
        },
    );
    let owed = verification.unwrap();
    assert!(crate::routing::links::handshake::link_proof_signature_valid(&owed));
    let shared = x25519_diffie_hellman(&owed.initiator_secret, &owed.responder_encryption);
    expire(&mut initiator, 13001);
    initiator.resume_link_proof(
        owed,
        shared,
        AttachedInterfaces::new(&arrival_interfaces()),
        InstantMillis(14000),
        &mut test_fill_entropy,
        &mut |_| panic!("expired proof must not send LRRTT or settle again"),
    );
    let events = take();
    assert!(events.iter().any(|event| event.phase
        == LinkHandshakePhase::ProofIngress(LinkHandshakeIngress::VerificationPending)));
    assert_eq!(
        events.last().unwrap().phase,
        LinkHandshakePhase::ProofCompletion(LinkProofCompletion::WriteRejected(
            WriteLinkRttError::NotPending
        ))
    );
    assert!(!events
        .iter()
        .any(|event| event.phase
            == LinkHandshakePhase::ProofCompletion(LinkProofCompletion::Activated)));
}

#[test]
fn relay_expiry_records_its_earlier_proof_deadline_before_late_proof_arrives() {
    take();
    let onward = InterfaceId::new([0xB7; 8]);
    let mut lora = routable_descriptor(onward);
    lora.bitrate = BitrateBps::guess(3515); // Trial SF9/250 kHz effective bitrate.
    let descriptors = [routable_descriptor(arrival()), lora];
    let interfaces = AttachedInterfaces::new(&descriptors);
    let (mut relay, mut responder) = relay_that_routes_to_the_responder(onward);
    // Teach a real parsed two-hop route, as on phone -> board -> board -> phone.
    let mut announce = [0; BROADCAST_MTU];
    let length = crate::routing::announce::write_retransmitted_announce_wire_packet(
        &relay
            .routing_table
            .stored_announce_for(&peer_destination())
            .unwrap()
            .announce,
        1,
        RESPONDER_TRANSPORT_ID,
        &mut announce,
    )
    .unwrap();
    relay.routing_table.drop_route(&peer_destination());
    assert!(matches!(
        relay.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(600),
                source_interface: onward,
                bytes: &mut announce[..length],
            },
            interfaces
        ),
        IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_))
    ));
    pin_transport_id(&mut responder, RESPONDER_TRANSPORT_ID);
    relay.set_link_handshake_observer(Some(observe));
    let mut initiator = EngineState::<TestStorageLayout>::new(second_secret_key());
    let mut request = transported_request_wire(&mut initiator);
    let (link_id, mut proof) = super::deadline_reference::relay_request_and_proof(
        &mut relay,
        &mut responder,
        &mut request,
        interfaces,
        1100,
    );
    let deadline = relay
        .transported_links
        .entry_for(&link_id)
        .unwrap()
        .proof_timeout;
    assert_eq!(deadline, InstantMillis(1100 + 13_138));
    // Controlled queue-delay injection, not a measured device queue residence:
    // 14 s is below the current 120 s queue-age bound but beyond this proof timer.
    let delayed_arrival = InstantMillis(1100 + 14_000);
    assert!(delayed_arrival > deadline);
    assert!(
        initiator.links.earliest_timeout_at().unwrap() > delayed_arrival,
        "the relay can expire while its upstream initiator is still waiting"
    );
    relay.fire_due_link_deadlines(deadline, interfaces, &mut test_fill_entropy, &mut |_| {});
    assert_eq!(
        relay.ingest_for_test(
            InboundPacket {
                arrived_at: delayed_arrival,
                source_interface: onward,
                bytes: &mut proof
            },
            interfaces
        ),
        IngestPacketOutcome::Ignored(IgnoreReason::UnknownLink)
    );
    let events = take();
    assert!(
        matches!(events[0].phase, LinkHandshakePhase::RelayPending { timeout_at, .. } if timeout_at == deadline)
    );
    assert!(events.iter().any(|event| matches!(event.phase, LinkHandshakePhase::RelayTimeout { timeout_at, .. } if timeout_at == deadline)));
    assert_eq!(
        events.last().unwrap().phase,
        LinkHandshakePhase::ProofIngress(LinkHandshakeIngress::Ignored(IgnoreReason::UnknownLink))
    );
}

#[test]
fn deferred_signature_rejection_is_observable_without_changing_pending_state() {
    let mut initiator = endpoint();
    let (dispatch, request) = start(&mut initiator);
    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request, 1100, 0x99);
    let mut proof = proofs[0].clone();
    let header_len = WirePacketHeader::parse(&proof).unwrap().0.wire_len();
    proof[header_len] ^= 1;
    let mut verification = None;
    initiator.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(1200),
            source_interface: arrival(),
            bytes: &mut proof,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(1200),
            fill_random: &mut test_fill_entropy,
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if let EngineReaction::Directive(Directive::Fulfill(OwedWork::Crypto(
                    CryptoOwed::LinkProofVerify(owed),
                ))) = reaction
                {
                    verification = Some(owed);
                }
            },
        },
    );
    let owed = verification.unwrap();
    assert!(!crate::routing::links::handshake::link_proof_signature_valid(&owed));
    let deadline = initiator.links.earliest_timeout_at();
    initiator.observe_link_proof_rejected(owed.link_id, owed.source_interface, InstantMillis(1250));
    assert!(matches!(
        initiator.links.phase_for(&dispatch.link_id),
        Some(LinkPhase::Pending { .. })
    ));
    assert_eq!(initiator.links.earliest_timeout_at(), deadline);
    let events = take();
    assert_eq!(events.last().unwrap().command, Some(CommandId(7)));
    assert_eq!(events.last().unwrap().at, Some(InstantMillis(1250)));
    assert_eq!(
        events.last().unwrap().phase,
        LinkHandshakePhase::ProofCompletion(LinkProofCompletion::InvalidSignature)
    );
}
