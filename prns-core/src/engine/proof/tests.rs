use super::*;
use crate::engine::test_support::*;
use crate::engine::IngestIo;
use crate::engine::{
    Directive, EngineReaction, EngineState, IngestPacketOutcome, Journaled, RatchetPolicy,
};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::{InboundPacket, InterfaceId};
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::Delivery;
use crate::routing::links::table::LinkActivation;
use crate::routing::proof::{
    ProofObligation, ProofRequest, EXPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN,
};
use crate::routing::upstream_app_destinations::LinkRequestPolicy;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::wire::{WirePacketHeader, BROADCAST_MTU, HEADER_MIN_LEN};

fn fulfill_channel_ack(
    state: &mut EngineState<TestStorageLayout>,
    link_id: &crate::routing::links::LinkId,
    packet_hash: &PacketHash,
) -> std::vec::Vec<u8> {
    let owed = state
        .prepare_channel_ack_sign(InterfaceId::new([0xEE; 8]), link_id, packet_hash)
        .expect("an active link owes its channel ACK signature");
    let signature = crate::crypto::ed25519_sign(&owed.signing_secret, owed.packet_hash.as_bytes());
    let mut wire = std::vec::Vec::new();
    let outcome = state.resume_channel_ack_sign(
        ChannelAckSignCompleted {
            target: owed.target,
            link_id: owed.link_id,
            packet_hash: owed.packet_hash,
            signature,
        },
        InstantMillis(2_000),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                wire.extend_from_slice(bytes);
            }
        },
    );
    assert_eq!(outcome, ResumeChannelAckSignOutcome::Sent);
    wire
}

#[test]
fn write_proof_is_byte_identical_to_the_rns_1_4_2_implicit_proof() {
    let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
    let held = state.hold_identity(fixed_secret_key()).unwrap();
    let destination = state
        .register_single_destination(
            &held,
            "personal",
            &["node"],
            b"",
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .unwrap();

    let mut raw = sealed_single_packet(&identity, destination, b"proof-parity");
    assert_eq!(raw, bytes_from_hex(RNS_1_4_2_SEALED_FOR_PROOF));

    let mut proof = std::vec::Vec::new();
    crate::engine::drive_packet_to_quiescence(
        &mut state,
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: InterfaceId::new([0xEE; 8]),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&transporting_interfaces()),
            now: InstantMillis(1_000),
            fill_random: &mut |_| {},
            should_prove: &mut |_| true,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    proof.extend_from_slice(bytes);
                }
            },
        },
    );
    assert_eq!(proof, bytes_from_hex(RNS_1_4_2_IMPLICIT_PROOF).as_slice());
}

#[test]
fn explicit_non_link_proofs_name_the_proven_packet_hash() {
    let mut state: EngineState<TestStorageLayout> = EngineState::default();
    state.set_protocol_policy(crate::engine::EngineProtocolPolicy {
        proof_form: ProofForm::Explicit,
        ..Default::default()
    });
    let packet_hash = PacketHash::new([0xA5; PACKET_HASH_LEN]);
    let signature = Ed25519Signature([0x5A; 64]);
    let mut wire = Vec::new();
    state.resume_proof_sign(
        ProofSignCompleted {
            target: InterfaceId::new([0xB4; 8]),
            packet_hash,
            signature,
        },
        &mut |reaction| match reaction {
            EngineReaction::Directive(Directive::Send { target, bytes }) => {
                assert_eq!(target, InterfaceId::new([0xB4; 8]));
                wire.extend_from_slice(bytes);
            }
            EngineReaction::Directive(Directive::Fulfill(never)) => match never {},
            EngineReaction::Journaled(_) | EngineReaction::Directive(_) => {
                panic!("a completed proof signature emits exactly one send")
            }
        },
    );
    assert_eq!(wire.len(), EXPLICIT_PROOF_WIRE_LEN);
    let (_, payload) = WirePacketHeader::parse(&wire).unwrap();
    assert_eq!(&payload[..PACKET_HASH_LEN], packet_hash.as_bytes());
    assert_eq!(&payload[PACKET_HASH_LEN..], &signature.0);
}

fn prove_if_state() -> (
    EngineState<TestStorageLayout>,
    InMemoryNodeIdentity,
    crate::wire::DestinationHash,
) {
    let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
    let held = state.hold_identity(fixed_secret_key()).unwrap();
    let destination = state
        .register_single_destination(
            &held,
            "personal",
            &["node"],
            b"",
            ProofStrategy::ProveIf,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .unwrap();
    (state, identity, destination)
}

#[test]
fn a_prove_if_delivery_defers_the_proof_to_the_app() {
    let (mut state, identity, destination) = prove_if_state();
    let mut raw = sealed_single_packet(&identity, destination, b"prove-if");
    let IngestPacketOutcome::Delivery {
        delivery: Delivery::Single(single),
        proof: ProofObligation::OwedIfApp(_),
    } = state.ingest_for_test(
        plain_data_packet(&mut raw),
        AttachedInterfaces::new(&transporting_interfaces()),
    )
    else {
        panic!("a ProveIf delivery defers its proof to the app");
    };
    assert_eq!(
        single.plaintext, b"prove-if",
        "the deferred decision sees the decrypted content",
    );
}

fn prove_if_proof_directive(
    decide: impl FnMut(&ProofRequest) -> bool,
) -> (bool, std::vec::Vec<u8>) {
    let (mut state, identity, destination) = prove_if_state();
    let mut raw = sealed_single_packet(&identity, destination, b"prove-if");
    let mut decide = decide;
    let mut seen = std::vec::Vec::new();
    let mut proved = false;
    crate::engine::drive_packet_to_quiescence(
        &mut state,
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: InterfaceId::new([0xEE; 8]),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&transporting_interfaces()),
            now: InstantMillis(1_000),
            fill_random: &mut |bytes| bytes.fill(0),
            should_prove: &mut |request| {
                seen = request.plaintext.to_vec();
                decide(request)
            },
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { .. }) = reaction {
                    proved = true;
                }
            },
        },
    );
    (proved, seen)
}

#[test]
fn the_app_decider_gates_the_prove_if_proof() {
    let (proved, seen) = prove_if_proof_directive(|_| true);
    assert!(
        proved,
        "the decider agreed, so the manifold answers a proof"
    );
    assert_eq!(seen, b"prove-if", "the decider sees the decrypted content");

    let (proved, _) = prove_if_proof_directive(|_| false);
    assert!(!proved, "the decider declined, so no proof goes out");
}

#[test]
fn delivery_is_journaled_before_the_prove_if_decision_and_proof_egress() {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Step {
        Delivered,
        Decided,
        ProofSent,
    }

    let (mut state, identity, destination) = prove_if_state();
    let mut raw = sealed_single_packet(&identity, destination, b"persist-before-proof");
    let steps = core::cell::RefCell::new(std::vec::Vec::new());
    crate::engine::drive_packet_to_quiescence(
        &mut state,
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: InterfaceId::new([0xEE; 8]),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&transporting_interfaces()),
            now: InstantMillis(1_000),
            fill_random: &mut |bytes| bytes.fill(0),
            should_prove: &mut |_| {
                steps.borrow_mut().push(Step::Decided);
                true
            },
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::Delivered(_)) => {
                    steps.borrow_mut().push(Step::Delivered);
                }
                EngineReaction::Directive(Directive::Send { .. }) => {
                    steps.borrow_mut().push(Step::ProofSent);
                }
                _ => {}
            },
        },
    );

    assert_eq!(
        *steps.borrow(),
        [Step::Delivered, Step::Decided, Step::ProofSent],
        "the host can persist delivery before policy or proof egress acknowledges it",
    );
}

#[test]
fn an_initiator_channel_ack_is_signed_by_the_link_key() {
    use crate::crypto::{
        ed25519_public_key, ed25519_verify, x25519_diffie_hellman, Ed25519PublicKey,
        Ed25519SecretKey, X25519PublicKey, X25519SecretKey,
    };
    use crate::engine::CommandId;
    use crate::routing::links::table::InitiatedLink;
    use crate::routing::links::{LinkId, LinkKey};

    let mut state = EngineState::<TestStorageLayout>::default();
    let link_id = LinkId::new([0x5C; 16]);
    let link_signing = Ed25519SecretKey::new([0x42; 32]);
    let link_signing_public = ed25519_public_key(&link_signing);
    state
        .links
        .track_initiated(InitiatedLink {
            link_id,
            destination: DestinationHash::new([0x77; 16]),
            route_evidence: crate::routing::routes::RouteEvidenceHandle::new(
                crate::routing::routes::RouteEvidenceId::FIRST,
                0,
            ),
            expected_hops: 1,
            mode: crate::routing::links::LinkMode::Aes256Cbc,
            initiator_secret: X25519SecretKey::new([0x33; 32]),
            link_signing,
            requested_at: InstantMillis(0),
            timeout_at: InstantMillis(5_000),
            command_id: CommandId(1),
        })
        .unwrap();
    let shared = x25519_diffie_hellman(
        &X25519SecretKey::new([0x33; 32]),
        &X25519PublicKey([0x44; 32]),
    );
    state
        .links
        .activate_initiated(
            &link_id,
            LinkKey::derive(&link_id, &shared),
            &LinkActivation {
                received_hops: 1,
                rtt: crate::units::RttMillis::new(250),
                mtu: BROADCAST_MTU,
                attached_interface: InterfaceId::new([0xEE; 8]),
                peer_signing: Ed25519PublicKey([0x99; 32]),
            },
            InstantMillis(1_000),
        )
        .unwrap();

    let packet_hash = PacketHash::new([0xAB; 32]);
    let buf = fulfill_channel_ack(&mut state, &link_id, &packet_hash);
    assert_eq!(buf.len(), LINK_PROOF_WIRE_LEN);
    assert_eq!(
        &buf[HEADER_MIN_LEN..HEADER_MIN_LEN + PACKET_HASH_LEN],
        packet_hash.as_bytes(),
        "the proof names the packet it acks",
    );
    let signature = Ed25519Signature(
        buf[HEADER_MIN_LEN + PACKET_HASH_LEN..LINK_PROOF_WIRE_LEN]
            .try_into()
            .unwrap(),
    );
    ed25519_verify(&link_signing_public, packet_hash.as_bytes(), &signature)
        .expect("the initiator signs the ack with its own ephemeral link key");
}

#[test]
fn a_responder_channel_ack_is_signed_by_the_held_identity() {
    use crate::crypto::{
        ed25519_verify, x25519_diffie_hellman, Ed25519PublicKey, X25519PublicKey, X25519SecretKey,
    };
    use crate::identity::IdentitySigner;
    use crate::routing::links::table::RespondingLink;
    use crate::routing::links::{LinkId, LinkKey};

    let mut state = EngineState::<TestStorageLayout>::default();
    let identity = state.hold_identity(fixed_secret_key()).unwrap();
    let signer = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
    let signing_public = *signer.signing_public_key().as_ed25519();

    let link_id = LinkId::new([0x6D; 16]);
    let shared = x25519_diffie_hellman(
        &X25519SecretKey::new([0x55; 32]),
        &X25519PublicKey([0x66; 32]),
    );
    state
        .links
        .track_responding(RespondingLink {
            link_id,
            key: LinkKey::derive(&link_id, &shared),
            requested_at: InstantMillis(0),
            timeout_at: InstantMillis(5_000),
            mtu: BROADCAST_MTU,
            initiator_signing: Ed25519PublicKey([0x99; 32]),
            destination: DestinationHash::new([0x77; 16]),
            identity,
            proof_strategy: ProofStrategy::ProveAll,
        })
        .unwrap();
    state
        .links
        .activate_responding(
            &link_id,
            crate::units::RttMillis::new(250),
            InterfaceId::new([0xEE; 8]),
            InstantMillis(1_000),
        )
        .unwrap();

    let packet_hash = PacketHash::new([0xCD; 32]);
    let buf = fulfill_channel_ack(&mut state, &link_id, &packet_hash);
    assert_eq!(buf.len(), LINK_PROOF_WIRE_LEN);
    let signature = Ed25519Signature(
        buf[HEADER_MIN_LEN + PACKET_HASH_LEN..LINK_PROOF_WIRE_LEN]
            .try_into()
            .unwrap(),
    );
    ed25519_verify(&signing_public, packet_hash.as_bytes(), &signature)
        .expect("the responder signs the ack with the destination identity it answers for");
}

#[test]
fn an_inactive_link_does_not_owe_a_channel_ack_signature() {
    use crate::routing::links::LinkId;

    let state = EngineState::<TestStorageLayout>::default();
    assert!(matches!(
        state.prepare_channel_ack_sign(
            InterfaceId::new([0xEE; 8]),
            &LinkId::new([0x01; 16]),
            &PacketHash::new([0u8; 32]),
        ),
        Err(ChannelAckSignUnavailable::LinkNotActive),
    ));
}

#[test]
fn a_channel_ack_completion_for_a_retired_link_emits_nothing() {
    use crate::crypto::Ed25519Signature;
    use crate::routing::links::LinkId;

    let mut state = EngineState::<TestStorageLayout>::default();
    let mut emitted = false;
    let outcome = state.resume_channel_ack_sign(
        ChannelAckSignCompleted {
            target: InterfaceId::new([0xEE; 8]),
            link_id: LinkId::new([0x01; 16]),
            packet_hash: PacketHash::new([0x02; 32]),
            signature: Ed25519Signature([0x03; 64]),
        },
        InstantMillis(2_000),
        &mut |_| emitted = true,
    );

    assert_eq!(outcome, ResumeChannelAckSignOutcome::LinkNoLongerActive);
    assert!(!emitted);
}
