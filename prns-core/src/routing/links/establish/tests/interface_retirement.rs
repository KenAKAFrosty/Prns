use super::*;
use crate::routing::links::handshake::LinkProofSignOwed;
use core::sync::atomic::{AtomicU32, Ordering};

fn sign_parts(owed: &LinkProofSignOwed) -> (X25519PublicKey, X25519SharedSecret, Ed25519Signature) {
    let encryption = x25519_public_key(&owed.ephemeral_secret);
    let shared = x25519_diffie_hellman(&owed.ephemeral_secret, &owed.request.initiator_encryption);
    let data = crate::routing::links::handshake::link_proof_signed_data(
        &owed.request.link_id,
        &encryption,
        owed.responder_signing.as_ed25519(),
        owed.mtu,
        owed.request.mode,
    );
    (
        encryption,
        shared,
        crate::crypto::ed25519_sign(&owed.signing_secret, &data),
    )
}

fn take_sign_work(
    responder: &mut EngineState<TestStorageLayout>,
    request: &[u8],
    interfaces: AttachedInterfaces<'_>,
) -> LinkProofSignOwed {
    let mut raw = request.to_vec();
    let mut work = None;
    responder.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(1_100),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces,
            now: InstantMillis(1_100),
            fill_random: &mut |bytes: &mut [u8]| bytes.fill(0x99),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |event| {
                if let EngineReaction::Directive(Directive::Fulfill(OwedWork::Crypto(
                    CryptoOwed::LinkProofSign(owed),
                ))) = event
                {
                    work = Some(owed);
                }
            },
        },
    );
    work.expect("request schedules actual deferred proof crypto")
}

#[test]
fn administrative_off_on_fences_deferred_establish_even_after_route_relearn() {
    let mut engine = neighbor_with_a_route();
    let epoch = AtomicU32::new(0);
    let generation = |_| epoch.load(Ordering::SeqCst);
    let descriptors = arrival_interfaces();
    let interfaces = AttachedInterfaces::new(&descriptors).with_generations(&generation);
    let owed = engine
        .prepare_establish_link_with_interfaces(
            CommandId(7),
            establish(),
            InstantMillis(1_000),
            None,
            vector_establish_entropy(),
            interfaces,
        )
        .unwrap();
    engine.interface_unavailable(arrival(), InstantMillis(1_001), interfaces, &mut |_| {});
    epoch.store(1, Ordering::SeqCst);
    engine.interface_attached(arrival(), InstantMillis(1_002));
    // A fresh authenticated announce can relearn the same path. It does not revive old work.
    let replacement = neighbor_with_a_route();
    let wire = replacement
        .routing_table
        .stored_announce_for(&peer_destination())
        .unwrap();
    let announce = wire.announce;
    let evidence =
        engine.route_evidence_id_for_update(&peer_destination(), arrival(), wire.next_hop);
    let _ = engine.routing_table.upsert_route(
        &crate::routing::AnnounceArrival {
            announce,
            hops: wire.hops,
            arrived_at: InstantMillis(1_003),
            receiving_interface: arrival(),
            next_hop: wire.next_hop,
            is_path_response: false,
        },
        evidence,
        interfaces,
        &mut |_| {},
    );
    let mut settlements = std::vec::Vec::new();
    engine.resume_establish_link(owed.fulfill(), interfaces, &mut |event| match event {
        EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
            settlements.push((id, settlement))
        }
        EngineReaction::Directive(Directive::Send { .. }) => {
            panic!("pre-Off crypto must not emit a new request")
        }
        _ => {}
    });
    assert_eq!(
        settlements,
        [(
            CommandId(7),
            Settlement::EstablishLink(Err(EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::RouteVanished
            )))
        )]
    );
    assert_eq!(engine.links.active_link_count(), 0);
    assert_eq!(engine.link_deadlines_wake(), WakeSchedule::Idle);
    let mut buf = [0u8; BROADCAST_MTU];
    let fresh = engine
        .write_commanded_link_request(
            CommandId(8),
            &establish(),
            InstantMillis(1_004),
            vector_establish_entropy(),
            interfaces,
            &mut buf,
        )
        .dispatched();
    assert!(engine.links.has_local_link(&fresh.link_id));
}

#[test]
fn administrative_off_at_final_route_selection_rejects_changed_disabled_lane() {
    let mut engine = neighbor_with_a_route();
    let descriptors = both_arrival_interfaces();
    let allowed = |id| id != other_arrival();
    let interfaces = AttachedInterfaces::new(&descriptors).with_availability(&allowed);
    let owed = engine
        .prepare_establish_link_with_interfaces(
            CommandId(7),
            establish(),
            InstantMillis(1_000),
            None,
            vector_establish_entropy(),
            interfaces,
        )
        .unwrap();
    let replacement = neighbor_with_a_route();
    let wire = replacement
        .routing_table
        .stored_announce_for(&peer_destination())
        .unwrap();
    engine.routing_table.drop_route(&peer_destination());
    let evidence =
        engine.route_evidence_id_for_update(&peer_destination(), other_arrival(), wire.next_hop);
    let _ = engine.routing_table.upsert_route(
        &crate::routing::AnnounceArrival {
            announce: wire.announce,
            hops: wire.hops,
            arrived_at: InstantMillis(1_001),
            receiving_interface: other_arrival(),
            next_hop: wire.next_hop,
            is_path_response: false,
        },
        evidence,
        AttachedInterfaces::new(&descriptors),
        &mut |_| {},
    );
    let mut buf = [0u8; BROADCAST_MTU];
    let outcome = engine.write_commanded_link_request_from_parts(
        owed.fulfill(),
        FirstHopTiming {
            interfaces,
            shared_instance_floor_ms: None,
        },
        &mut buf,
    );
    assert!(matches!(
        outcome,
        EstablishLinkWriteOutcome::Rejected {
            rejection: WriteEstablishLinkRejection::RouteVanished
        }
    ));
    assert_eq!(engine.link_deadlines_wake(), WakeSchedule::Idle);
}

#[test]
fn administrative_off_on_fences_deferred_proof_before_handshake_row_exists() {
    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let descriptors = arrival_interfaces();
    let epoch = AtomicU32::new(0);
    let generation = |_| epoch.load(Ordering::SeqCst);
    let interfaces = AttachedInterfaces::new(&descriptors).with_generations(&generation);
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            interfaces,
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let owed = take_sign_work(&mut responder, &request[..dispatch.wire_bytes], interfaces);
    assert_eq!(owed.source_generation, 0);
    assert!(!responder.links.has_local_link(&dispatch.link_id));
    responder.interface_unavailable(arrival(), InstantMillis(1_101), interfaces, &mut |_| {});
    epoch.store(1, Ordering::SeqCst);
    responder.interface_attached(arrival(), InstantMillis(1_102));
    let (encryption, shared, signature) = sign_parts(&owed);
    let mut buf = [0u8; BROADCAST_MTU];
    assert_eq!(
        responder.write_owed_link_proof_with_parts(
            &owed,
            &encryption,
            &shared,
            &signature,
            interfaces,
            &mut buf
        ),
        Err(WriteLinkProofError::InterfaceUnavailable)
    );
    responder.resume_link_proof_sign(owed, encryption, shared, signature, interfaces, &mut |_| {
        panic!("late proof must neither send nor recreate a handshake")
    });
    assert!(!responder.links.has_local_link(&dispatch.link_id));
    assert_eq!(responder.link_deadlines_wake(), WakeSchedule::Idle);
    // New crypto prepared in the current generation still completes normally.
    let mut fresh_responder = personal_node_announcer();
    let fresh = take_sign_work(
        &mut fresh_responder,
        &request[..dispatch.wire_bytes],
        interfaces,
    );
    assert_eq!(fresh.source_generation, 1);
    let (encryption, shared, signature) = sign_parts(&fresh);
    let mut sent = 0;
    fresh_responder.resume_link_proof_sign(
        fresh,
        encryption,
        shared,
        signature,
        interfaces,
        &mut |event| {
            if matches!(event, EngineReaction::Directive(Directive::Send { .. })) {
                sent += 1;
            }
        },
    );
    assert_eq!(sent, 1);
    assert!(fresh_responder.links.has_local_link(&dispatch.link_id));
}

#[test]
fn administrative_off_settles_pending_once_and_late_verified_proof_cannot_activate() {
    let mut initiator = neighbor_with_a_route();
    let descriptors = arrival_interfaces();
    let interfaces = AttachedInterfaces::new(&descriptors);
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            interfaces,
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let owed = take_sign_work(&mut responder, &request[..dispatch.wire_bytes], interfaces);
    let (encryption, shared, signature) = sign_parts(&owed);
    let mut proof = std::vec::Vec::new();
    responder.resume_link_proof_sign(
        owed,
        encryption,
        shared,
        signature,
        interfaces,
        &mut |event| {
            if let EngineReaction::Directive(Directive::Send { bytes, .. }) = event {
                proof.extend_from_slice(bytes);
            }
        },
    );
    let mut verify = None;
    initiator.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(1_250),
            source_interface: arrival(),
            bytes: &mut proof,
        },
        IngestIo {
            interfaces,
            now: InstantMillis(1_250),
            fill_random: &mut test_fill_entropy,
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |event| {
                if let EngineReaction::Directive(Directive::Fulfill(OwedWork::Crypto(
                    CryptoOwed::LinkProofVerify(owed),
                ))) = event
                {
                    verify = Some(owed);
                }
            },
        },
    );
    let owed = verify.expect("actual delayed proof verification");
    assert!(crate::routing::links::handshake::link_proof_signature_valid(&owed));
    let shared = x25519_diffie_hellman(&owed.initiator_secret, &owed.responder_encryption);
    let mut settlements = std::vec::Vec::new();
    initiator.interface_unavailable(arrival(), InstantMillis(1_251), interfaces, &mut |event| {
        if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) = event {
            settlements.push((id, settlement));
        }
    });
    assert_eq!(
        settlements,
        [(
            CommandId(7),
            Settlement::EstablishLink(Err(EstablishLinkFailure::Rejected(
                EstablishLinkRejection::NoRouteToDestination
            )))
        )]
    );
    initiator.interface_attached(arrival(), InstantMillis(1_252));
    initiator.resume_link_proof(
        owed,
        shared,
        interfaces,
        InstantMillis(1_253),
        &mut test_fill_entropy,
        &mut |event| {
            assert!(!matches!(
                event,
                EngineReaction::Directive(Directive::Send { .. })
                    | EngineReaction::Journaled(Journaled::CommandSettled { .. })
            ));
        },
    );
    assert!(!initiator.links.has_local_link(&dispatch.link_id));
    assert_eq!(initiator.link_deadlines_wake(), WakeSchedule::Idle);
}

#[test]
fn destination_retirement_fences_deferred_proof_across_resume_without_fencing_other_destinations() {
    let mut initiator = neighbor_with_a_route();
    let descriptors = arrival_interfaces();
    let interfaces = AttachedInterfaces::new(&descriptors);
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(911),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            interfaces,
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let owed = take_sign_work(&mut responder, &request[..dispatch.wire_bytes], interfaces);
    let generation = owed.destination_generation;
    let destination = owed.request.destination;
    let identity = owed.identity;
    // Registering another service must not invalidate this destination's crypto.
    let other = responder
        .register_single_destination(
            &identity,
            "unrelated",
            &["service"],
            b"",
            ProofStrategy::ProveNone,
            LinkRequestPolicy::AcceptAll,
            crate::crypto::ratchets::RatchetPolicy::NoRatchets,
        )
        .unwrap();
    let _ =
        responder.retire_destination(&other, interfaces, &mut |bytes| bytes.fill(1), &mut |_| {});
    assert_eq!(
        responder
            .upstream_app_destinations
            .lookup_single(&destination)
            .unwrap()
            .registration_generation,
        generation
    );
    let (encryption, shared, signature) = sign_parts(&owed);
    let mut output = [0; BROADCAST_MTU];
    assert!(
        responder
            .write_owed_link_proof_with_parts(
                &owed,
                &encryption,
                &shared,
                &signature,
                interfaces,
                &mut output
            )
            .is_ok(),
        "another destination's retirement must preserve this proof"
    );
    let _ = responder.retire_destination(
        &destination,
        interfaces,
        &mut |bytes| bytes.fill(1),
        &mut |_| {},
    );
    let replacement = responder
        .register_single_destination(
            &identity,
            "personal",
            &["node"],
            b"",
            ProofStrategy::ProveNone,
            LinkRequestPolicy::AcceptAll,
            crate::crypto::ratchets::RatchetPolicy::NoRatchets,
        )
        .unwrap();
    assert_eq!(replacement, destination);
    assert_ne!(
        responder
            .upstream_app_destinations
            .lookup_single(&destination)
            .unwrap()
            .registration_generation,
        generation
    );
    let (encryption, shared, signature) = sign_parts(&owed);
    let mut output = [0; BROADCAST_MTU];
    assert_eq!(
        responder.write_owed_link_proof_with_parts(
            &owed,
            &encryption,
            &shared,
            &signature,
            interfaces,
            &mut output
        ),
        Err(WriteLinkProofError::DestinationRetired)
    );
    responder.resume_link_proof_sign(owed, encryption, shared, signature, interfaces, &mut |_| {
        panic!("retired generation must not recreate a responder")
    });
    assert!(!responder.links.has_local_link(&dispatch.link_id));
}

#[test]
fn staged_service_refuses_link_handshakes() {
    let mut initiator = neighbor_with_a_route();
    let descriptors = arrival_interfaces();
    let interfaces = AttachedInterfaces::new(&descriptors);
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(912),
            &establish(),
            InstantMillis(1000),
            vector_establish_entropy(),
            interfaces,
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    assert!(responder.set_single_ingress_enabled(
        &crate::engine::test_support::personal_node_destination(),
        false
    ));
    assert_eq!(
        responder.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(1100),
                source_interface: arrival(),
                bytes: &mut request[..dispatch.wire_bytes]
            },
            interfaces
        ),
        IngestPacketOutcome::Ignored(IgnoreReason::LinkRequestsRefused)
    );
    assert!(!responder.links.has_local_link(&dispatch.link_id));
}
