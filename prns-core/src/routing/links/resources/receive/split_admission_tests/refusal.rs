use super::*;
use crate::engine::SendRequestFailure;
use crate::routing::links::resources::assembly::AssemblyBytes;
use crate::units::ByteLimit;
use crate::wire::{WireContext, WirePacketHeader};

enum Refusal {
    TransferCapacity,
    QueueCapacity,
    QueueDeadline,
}

fn occupy_transfers(response: &mut SplitResponse) {
    accept_everything(&mut response.receiver);
    for byte in [0xC1, 0xC2] {
        assert_eq!(
            feed(
                &mut response.receiver,
                &advertisement_frame(&[byte; SEGMENT_BYTES], None),
                2_350,
            )
            .frames
            .len(),
            1
        );
    }
}

fn expire_offers(response: &mut SplitResponse) -> InboundCapture {
    let mut capture = InboundCapture::default();
    response.receiver.fire_due_resource_deadlines(
        InstantMillis(7_400),
        &mut |bytes| bytes.fill(0xC5),
        &mut |reaction| match reaction {
            EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                capture.frames.push((target, filled_frame(fill).unwrap()));
            }
            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                capture.settlements.push((id, settlement));
            }
            _ => {}
        },
    );
    capture
}

fn refused_continuation_cleans_up(refusal: Refusal) {
    let mut response = SplitResponse::with_limit(ByteLimit::Maximum(256));
    let continuation = match refusal {
        Refusal::TransferCapacity => rewrite_advertisement(&response.continuation, |ad| {
            ad.transfer_bytes = 5_000;
        }),
        Refusal::QueueCapacity => {
            occupy_transfers(&mut response);
            for byte in [0xD1, 0xD2, 0xD3, 0xD4] {
                assert_eq!(
                    feed(
                        &mut response.receiver,
                        &advertisement_frame(&[byte; SEGMENT_BYTES], None),
                        2_375,
                    ),
                    InboundCapture::default()
                );
            }
            assert_eq!(response.receiver.pending_resource_offers.len(), 4);
            response.continuation.clone()
        }
        Refusal::QueueDeadline => {
            occupy_transfers(&mut response);
            response.continuation.clone()
        }
    };
    let mut rejected = feed(&mut response.receiver, &continuation, 2_400);
    if matches!(refusal, Refusal::QueueDeadline) {
        assert_eq!(rejected, InboundCapture::default());
        assert_eq!(response.receiver.pending_resource_offers.len(), 1);
        rejected = expire_offers(&mut response);
    }
    assert!(rejected.frames.iter().any(|(_, frame)| {
        WirePacketHeader::parse(frame)
            .is_ok_and(|(header, _)| header.context == WireContext::ResourceReceiverCancel)
    }));
    rejected.frames.clear();
    let failure = match refusal {
        Refusal::TransferCapacity | Refusal::QueueCapacity | Refusal::QueueDeadline => {
            SendRequestFailure::ResourceCapacity
        }
    };
    assert_eq!(
        rejected,
        InboundCapture {
            settlements: std::vec![(REQUEST, Settlement::SendRequest(Err(failure)))],
            ..InboundCapture::default()
        }
    );
    assert!(!response
        .receiver
        .receipts
        .has_pending_request(&link_id(), response.request));
    assert_eq!(
        response
            .receiver
            .incoming_assemblies
            .original_hash(&link_id()),
        None
    );
}

#[test]
fn impossible_continuation_releases_the_failed_response_assembly() {
    refused_continuation_cleans_up(Refusal::TransferCapacity);
}

#[test]
fn full_offer_queue_releases_the_failed_response_assembly() {
    refused_continuation_cleans_up(Refusal::QueueCapacity);
}

#[test]
fn expired_offer_wait_releases_the_failed_response_assembly() {
    refused_continuation_cleans_up(Refusal::QueueDeadline);
}

#[test]
fn stale_queued_continuations_cannot_fail_a_replacement_at_the_wait_deadline() {
    for change in [
        QueuedChain::Removed,
        QueuedChain::Replaced,
        QueuedChain::Advanced,
        QueuedChain::ChangedCount,
        QueuedChain::ChangedStreamSize,
        QueuedChain::ChangedCorrelation,
    ] {
        let mut response = SplitResponse::after_first_segment();
        occupy_transfers(&mut response);
        assert_eq!(
            feed(&mut response.receiver, &response.continuation, 2_400),
            InboundCapture::default()
        );
        assert_eq!(response.receiver.pending_resource_offers.len(), 1);
        let deadline = response
            .receiver
            .receipts
            .pending_request_deadline(&link_id(), response.request);
        match change {
            QueuedChain::Removed => response.receiver.incoming_assemblies.clear(&link_id()),
            QueuedChain::Replaced
            | QueuedChain::ChangedCount
            | QueuedChain::ChangedStreamSize
            | QueuedChain::ChangedCorrelation => {
                let original = match change {
                    QueuedChain::Replaced => ResourceHash::new([0xD1; 32]),
                    _ => response.original,
                };
                let count = match change {
                    QueuedChain::ChangedCount => 3,
                    _ => 2,
                };
                let correlation = match change {
                    QueuedChain::ChangedCorrelation => {
                        AssemblyCorrelation::Request(response.request)
                    }
                    _ => AssemblyCorrelation::Response(response.request),
                };
                let size = if matches!(change, QueuedChain::ChangedStreamSize) {
                    512
                } else {
                    256
                };
                response.receiver.incoming_assemblies.begin(
                    link_id(),
                    original,
                    count,
                    size,
                    correlation,
                );
                assert_eq!(
                    response.receiver.incoming_assemblies.advance(
                        &link_id(),
                        &original,
                        ResourceSegment {
                            index: 1,
                            total_segments: count,
                            total_data_bytes: size
                        },
                        AssemblyBytes {
                            stream: SEGMENT_BYTES as u64,
                            value: SEGMENT_BYTES as u64
                        },
                        correlation
                    ),
                    Some(AssemblyProgress::Assembling)
                );
            }
            QueuedChain::Advanced => {
                assert_eq!(
                    response.receiver.incoming_assemblies.advance(
                        &link_id(),
                        &response.original,
                        ResourceSegment {
                            index: 2,
                            total_segments: 2,
                            total_data_bytes: 256
                        },
                        AssemblyBytes {
                            stream: SEGMENT_BYTES as u64,
                            value: SEGMENT_BYTES as u64
                        },
                        AssemblyCorrelation::Response(response.request)
                    ),
                    Some(AssemblyProgress::Complete {
                        total_size_bytes: (2 * SEGMENT_BYTES) as u64
                    })
                );
            }
            QueuedChain::Unchanged => unreachable!(),
        }
        let expected = (
            response
                .receiver
                .incoming_assemblies
                .original_hash(&link_id()),
            response
                .receiver
                .incoming_assemblies
                .correlation(&link_id()),
            deadline,
        );
        let mut expired = expire_offers(&mut response);
        assert!(expired.frames.iter().all(|(_, frame)| {
            WirePacketHeader::parse(frame).unwrap().0.context != WireContext::ResourceReceiverCancel
        }));
        expired.frames.clear();
        assert_eq!(expired, InboundCapture::default());
        assert!(response.receiver.pending_resource_offers.is_empty());
        assert_eq!(
            (
                response
                    .receiver
                    .incoming_assemblies
                    .original_hash(&link_id()),
                response
                    .receiver
                    .incoming_assemblies
                    .correlation(&link_id()),
                response
                    .receiver
                    .receipts
                    .pending_request_deadline(&link_id(), response.request),
            ),
            expected
        );
    }
}

#[test]
fn refusing_a_whole_response_does_not_clear_another_assembly_correlation() {
    for correlation in [
        AssemblyCorrelation::Unsolicited,
        AssemblyCorrelation::Request(RequestId([0xE1; 16])),
        AssemblyCorrelation::Response(RequestId([0xE2; 16])),
    ] {
        let mut response = SplitResponse::with_limit(ByteLimit::Maximum(256));
        response.receiver.incoming_assemblies.begin(
            link_id(),
            response.original,
            2,
            256,
            correlation,
        );
        let refused = rewrite_advertisement(&response.continuation, |ad| {
            ad.segment_index = 1;
            ad.total_segments = 1;
            ad.flags.split = false;
            ad.data_bytes = 1_000;
        });
        let mut rejected = feed(&mut response.receiver, &refused, 2_400);
        assert_eq!(rejected.frames.len(), 1);
        rejected.frames.clear();
        assert_eq!(
            rejected,
            InboundCapture {
                settlements: std::vec![(
                    REQUEST,
                    Settlement::SendRequest(Err(SendRequestFailure::ResponseTooLarge))
                )],
                ..InboundCapture::default()
            }
        );
        assert_eq!(
            (
                response
                    .receiver
                    .incoming_assemblies
                    .original_hash(&link_id()),
                response
                    .receiver
                    .incoming_assemblies
                    .correlation(&link_id()),
            ),
            (Some(response.original), Some(correlation))
        );
    }
}
