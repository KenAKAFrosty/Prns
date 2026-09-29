use super::tests_support::*;
use crate::engine::test_support::TestStorageLayout;
use crate::engine::{
    CommandId, DeliveryEvidence, EngineReaction, EngineState, InstantMillis, Journaled, NoOwedWork,
    PacketReceiptDelivered, ResourceDecompressionCompleted, SendRequestFailure, Settlement,
};
use crate::routing::links::request::{write_response_plaintext, RequestId, RESPONSE_WIRE_OVERHEAD};
use crate::routing::links::resources::table::IncomingResourceStatus;
use crate::routing::links::resources::{ResourceFailureCause, ResourceSegment};
use crate::units::{ByteLimit, RttMillis};
use crate::wire::{WireContext, WirePacketHeader};
use proptest::prelude::*;

#[derive(Clone, Copy)]
enum Envelope {
    Canonical,
    Legacy,
    WrongRequest,
}

#[derive(Clone, Copy)]
enum Expected {
    Value,
    RefusedAdvertisement,
    RefusedBody,
    CorruptTransfer,
}

fn encoded(request: RequestId, body: &[u8], envelope: Envelope) -> std::vec::Vec<u8> {
    let id = match envelope {
        Envelope::Legacy => return body.to_vec(),
        Envelope::Canonical => request,
        Envelope::WrongRequest => RequestId([0xF3; 16]),
    };
    let mut bytes = std::vec![0; RESPONSE_WIRE_OVERHEAD + core::cmp::max(body.len(), 1)];
    let length = write_response_plaintext(&id, body, &mut bytes).unwrap();
    bytes.truncate(length);
    bytes
}

fn assert_retired(receiver: &EngineState<TestStorageLayout>, request: RequestId) {
    assert!(!receiver.receipts.has_pending_request(&link_id(), request));
    assert!(receiver.incoming_resources.is_empty());
    assert!(receiver.pending_resource_offers.is_empty());
}

fn expected_settlement(expected: Expected, rtt: u64) -> Settlement {
    Settlement::SendRequest(match expected {
        Expected::Value => Ok(PacketReceiptDelivered {
            rtt: RttMillis::new(rtt),
            evidence: DeliveryEvidence::Response,
        }),
        Expected::RefusedAdvertisement | Expected::RefusedBody => {
            Err(SendRequestFailure::ResponseTooLarge)
        }
        Expected::CorruptTransfer => Err(SendRequestFailure::ResponseTransferFailed(
            ResourceFailureCause::TransferCorrupt,
        )),
    })
}

fn check_response(body: &[u8], envelope: Envelope, maximum: ByteLimit, expected: Expected) {
    let mut receiver = engine_with_active_link();
    let request =
        track_pending_request_with_limit(&mut receiver, CommandId(42), 1_800, 20_000, maximum);
    let stream = encoded(request, body, envelope);
    let mut sender = engine_with_active_link();
    let advertisement = advertise_response_segment_from(
        &mut sender,
        CommandId(7),
        request,
        &stream,
        None,
        ResourceSegment::whole(stream.len() as u64),
        1_500,
    );
    let pull = feed(&mut receiver, &advertisement, 2_000);
    assert!(pull.responses.is_empty());
    assert_eq!(pull.frames.len(), 1);
    let (header, _) = WirePacketHeader::parse(&pull.frames[0].1).unwrap();
    if let Expected::RefusedAdvertisement = expected {
        assert_eq!(header.context, WireContext::ResourceReceiverCancel);
        assert_eq!(
            pull.settlements,
            std::vec![(CommandId(42), expected_settlement(expected, 200))]
        );
        assert_retired(&receiver, request);
        return;
    }
    assert_eq!(header.context, WireContext::ResourceRequest);
    assert!(pull.settlements.is_empty());
    assert!(!receiver.incoming_resources.is_empty());
    let served = feed(&mut sender, &pull.frames[0].1, 2_100);
    assert!(!served.frames.is_empty());
    let mut responses = std::vec::Vec::new();
    let mut settlements = std::vec::Vec::new();
    for (_, part) in &served.frames {
        let capture = feed(&mut receiver, part, 2_200);
        assert!(capture.received.is_empty());
        responses.extend(capture.responses);
        settlements.extend(capture.settlements);
    }
    let value = match (envelope, body.is_empty()) {
        (Envelope::Canonical, true) => &[0xC0][..],
        _ => body,
    };
    let expected_responses = match expected {
        Expected::Value => std::vec![(CommandId(42), request, value.to_vec())],
        Expected::RefusedBody | Expected::CorruptTransfer => std::vec![],
        Expected::RefusedAdvertisement => unreachable!(),
    };
    assert_eq!(
        (responses, settlements),
        (
            expected_responses,
            std::vec![(CommandId(42), expected_settlement(expected, 400))]
        ),
    );
    assert_retired(&receiver, request);
    for (_, part) in &served.frames {
        let replay = feed(&mut receiver, part, 2_300);
        assert_eq!(
            (replay.responses, replay.settlements),
            (std::vec![], std::vec![])
        );
    }
}

#[test]
fn whole_responses_enforce_encoded_value_limits_for_canonical_and_legacy_bodies() {
    let bodies: &[&[u8]] = &[
        &[],
        b"x",
        b"answer",
        &[0xC0],
        &[0xC4, 0],
        &[0xC4, 2, 0x42, 0x43],
        &[0xC5, 0, 2, 0x42, 0x43],
        &[0xC6, 0, 0, 0, 2, 0x42, 0x43],
        b"\x92not-a-valid-response-envelope",
        &[0xA7; 1_200],
    ];
    for body in bodies {
        for envelope in [Envelope::Canonical, Envelope::Legacy] {
            let length = match envelope {
                Envelope::Canonical => core::cmp::max(body.len(), 1),
                Envelope::Legacy => body.len(),
                Envelope::WrongRequest => unreachable!(),
            } as u64;
            for maximum in [
                ByteLimit::Maximum(length),
                ByteLimit::Maximum(u64::MAX),
                ByteLimit::Unlimited,
            ] {
                check_response(body, envelope, maximum, Expected::Value);
            }
            if length > 0 {
                let expected = match envelope {
                    Envelope::Canonical => Expected::RefusedAdvertisement,
                    Envelope::Legacy => Expected::RefusedBody,
                    Envelope::WrongRequest => unreachable!(),
                };
                check_response(body, envelope, ByteLimit::Maximum(length - 1), expected);
            }
        }
    }
    check_response(
        &[0xA7; 1_200],
        Envelope::Legacy,
        ByteLimit::Maximum(0),
        Expected::RefusedAdvertisement,
    );
}

#[test]
fn a_verified_response_enclosing_a_different_request_is_a_terminal_failure() {
    check_response(
        b"answer",
        Envelope::WrongRequest,
        ByteLimit::Unlimited,
        Expected::CorruptTransfer,
    );
}

#[test]
fn compressed_whole_responses_are_limited_after_inflation() {
    // Python bz2.compress of the canonical envelope and CASE1 plaintext.
    const CANONICAL_BZ2: &str = "425a683931415926535931289c1b0002aeffa48008c42042000200020010001e7f9e0010500c000080200090318002600026052a513434d346834f530d4fd53be3fb7ebe3c84fc139d47513689b387ae7cb97613409a09a09a09e013b84c09d44e9d84fd13509e827c89b04da274c551f41302604e026f135135dd9ce719ff17724538509031289c1b";
    let body = b"reticulum resources ride the link ".repeat(40);
    for (envelope, fixture, overflow) in [
        (
            Envelope::Canonical,
            CANONICAL_BZ2,
            Expected::RefusedAdvertisement,
        ),
        (Envelope::Legacy, CASE1_BZ2, Expected::RefusedBody),
    ] {
        let compressed = bytes_from_hex(fixture);
        for (maximum, expected) in [
            (body.len() as u64, Expected::Value),
            (body.len() as u64 - 1, overflow),
        ] {
            let mut receiver = engine_with_active_link();
            let request = track_pending_request_with_limit(
                &mut receiver,
                CommandId(42),
                1_800,
                20_000,
                ByteLimit::Maximum(maximum),
            );
            let stream = encoded(request, &body, envelope);
            let mut sender = engine_with_active_link();
            let advertisement = advertise_response_segment_from(
                &mut sender,
                CommandId(7),
                request,
                &stream,
                Some(&compressed),
                ResourceSegment::whole(stream.len() as u64),
                1_500,
            );
            let pull = feed(&mut receiver, &advertisement, 2_000);
            if let Expected::RefusedAdvertisement = expected {
                assert_eq!(
                    pull.settlements,
                    std::vec![(CommandId(42), expected_settlement(expected, 200))]
                );
                assert!(pull.responses.is_empty());
                assert_retired(&receiver, request);
                continue;
            }
            assert!(pull.settlements.is_empty());
            assert_eq!(pull.frames.len(), 1);
            let served = feed(&mut sender, &pull.frames[0].1, 2_100);
            assert_eq!(served.frames.len(), 1);
            let capture = feed(&mut receiver, &served.frames[0].1, 2_200);
            assert_eq!(
                (capture.responses, capture.settlements),
                (std::vec![], std::vec![])
            );
            let hash = receiver
                .incoming_resources
                .first_hash_for_link(&link_id())
                .unwrap();
            let index = receiver
                .incoming_resources
                .lookup(&link_id(), &hash)
                .unwrap();
            assert_eq!(
                receiver.incoming_resources.state(index).status,
                IncomingResourceStatus::AwaitingDecompression
            );
            let mut responses = std::vec::Vec::new();
            let mut settlements = std::vec::Vec::new();
            receiver.resume_resource_decompression(
                ResourceDecompressionCompleted {
                    link_id: link_id(),
                    hash,
                    plaintext: &stream,
                },
                InstantMillis(2_400),
                &mut |bytes| bytes.fill(0xC9),
                &mut |reaction: EngineReaction<'_, NoOwedWork>| match reaction {
                    EngineReaction::Journaled(Journaled::ResponseReceived {
                        command_id,
                        request_id,
                        data,
                        ..
                    }) => {
                        responses.push((command_id, request_id, data.to_vec()));
                    }
                    EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                        settlements.push((id, settlement))
                    }
                    _ => {}
                },
            );
            let expected_responses = match expected {
                Expected::Value => std::vec![(CommandId(42), request, body.clone())],
                Expected::RefusedBody => std::vec![],
                _ => unreachable!(),
            };
            assert_eq!(
                (responses, settlements),
                (
                    expected_responses,
                    std::vec![(CommandId(42), expected_settlement(expected, 600))]
                ),
            );
            assert_retired(&receiver, request);
        }
    }
}

#[test]
fn whole_uncompressed_responses_refuse_a_false_advertised_stream_length() {
    let body = [0xA7; 128];
    for declared in [0, body.len() as u64 - 1, body.len() as u64 + 1] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request_with_limit(
            &mut receiver,
            CommandId(42),
            1_800,
            20_000,
            ByteLimit::Unlimited,
        );
        let mut sender = engine_with_active_link();
        let advertisement = advertise_response_segment_from(
            &mut sender,
            CommandId(7),
            request,
            &body,
            None,
            ResourceSegment::whole(body.len() as u64),
            1_500,
        );
        let advertisement = rewrite_advertisement(&advertisement, |advertisement| {
            advertisement.data_bytes = declared;
        });
        let pull = feed(&mut receiver, &advertisement, 2_000);
        assert_eq!(pull.frames.len(), 1);
        assert!(pull.settlements.is_empty());
        let served = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(served.frames.len(), 1);
        let capture = feed(&mut receiver, &served.frames[0].1, 2_200);
        assert_eq!(
            (capture.responses, capture.settlements),
            (
                std::vec![],
                std::vec![(
                    CommandId(42),
                    expected_settlement(Expected::CorruptTransfer, 400)
                )]
            ),
        );
        assert_retired(&receiver, request);
    }
}

#[test]
fn split_advertisements_defer_value_budgets_until_verification() {
    for (has_metadata, total_segments) in [(false, 2), (true, 2)] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request_with_limit(
            &mut receiver,
            CommandId(42),
            1_800,
            20_000,
            ByteLimit::Maximum(6),
        );
        let body = encoded(request, b"answer", Envelope::Canonical);
        let mut sender = engine_with_active_link();
        let advertisement = advertise_response_segment_from(
            &mut sender,
            CommandId(7),
            request,
            &body,
            None,
            ResourceSegment::whole(body.len() as u64),
            1_500,
        );
        let advertisement = rewrite_advertisement(&advertisement, |advertisement| {
            advertisement.flags.has_metadata = has_metadata;
            advertisement.flags.split = total_segments > 1;
            advertisement.total_segments = total_segments;
        });
        let mut capture = feed(&mut receiver, &advertisement, 2_000);
        assert_eq!(capture.frames.len(), 1);
        capture.frames.clear();
        assert_eq!(capture, InboundCapture::default());
        assert!(receiver.receipts.has_pending_request(&link_id(), request));
        assert_eq!(receiver.incoming_resources.len(), 1);
    }
}

proptest! {
    #[test]
    fn canonical_whole_responses_obey_the_exact_value_budget(
        body in proptest::collection::vec(any::<u8>(), 0..=256),
        maximum in 0u64..=257,
    ) {
        let retained = core::cmp::max(body.len(), 1) as u64;
        let expected = if retained <= maximum {
            Expected::Value
        } else {
            Expected::RefusedAdvertisement
        };
        check_response(&body, Envelope::Canonical, ByteLimit::Maximum(maximum), expected);
    }
}
