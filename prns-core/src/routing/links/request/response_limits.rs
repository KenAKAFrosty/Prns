use super::*;
use crate::engine::{DeliveryEvidence, PacketReceiptDelivered, SendRequestFailure, Settlement};
use crate::routing::links::data::write_link_packet;
use crate::routing::links::resources::receive::tests_support::{
    engine_with_active_link, feed, link_id, link_key, track_pending_request_with_limit,
};
use crate::units::ByteLimit;
use proptest::prelude::*;

fn frame(request: RequestId, body: &[u8], nonce: u8) -> std::vec::Vec<u8> {
    let mut plaintext = [0; WRAPPED_PLAINTEXT_CAP];
    let length = write_response_plaintext(&request, body, &mut plaintext).unwrap();
    let mut frame = [0; BROADCAST_MTU];
    let length = write_link_packet(
        &link_id(),
        &link_key(),
        BROADCAST_MTU,
        WireContext::Response,
        &plaintext[..length],
        &[nonce; 16],
        &mut frame,
    )
    .unwrap();
    frame[..length].to_vec()
}

fn delivered() -> Settlement {
    Settlement::SendRequest(Ok(PacketReceiptDelivered {
        rtt: RttMillis::new(200),
        evidence: DeliveryEvidence::Response,
    }))
}

fn check_limit(body: &[u8], limit: ByteLimit, accepted: bool) {
    let mut receiver = engine_with_active_link();
    let request =
        track_pending_request_with_limit(&mut receiver, CommandId(42), 1_800, 20_000, limit);
    // Empty input is sent as the one-byte MessagePack nil value, not zero bytes.
    let expected_body = if body.is_empty() { &[NIL] } else { body };
    let capture = feed(&mut receiver, &frame(request, body, 1), 2_000);
    let expected = if accepted {
        (
            std::vec![(CommandId(42), request, expected_body.to_vec())],
            std::vec![(CommandId(42), delivered())],
        )
    } else {
        (
            std::vec![],
            std::vec![(
                CommandId(42),
                Settlement::SendRequest(Err(SendRequestFailure::ResponseTooLarge)),
            )],
        )
    };
    assert_eq!((capture.responses, capture.settlements), expected);
    assert!(!receiver.receipts.has_pending_request(request));

    // A fresh IV bypasses packet deduplication: the retired receipt, not the
    // dedup cache, must prevent another delivery or terminal settlement.
    let replay = feed(&mut receiver, &frame(request, body, 2), 2_001);
    assert_eq!(
        (replay.responses, replay.settlements),
        (std::vec![], std::vec![])
    );

    // Refusal leaves the link and receipt slot usable for another request.
    let next = track_pending_request_with_limit(
        &mut receiver,
        CommandId(43),
        2_100,
        20_000,
        ByteLimit::Maximum(1),
    );
    let capture = feed(&mut receiver, &frame(next, &[0x2A], 3), 2_300);
    assert_eq!(
        (capture.responses, capture.settlements),
        (
            std::vec![(CommandId(43), next, std::vec![0x2A])],
            std::vec![(CommandId(43), delivered())],
        )
    );
    assert!(!receiver.receipts.has_pending_request(next));
}

#[test]
fn packet_limits_count_every_delivered_byte_without_interpreting_the_value() {
    let bodies: &[&[u8]] = &[
        &[],
        &[NIL],
        b"a",
        b"ab",
        b"test",
        &[BIN_8, 0],
        &[BIN_8, 4, b't', b'e', b's', b't'],
        &[BIN_16, 0, 4, b't', b'e', b's', b't'],
        &[BIN_32, 0, 0, 0, 4, b't', b'e', b's', b't'],
        // The value is opaque: truncated, mismatched and trailing binary
        // encodings get neither a header allowance nor implicit decoding.
        &[BIN_8],
        &[BIN_16, 0],
        &[BIN_32, 0, 0, 0],
        &[BIN_8, 2, 0],
        &[BIN_8, 0, 0],
        &[0x5A; MAX_RESPOND_DATA_LEN],
    ];
    for body in bodies {
        let length = core::cmp::max(body.len(), 1) as u64;
        for (limit, accepted) in [
            (ByteLimit::Maximum(0), false),
            (ByteLimit::Maximum(length - 1), false),
            (ByteLimit::Maximum(length), true),
            (ByteLimit::Maximum(length + 1), true),
            (ByteLimit::Maximum(u64::MAX), true),
            (ByteLimit::Unlimited, true),
        ] {
            check_limit(body, limit, accepted);
        }
    }
}

proptest! {
    #[test]
    fn arbitrary_packet_values_obey_the_exact_retained_byte_limit(
        body in proptest::collection::vec(any::<u8>(), 0..=MAX_RESPOND_DATA_LEN),
        maximum in 0u64..=(MAX_RESPOND_DATA_LEN as u64 + 1),
    ) {
        let retained = core::cmp::max(body.len(), 1) as u64;
        check_limit(&body, ByteLimit::Maximum(maximum), retained <= maximum);
    }
}
