use super::*;

const SEGMENT_BYTES: usize = 128;
const STREAM_BYTES: u64 = (2 * SEGMENT_BYTES) as u64;

fn check_stream_size(opening: Opening, advertised: u64) {
    let mut receiver = engine_with_active_link();
    let mut sender = engine_with_active_link();
    let request = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);
    let mut all = Delivery::default();
    for index in 1..=2 {
        let data = [0xA0 + index as u8; SEGMENT_BYTES];
        let at = 2_000 + (index - 1) * 1_000;
        let advertisement = advertise_response_segment_from(
            &mut sender,
            CommandId(20 + index),
            request,
            &data,
            opening.candidate(),
            ResourceSegment {
                index,
                total_segments: 2,
                total_data_bytes: STREAM_BYTES,
            },
            at,
        );
        let advertisement = rewrite_advertisement(&advertisement, |ad| ad.data_bytes = advertised);
        let pull = feed(&mut receiver, &advertisement, at + 100);
        assert_eq!(pull.frames.len(), 1);
        let hash = receiver
            .incoming_resources
            .first_hash_for_link(&link_id())
            .unwrap();
        let served = feed(&mut sender, &pull.frames[0].1, at + 200);
        assert_eq!(served.frames.len(), 1);
        let mut delivered = Delivery::default();
        let proofs = finish_segment(
            &mut receiver,
            &served.frames,
            &data,
            &opening,
            at + 300,
            &mut delivered,
        );
        let cumulative = index * SEGMENT_BYTES as u64;
        let valid = cumulative <= advertised && (index < 2 || cumulative == advertised);
        if !valid {
            assert!(
                proofs.is_empty(),
                "a false stream size must not receive a proof"
            );
            assert_eq!(
                delivered,
                expected_failure(hash, ResourceFailureCause::TransferCorrupt)
            );
            assert_retired(&receiver, request);
            for (_, part) in &served.frames {
                assert_eq!(
                    feed(&mut receiver, part, at + 500),
                    InboundCapture::default()
                );
            }
            return;
        }
        assert_eq!(proofs.len(), 1);
        for (_, proof) in proofs {
            feed(&mut sender, &proof, at + 400);
        }
        all.chunks.extend(delivered.chunks);
        all.settlements.extend(delivered.settlements);
        all.failures.extend(delivered.failures);
    }
    let finished_at = match opening {
        Opening::Uncompressed => 3_300,
        Opening::Inflated => 3_310,
    };
    assert_eq!(
        all,
        Delivery {
            chunks: std::vec![
                (CommandId(42), request, 1, std::vec![0xA1; SEGMENT_BYTES]),
                (CommandId(42), request, 2, std::vec![0xA2; SEGMENT_BYTES]),
            ],
            settlements: std::vec![(
                CommandId(42),
                Settlement::SendRequest(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(finished_at - 1_800),
                    evidence: DeliveryEvidence::Response,
                }))
            )],
            failures: std::vec![],
        }
    );
    assert_retired(&receiver, request);
}

#[test]
fn opened_segments_enforce_cumulative_advertised_stream_size() {
    for advertised in [
        0,
        SEGMENT_BYTES as u64 - 1,
        SEGMENT_BYTES as u64,
        STREAM_BYTES - 1,
        STREAM_BYTES,
        STREAM_BYTES + 1,
    ] {
        check_stream_size(Opening::Uncompressed, advertised);
    }
}

#[test]
fn inflated_segments_enforce_cumulative_advertised_stream_size() {
    for advertised in [
        0,
        SEGMENT_BYTES as u64 - 1,
        SEGMENT_BYTES as u64,
        STREAM_BYTES - 1,
        STREAM_BYTES,
        STREAM_BYTES + 1,
    ] {
        check_stream_size(Opening::Inflated, advertised);
    }
}

#[test]
fn changed_stream_size_cannot_be_completed_or_cleared_by_an_old_transfer() {
    for opening in [Opening::Uncompressed, Opening::Inflated] {
        let mut receiver = engine_with_active_link();
        let mut sender = engine_with_active_link();
        let request = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);
        let data = [0xA1; SEGMENT_BYTES];
        let advertisement = advertise_response_segment_from(
            &mut sender,
            CommandId(20),
            request,
            &data,
            opening.candidate(),
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: STREAM_BYTES,
            },
            2_000,
        );
        let pull = feed(&mut receiver, &advertisement, 2_100);
        assert_eq!(pull.frames.len(), 1);
        let hash = receiver
            .incoming_resources
            .first_hash_for_link(&link_id())
            .unwrap();
        let served = feed(&mut sender, &pull.frames[0].1, 2_200);
        assert_eq!(served.frames.len(), 1);
        let mut delivered = Delivery::default();
        if matches!(opening, Opening::Inflated) {
            assert!(delivered
                .absorb(feed(&mut receiver, &served.frames[0].1, 2_300))
                .is_empty());
            assert_eq!(delivered, Delivery::default());
        }
        let deadline = receiver
            .receipts
            .pending_request_deadline(&link_id(), request);
        receiver.incoming_assemblies.begin(
            link_id(),
            hash,
            2,
            STREAM_BYTES + 1,
            AssemblyCorrelation::Response(request),
        );
        match opening {
            Opening::Uncompressed => assert!(delivered
                .absorb(feed(&mut receiver, &served.frames[0].1, 2_400))
                .is_empty()),
            Opening::Inflated => {
                receiver.resume_resource_decompression(
                    ResourceDecompressionCompleted {
                        link_id: link_id(),
                        hash,
                        plaintext: &data,
                    },
                    InstantMillis(2_400),
                    &mut |bytes| bytes.fill(0xC9),
                    &mut |reaction: EngineReaction<'_, NoOwedWork>| {
                        assert!(!matches!(
                            &reaction,
                            EngineReaction::Directive(Directive::EmitFrame { .. })
                        ));
                        delivered.record(reaction);
                    },
                );
            }
        }
        assert_eq!(
            delivered,
            Delivery {
                failures: std::vec![(hash, ResourceFailureCause::TransferCorrupt)],
                ..Delivery::default()
            }
        );
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(
            receiver
                .receipts
                .pending_request_deadline(&link_id(), request),
            deadline
        );
        assert_eq!(
            receiver.incoming_assemblies.fit(
                &link_id(),
                &hash,
                ResourceSegment {
                    index: 1,
                    total_segments: 2,
                    total_data_bytes: STREAM_BYTES + 1
                },
                AssemblyCorrelation::Response(request),
            ),
            crate::routing::links::resources::assembly::SegmentFit::Expected
        );
    }
}
