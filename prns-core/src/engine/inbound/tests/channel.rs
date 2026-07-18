use super::super::*;
use crate::crypto::{
    ed25519_public_key, ed25519_verify, x25519_diffie_hellman, Ed25519PublicKey, Ed25519SecretKey,
    Ed25519Signature, X25519PublicKey, X25519SecretKey,
};
use crate::engine::test_support::{transporting_interfaces, TestStorageLayout};
use crate::engine::CommandId;
use crate::engine::{Directive, EngineReaction};
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::links::channel::{write_envelope, ChannelSequence, MessageType};
use crate::routing::links::data::write_link_packet;
use crate::routing::links::table::InitiatedLink;
use crate::routing::links::{LinkId, LinkKey};
use crate::routing::proof::LINK_PROOF_WIRE_LEN;
use crate::wire::{
    DestinationHash, DestinationType, PacketType, WireContext, BROADCAST_MTU, HEADER_MIN_LEN,
};
use std::vec::Vec;

const LANE: [u8; 8] = [0xEE; 8];

fn shared() -> crate::crypto::X25519SharedSecret {
    x25519_diffie_hellman(
        &X25519SecretKey::new([0x33; 32]),
        &X25519PublicKey([0x44; 32]),
    )
}

fn active_initiator() -> (
    EngineState<TestStorageLayout>,
    LinkId,
    LinkKey,
    Ed25519PublicKey,
) {
    let link_id = LinkId::new([0x5C; 16]);
    let link_signing = Ed25519SecretKey::new([0x42; 32]);
    let link_signing_public = ed25519_public_key(&link_signing);
    let mut state = EngineState::<TestStorageLayout>::default();
    state
        .links
        .track_initiated(InitiatedLink {
            link_id,
            destination: DestinationHash::new([0x77; 16]),
            initiator_secret: X25519SecretKey::new([0x33; 32]),
            link_signing,
            requested_at: InstantMillis(0),
            timeout_at: InstantMillis(5_000),
            command_id: CommandId(1),
        })
        .unwrap();
    state
        .links
        .activate_initiated(
            &link_id,
            LinkKey::derive(&link_id, &shared()),
            &LinkActivation {
                rtt: crate::units::RttMillis::new(250),
                mtu: BROADCAST_MTU,
                attached_interface: InterfaceId::new(LANE),
                peer_signing: Ed25519PublicKey([0x99; 32]),
            },
            InstantMillis(1_000),
        )
        .unwrap();
    (
        state,
        link_id,
        LinkKey::derive(&link_id, &shared()),
        link_signing_public,
    )
}

fn channel_frame(
    key: &LinkKey,
    link_id: &LinkId,
    message_type: MessageType,
    sequence: ChannelSequence,
    body: &[u8],
) -> Vec<u8> {
    let mut envelope = [0u8; BROADCAST_MTU];
    let env_len = write_envelope(message_type, sequence, body, &mut envelope).unwrap();
    let mut frame = [0u8; BROADCAST_MTU];
    let len = write_link_packet(
        link_id,
        key,
        BROADCAST_MTU,
        WireContext::Channel,
        &envelope[..env_len],
        &[0u8; 16],
        &mut frame,
    )
    .unwrap();
    frame[..len].to_vec()
}

type FeedOutcome = (Vec<(MessageType, Vec<u8>)>, Option<Vec<u8>>);

fn feed(state: &mut EngineState<TestStorageLayout>, frame: &[u8], now: u64) -> FeedOutcome {
    let mut raw = frame.to_vec();
    let mut messages = Vec::new();
    let mut ack = None;
    state.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(now),
            source_interface: InterfaceId::new(LANE),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&transporting_interfaces()),
            now: InstantMillis(now),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::ChannelMessageReceived {
                    message_type,
                    data,
                    ..
                }) => messages.push((message_type, data.to_vec())),
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    ack = Some(bytes.to_vec())
                }
                _ => {}
            },
        },
    );
    (messages, ack)
}

fn assert_valid_ack(ack: &[u8], ciphertext: &[u8], link_id: &LinkId, signer: &Ed25519PublicKey) {
    assert_eq!(
        ack.len(),
        LINK_PROOF_WIRE_LEN,
        "the ack is one explicit proof"
    );
    let expected = PacketHash::of_fields(
        DestinationType::Link,
        PacketType::Data,
        &link_id.to_address(),
        WireContext::Channel,
        ciphertext,
    );
    assert_eq!(
        &ack[HEADER_MIN_LEN..HEADER_MIN_LEN + PACKET_HASH_LEN],
        expected.as_bytes(),
        "the ack names the packet it proves",
    );
    let signature = Ed25519Signature(
        ack[HEADER_MIN_LEN + PACKET_HASH_LEN..LINK_PROOF_WIRE_LEN]
            .try_into()
            .unwrap(),
    );
    ed25519_verify(signer, expected.as_bytes(), &signature)
        .expect("the ack verifies against the initiator's link signing key");
}

#[test]
fn an_in_order_channel_message_is_journaled_and_unconditionally_acked() {
    let (mut state, link_id, key, signer) = active_initiator();
    let frame = channel_frame(
        &key,
        &link_id,
        MessageType(7),
        ChannelSequence(0),
        b"hello channel",
    );
    let ciphertext = frame[HEADER_MIN_LEN..].to_vec();

    let (messages, ack) = feed(&mut state, &frame, 2_000);
    assert_eq!(
        messages,
        std::vec![(MessageType(7), b"hello channel".to_vec())],
        "the message is delivered to the journal in order",
    );
    assert_valid_ack(
        &ack.expect("a channel arrival owes an ack even when should_prove says no"),
        &ciphertext,
        &link_id,
        &signer,
    );
}

#[test]
fn a_gap_then_its_fill_journals_the_whole_run_in_order() {
    let (mut state, link_id, key, _signer) = active_initiator();

    let ahead = channel_frame(&key, &link_id, MessageType(1), ChannelSequence(1), b"one");
    let (messages, ack) = feed(&mut state, &ahead, 2_000);
    assert!(
        messages.is_empty(),
        "the out-of-order arrival waits for the gap"
    );
    assert!(
        ack.is_some(),
        "but it is still acked so the sender stops resending"
    );

    let gap = channel_frame(&key, &link_id, MessageType(0), ChannelSequence(0), b"zero");
    let (messages, ack) = feed(&mut state, &gap, 2_100);
    assert_eq!(
        messages,
        std::vec![
            (MessageType(0), b"zero".to_vec()),
            (MessageType(1), b"one".to_vec()),
        ],
        "filling the gap drains the buffered run in one arrival, in sequence order",
    );
    assert!(ack.is_some(), "the gap-filling arrival is acked too");
}
