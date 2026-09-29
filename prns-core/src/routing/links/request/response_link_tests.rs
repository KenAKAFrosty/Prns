use super::*;
use crate::crypto::Ed25519PublicKey;
use crate::engine::test_support::TestStorageLayout;
use crate::engine::{DeliveryEvidence, EngineState, PacketReceiptDelivered, Settlement};
use crate::identity::{IdentityHash, IdentitySigningPublicKey};
use crate::routing::delivery::receipts::{OutstandingReceipt, ReceiptKind};
use crate::routing::links::data::write_link_packet;
use crate::routing::links::resources::receive::tests_support::{
    advertise_response_segment_from, engine_with_active_link, feed, lane, link_id, link_key,
    rewrite_advertisement, InboundCapture, RESPONDER_DESTINATION,
};
use crate::routing::links::resources::{ResourceFailureCause, ResourceSegment};
use crate::routing::links::table::RespondingLink;
use crate::units::ByteLimit;

const OWNER: LinkId = LinkId::new([0x61; 16]);
const COMMAND: CommandId = CommandId(42);
const REQUEST_HASH: PacketHash = PacketHash::new([0x62; 32]);
const REQUEST_DEADLINE: InstantMillis = InstantMillis(20_000);

fn owner_key() -> LinkKey {
    use crate::crypto::{x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};
    let shared = x25519_diffie_hellman(
        &X25519SecretKey::new([0x63; 32]),
        &X25519PublicKey([0x64; 32]),
    );
    LinkKey::derive(&OWNER, &shared)
}

fn requester() -> EngineState<TestStorageLayout> {
    let mut receiver = engine_with_active_link();
    receiver
        .links
        .track_responding(RespondingLink {
            link_id: OWNER,
            key: owner_key(),
            requested_at: InstantMillis(500),
            timeout_at: InstantMillis(5_000),
            mtu: BROADCAST_MTU,
            initiator_signing: Ed25519PublicKey([0x64; 32]),
            destination: RESPONDER_DESTINATION,
            identity: IdentityHash::new([0x65; 16]),
            proof_strategy: crate::routing::upstream_app_destinations::ProofStrategy::ProveNone,
        })
        .unwrap();
    receiver
        .links
        .activate_responding(&OWNER, RttMillis::new(250), lane(), InstantMillis(1_000))
        .unwrap();
    assert_eq!(
        receiver.receipts.track(OutstandingReceipt {
            packet_hash: REQUEST_HASH,
            command_id: COMMAND,
            kind: ReceiptKind::request(
                OWNER,
                ByteLimit::Maximum(32),
                crate::engine::SendRequestIntent::Application
            ),
            peer_signing_key: IdentitySigningPublicKey::new(Ed25519PublicKey([0x64; 32])),
            sent_at: InstantMillis(1_800),
            timeout_at: REQUEST_DEADLINE,
        }),
        None
    );
    receiver
}

fn packet(link: LinkId, key: &LinkKey, body: &[u8]) -> std::vec::Vec<u8> {
    let mut plaintext = [0; WRAPPED_PLAINTEXT_CAP];
    let len = write_response_plaintext(&RequestId::of_packet(&REQUEST_HASH), body, &mut plaintext)
        .unwrap();
    let mut frame = [0; BROADCAST_MTU];
    let len = write_link_packet(
        &link,
        key,
        BROADCAST_MTU,
        WireContext::Response,
        &plaintext[..len],
        &[0x66; 16],
        &mut frame,
    )
    .unwrap();
    frame[..len].to_vec()
}

fn assert_owner_can_still_complete(receiver: &mut EngineState<TestStorageLayout>) {
    let request = RequestId::of_packet(&REQUEST_HASH);
    assert_eq!(
        receiver.receipts.pending_request_deadline(&OWNER, request),
        Some(REQUEST_DEADLINE)
    );
    assert!(receiver.incoming_resources.is_empty());
    assert!(receiver.pending_resource_offers.is_empty());
    assert_eq!(receiver.incoming_assemblies.original_hash(&link_id()), None);
    assert_eq!(
        feed(receiver, &packet(OWNER, &owner_key(), b"ok"), 2_100),
        InboundCapture {
            responses: std::vec![(COMMAND, request, b"ok".to_vec())],
            settlements: std::vec![(
                COMMAND,
                Settlement::SendRequest(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(300),
                    evidence: DeliveryEvidence::Response,
                }))
            )],
            ..InboundCapture::default()
        }
    );
    assert!(!receiver.receipts.has_pending_request(&OWNER, request));
}

#[test]
fn a_packet_response_on_another_authenticated_link_cannot_deliver_or_fail_the_request() {
    for body in [b"wrong".as_slice(), &[0x67; 33]] {
        let mut receiver = requester();
        assert_eq!(
            feed(&mut receiver, &packet(link_id(), &link_key(), body), 2_000),
            InboundCapture::default()
        );
        assert_owner_can_still_complete(&mut receiver);
    }
}

#[test]
fn resource_offers_on_another_link_cannot_allocate_claim_or_refuse_the_request() {
    for segments in [1, 2] {
        for advertised_bytes in [32, 52] {
            let mut receiver = requester();
            let frame = advertise_response_segment_from(
                &mut engine_with_active_link(),
                CommandId(7),
                RequestId::of_packet(&REQUEST_HASH),
                &[0x68; 16],
                None,
                ResourceSegment {
                    index: 1,
                    total_segments: segments,
                    total_data_bytes: 32,
                },
                1_900,
            );
            let frame = rewrite_advertisement(&frame, |ad| ad.data_bytes = advertised_bytes);
            assert_eq!(
                feed(&mut receiver, &frame, 2_000),
                InboundCapture::default()
            );
            assert_owner_can_still_complete(&mut receiver);
        }
    }
}

#[test]
fn late_resource_failure_cannot_settle_an_identically_named_request_on_another_link() {
    let mut receiver = requester();
    let request = RequestId::of_packet(&REQUEST_HASH);
    let other_command = CommandId(43);
    assert_eq!(
        receiver.receipts.track(OutstandingReceipt {
            packet_hash: REQUEST_HASH,
            command_id: other_command,
            kind: ReceiptKind::request(
                link_id(),
                ByteLimit::Maximum(32),
                crate::engine::SendRequestIntent::Application
            ),
            peer_signing_key: IdentitySigningPublicKey::new(Ed25519PublicKey([0x99; 32])),
            sent_at: InstantMillis(1_800),
            timeout_at: REQUEST_DEADLINE,
        }),
        None
    );
    let advertisement = advertise_response_segment_from(
        &mut engine_with_active_link(),
        CommandId(7),
        request,
        &[0x68; 16],
        None,
        ResourceSegment {
            index: 1,
            total_segments: 2,
            total_data_bytes: 32,
        },
        1_900,
    );
    let pull = feed(&mut receiver, &advertisement, 2_000);
    assert_eq!(pull.frames.len(), 1);
    let hash = receiver
        .incoming_resources
        .first_hash_for_link(&link_id())
        .unwrap();
    // Model receipt retirement while the Resource is still in flight. The
    // identically named request on OWNER must survive the later cancellation.
    assert_eq!(
        receiver
            .receipts
            .take_request_for_command(&link_id(), other_command, request),
        Some(crate::routing::delivery::receipts::ProvenRequestReceipt {
            command_id: other_command,
            intent: crate::engine::SendRequestIntent::Application,
            sent_at: InstantMillis(1_800),
        })
    );
    let mut cancel = [0; BROADCAST_MTU];
    let length = write_link_packet(
        &link_id(),
        &link_key(),
        BROADCAST_MTU,
        WireContext::ResourceInitiatorCancel,
        hash.as_bytes(),
        &[0x69; 16],
        &mut cancel,
    )
    .unwrap();
    assert_eq!(
        feed(&mut receiver, &cancel[..length], 2_050),
        InboundCapture {
            failed: std::vec![(hash, ResourceFailureCause::CancelledBySender)],
            ..InboundCapture::default()
        }
    );
    assert_owner_can_still_complete(&mut receiver);
}
