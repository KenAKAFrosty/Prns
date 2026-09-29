use super::tests_support::*;
use crate::engine::{
    CommandId, DeliveryEvidence, EngineReaction, EngineState, InstantMillis, Journaled, NoOwedWork,
    PacketReceiptDelivered, ResourceDecompressionCompleted, SendRequestFailure, Settlement,
};
use crate::routing::links::request::{write_response_plaintext, RequestId, RESPONSE_WIRE_OVERHEAD};
use crate::routing::links::resources::{
    ResourceBody, ResourceFailureCause, ResourceMemoryLimits, ResourceMetadata, ResourceSegment,
    MAX_EFFICIENT_SIZE,
};
use crate::storage::{GrowableHeap, StorageLayout};
use crate::units::{ByteLimit, RttMillis};
use proptest::prelude::*;

fn completion(data: &[u8], limit: ByteLimit, elapsed: u64) -> Settlement {
    let fits = match limit {
        ByteLimit::Unlimited => true,
        ByteLimit::Maximum(maximum) => data.len() as u64 <= maximum,
    };
    Settlement::SendRequest(if fits {
        Ok(PacketReceiptDelivered {
            rtt: RttMillis::new(elapsed),
            evidence: DeliveryEvidence::Response,
        })
    } else {
        Err(SendRequestFailure::ResponseTooLarge)
    })
}

fn check_file(body: ResourceBody<'_>, limit: ByteLimit) {
    let mut receiver = engine_with_active_link();
    let request =
        track_pending_request_with_limit(&mut receiver, CommandId(42), 1_800, 20_000, limit);
    let mut sender = engine_with_active_link();
    let advertisement = advertise_response_body_from(
        &mut sender,
        CommandId(7),
        request,
        body,
        ResourceSegment::whole(body.data.len() as u64),
        1_500,
    );
    let pull = feed(&mut receiver, &advertisement, 2_000);
    assert_eq!(pull.frames.len(), 1);
    assert!(pull.settlements.is_empty());
    let served = feed(&mut sender, &pull.frames[0].1, 2_100);
    assert!(!served.frames.is_empty());
    let mut responses = std::vec::Vec::new();
    let mut settlements = std::vec::Vec::new();
    for (_, frame) in &served.frames {
        let capture = feed(&mut receiver, frame, 2_200);
        assert!(capture.received.is_empty());
        responses.extend(capture.responses);
        settlements.extend(capture.settlements);
    }
    let elapsed = if body.compressed_candidate.is_some() {
        assert!(responses.is_empty() && settlements.is_empty());
        let hash = receiver
            .incoming_resources
            .first_hash_for_link(&link_id())
            .unwrap();
        let ResourceMetadata::Packed(packed) = body.metadata else {
            panic!("a whole file carries its metadata");
        };
        let mut plaintext = metadata_block(packed);
        plaintext.extend_from_slice(body.data);
        receiver.resume_resource_decompression(
            ResourceDecompressionCompleted {
                link_id: link_id(),
                hash,
                plaintext: &plaintext,
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
                EngineReaction::Journaled(Journaled::ResourceReceived { .. }) => {
                    panic!("a response is not an unsolicited resource")
                }
                _ => {}
            },
        );
        600
    } else {
        400
    };
    let expected = completion(body.data, limit, elapsed);
    let expected_responses = match expected {
        Settlement::SendRequest(Ok(_)) => std::vec![(CommandId(42), request, body.data.to_vec())],
        Settlement::SendRequest(Err(SendRequestFailure::ResponseTooLarge)) => std::vec![],
        _ => unreachable!(),
    };
    assert_eq!(
        (responses, settlements),
        (expected_responses, std::vec![(CommandId(42), expected)])
    );
    assert!(!receiver.receipts.has_pending_request(&link_id(), request));
    assert!(receiver.incoming_resources.is_empty());
    assert!(receiver.pending_resource_offers.is_empty());
    for (_, frame) in &served.frames {
        let replay = feed(&mut receiver, frame, 2_500);
        assert_eq!(
            (replay.responses, replay.settlements),
            (std::vec![], std::vec![])
        );
    }
}

#[test]
fn metadata_is_not_charged_to_the_whole_file_response_budget() {
    let values: &[&[u8]] = &[
        &[],
        &[0xC0],
        &[0xC4, 0],
        &[0xC5, 0, 1, 0x42],
        &[0xC6, 0, 0, 0, 1, 0x42],
        b"\x92not-an-envelope",
        &[0xA7; 128],
    ];
    for packed in [&[][..], b"opaque metadata", &[0xAA; 512]] {
        for data in values {
            for limit in [
                ByteLimit::Maximum(0),
                ByteLimit::Maximum(data.len().saturating_sub(1) as u64),
                ByteLimit::Maximum(data.len() as u64),
                ByteLimit::Maximum(u64::MAX),
                ByteLimit::Unlimited,
            ] {
                check_file(
                    ResourceBody {
                        data,
                        compressed_candidate: None,
                        metadata: ResourceMetadata::Packed(packed),
                    },
                    limit,
                );
            }
        }
    }
}

fn envelope_shaped_file(request: RequestId) -> std::vec::Vec<u8> {
    let mut data = std::vec![0; RESPONSE_WIRE_OVERHEAD + 4];
    let length = write_response_plaintext(&request, b"file", &mut data).unwrap();
    assert_eq!(length, data.len());
    data
}

#[test]
fn envelope_shaped_files_are_neither_unwrapped_nor_rejected_as_wrong_requests() {
    let request =
        track_pending_request(&mut engine_with_active_link(), CommandId(42), 1_800, 20_000);
    for enclosed in [request, RequestId([0xF3; 16])] {
        let data = envelope_shaped_file(enclosed);
        for limit in [
            ByteLimit::Unlimited,
            ByteLimit::Maximum(data.len() as u64),
            ByteLimit::Maximum(data.len() as u64 - 1),
        ] {
            check_file(
                ResourceBody {
                    data: &data,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::Packed(&[]),
                },
                limit,
            );
        }
    }
}

#[test]
fn compressed_metadata_responses_count_only_the_inflated_file_bytes() {
    let data = b"reticulum resources ride the link ".repeat(40);
    let packed = bytes_from_hex(META_PACKED);
    let compressed = bytes_from_hex(META_CASE1_BZ2);
    for maximum in [data.len() as u64 - 1, data.len() as u64] {
        check_file(
            ResourceBody {
                data: &data,
                compressed_candidate: Some(&compressed),
                metadata: ResourceMetadata::Packed(&packed),
            },
            ByteLimit::Maximum(maximum),
        );
    }
}

fn assert_capacity_refusal<S: StorageLayout>(mut receiver: EngineState<S>, metadata_bytes: usize) {
    let request = track_pending_request_with_limit(
        &mut receiver,
        CommandId(42),
        1_800,
        20_000,
        ByteLimit::Maximum(0),
    );
    let mut sender = active_engine::<GrowableHeap>();
    let packed = std::vec![0xAA; metadata_bytes];
    let advertisement = advertise_response_body_from(
        &mut sender,
        CommandId(7),
        request,
        ResourceBody {
            data: &[],
            compressed_candidate: None,
            metadata: ResourceMetadata::Packed(&packed),
        },
        ResourceSegment::whole(0),
        1_500,
    );
    let capture = feed(&mut receiver, &advertisement, 2_000);
    assert_eq!(
        (capture.responses, capture.settlements),
        (
            std::vec![],
            std::vec![(
                CommandId(42),
                Settlement::SendRequest(Err(SendRequestFailure::ResourceCapacity))
            )]
        )
    );
    assert_eq!(receiver.incoming_resources.active_buffer_bytes(), 0);
    assert!(receiver.incoming_resources.is_empty());
    assert!(receiver.pending_resource_offers.is_empty());
    assert!(!receiver.receipts.has_pending_request(&link_id(), request));
}

#[test]
fn metadata_does_not_bypass_fixed_transfer_capacity() {
    assert_capacity_refusal(engine_with_active_link(), 4096);
}

#[test]
fn metadata_does_not_bypass_the_heap_memory_budget() {
    let mut receiver = active_engine::<GrowableHeap>();
    receiver.set_resource_memory_limits(ResourceMemoryLimits {
        incoming_bytes: 0,
        outgoing_bytes: 0,
    });
    assert_capacity_refusal(receiver, 128);
}

#[test]
fn metadata_does_not_bypass_the_uncompressed_stream_ceiling() {
    for declared in [MAX_EFFICIENT_SIZE as u64 + 1, u64::MAX] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request_with_limit(
            &mut receiver,
            CommandId(42),
            1_800,
            20_000,
            ByteLimit::Maximum(0),
        );
        let advertisement = advertise_response_segment_from(
            &mut engine_with_active_link(),
            CommandId(7),
            request,
            b"\0\0\0",
            None,
            ResourceSegment::whole(3),
            1_500,
        );
        let advertisement = rewrite_advertisement(&advertisement, |ad| {
            ad.flags.has_metadata = true;
            ad.data_bytes = declared;
        });
        let capture = feed(&mut receiver, &advertisement, 2_000);
        assert_eq!(
            (capture.frames, capture.responses, capture.settlements),
            (std::vec![], std::vec![], std::vec![])
        );
        assert_eq!(receiver.incoming_resources.active_buffer_bytes(), 0);
        assert!(receiver.incoming_resources.is_empty());
        assert!(receiver.pending_resource_offers.is_empty());
        // Policy-declined offers allocate nothing; the request keeps its normal deadline.
        assert!(receiver.receipts.has_pending_request(&link_id(), request));
    }
}

#[test]
fn malformed_metadata_cannot_turn_framing_into_a_successful_empty_response() {
    for stream in [&[][..], &[0], &[0, 0], &[0, 0, 1], &[0xFF, 0xFF, 0xFF, 0]] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request_with_limit(
            &mut receiver,
            CommandId(42),
            1_800,
            20_000,
            ByteLimit::Maximum(0),
        );
        let mut sender = engine_with_active_link();
        let advertisement = advertise_response_segment_from(
            &mut sender,
            CommandId(7),
            request,
            stream,
            None,
            ResourceSegment::whole(stream.len() as u64),
            1_500,
        );
        let advertisement =
            rewrite_advertisement(&advertisement, |ad| ad.flags.has_metadata = true);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        assert_eq!(pull.frames.len(), 1);
        let served = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(served.frames.len(), 1);
        let capture = feed(&mut receiver, &served.frames[0].1, 2_200);
        assert_eq!(
            (capture.responses, capture.settlements),
            (
                std::vec![],
                std::vec![(
                    CommandId(42),
                    Settlement::SendRequest(Err(SendRequestFailure::ResponseTransferFailed(
                        ResourceFailureCause::MetadataOverrun
                    )))
                )]
            )
        );
        assert!(!receiver.receipts.has_pending_request(&link_id(), request));
        assert!(receiver.incoming_resources.is_empty());
        assert!(receiver.pending_resource_offers.is_empty());
    }
}

#[test]
fn split_file_segments_are_literal_data() {
    let mut receiver = engine_with_active_link();
    let request = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);
    let first = envelope_shaped_file(request);
    let last = envelope_shaped_file(RequestId([0xF3; 16]));
    let metadata = b"opaque metadata";
    let total_data_bytes = (first.len() + last.len()) as u64;
    let mut sender = engine_with_active_link();
    let mut responses = std::vec::Vec::new();
    let mut settlements = std::vec::Vec::new();
    for (index, data, metadata) in [
        (1, first.as_slice(), ResourceMetadata::Packed(metadata)),
        (
            2,
            last.as_slice(),
            ResourceMetadata::SentInFirstSegment {
                packed_len: metadata.len() as u32,
            },
        ),
    ] {
        let offset = (index - 1) * 1_000;
        let advertisement = advertise_response_body_from(
            &mut sender,
            CommandId(7),
            request,
            ResourceBody {
                data,
                compressed_candidate: None,
                metadata,
            },
            ResourceSegment {
                index,
                total_segments: 2,
                total_data_bytes,
            },
            1_500 + offset,
        );
        let pull = feed(&mut receiver, &advertisement, 2_000 + offset);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100 + offset);
        assert_eq!(serve.frames.len(), 1);
        let capture = feed(&mut receiver, &serve.frames[0].1, 2_200 + offset);
        responses.extend(capture.response_segments);
        settlements.extend(capture.settlements);
        for (_, proof) in capture.frames {
            feed(&mut sender, &proof, 2_300 + offset);
        }
    }
    assert_eq!(
        (responses, settlements),
        (
            std::vec![
                (CommandId(42), request, 1, first),
                (CommandId(42), request, 2, last.to_vec())
            ],
            std::vec![(
                CommandId(42),
                completion(&last, ByteLimit::Unlimited, 1_400)
            )],
        )
    );
    assert!(!receiver.receipts.has_pending_request(&link_id(), request));
    assert!(receiver.incoming_resources.is_empty());
    assert_eq!(receiver.incoming_assemblies.original_hash(&link_id()), None);
}

proptest! {
    #[test]
    fn arbitrary_metadata_does_not_change_literal_response_bytes_or_their_limit(
        data in proptest::collection::vec(any::<u8>(), 0..=256),
        packed in proptest::collection::vec(any::<u8>(), 0..=64),
        maximum in 0u64..=257,
    ) {
        check_file(ResourceBody { data: &data, compressed_candidate: None, metadata: ResourceMetadata::Packed(&packed) }, ByteLimit::Maximum(maximum));
    }
}
