use super::*;
use crate::engine::{NoOwedWork, ResourceDecompressionCompleted};

enum Completion {
    Uncompressed,
    DeferredInflate,
}

pub(super) enum SplitBoundary {
    Active,
    Completed,
    Expired,
}

pub(super) fn reach_split_boundary<S: crate::storage::StorageLayout>(
    response: &mut SplitResponse<S>,
    boundary: &SplitBoundary,
) -> u64 {
    match boundary {
        SplitBoundary::Active => 2_400,
        SplitBoundary::Completed => {
            let pull = feed(&mut response.receiver, &response.continuation, 2_500);
            assert_eq!(pull.frames.len(), 1);
            complete_admitted_split(response, &pull.frames[0].1)
        }
        SplitBoundary::Expired => expire_split(response),
    }
}

pub(super) fn complete_admitted_split<S: crate::storage::StorageLayout>(
    response: &mut SplitResponse<S>,
    pull: &[u8],
) -> u64 {
    let mut completed = serve_pull(&mut response.sender, &mut response.receiver, pull, 3_000);
    assert_eq!(completed.frames.len(), 1);
    for (_, proof) in completed.frames.drain(..) {
        feed(&mut response.sender, &proof, 3_100);
    }
    assert_eq!(
        completed,
        InboundCapture {
            response_segments: std::vec![(REQUEST, response.request, 2, LAST.to_vec())],
            settlements: std::vec![(
                REQUEST,
                Settlement::SendRequest(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(1_200),
                    evidence: DeliveryEvidence::Response,
                }))
            )],
            ..InboundCapture::default()
        }
    );
    assert_eq!(
        response
            .receiver
            .incoming_assemblies
            .original_hash(&link_id()),
        None
    );
    3_200
}

fn expire_split<S: crate::storage::StorageLayout>(response: &mut SplitResponse<S>) -> u64 {
    let deadline = response
        .receiver
        .receipts
        .pending_request_deadline(&link_id(), response.request)
        .unwrap();
    let mut settled = std::vec::Vec::new();
    response
        .receiver
        .settle_timed_out_receipts(deadline, &mut |reaction| match reaction {
            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                settled.push((id, settlement))
            }
            _ => panic!("expiry must only settle the request"),
        });
    assert_eq!(
        settled,
        std::vec![(
            REQUEST,
            Settlement::SendRequest(Err(crate::engine::SendRequestFailure::Timeout))
        )]
    );
    assert_eq!(
        response
            .receiver
            .incoming_assemblies
            .original_hash(&link_id()),
        None
    );
    deadline.0 + 100
}

#[test]
fn preadmitted_whole_completion_cannot_publish_over_a_split_response() {
    for boundary in [
        SplitBoundary::Active,
        SplitBoundary::Completed,
        SplitBoundary::Expired,
    ] {
        for completion in [Completion::Uncompressed, Completion::DeferredInflate] {
            let mut receiver = engine_with_active_link();
            let request = track_pending_request(&mut receiver, REQUEST, 1_800, 20_000);
            let mut competitor = engine_with_active_link();
            let body = [0xE7; 128];
            let candidate = match completion {
                Completion::Uncompressed => None,
                Completion::DeferredInflate => Some(b"worker-owned compressed bytes".as_slice()),
            };
            let advertisement = advertise_response_segment_from(
                &mut competitor,
                CommandId(99),
                request,
                &body,
                candidate,
                ResourceSegment::whole(128),
                1_810,
            );
            let pull = feed(&mut receiver, &advertisement, 1_820);
            assert_eq!(pull.frames.len(), 1);
            let hash = receiver
                .incoming_resources
                .first_hash_for_link(&link_id())
                .unwrap();
            let served = feed(&mut competitor, &pull.frames[0].1, 1_830);
            assert_eq!(served.frames.len(), 1);
            if let Completion::DeferredInflate = completion {
                assert_eq!(
                    feed(&mut receiver, &served.frames[0].1, 1_840),
                    InboundCapture::default()
                );
            }
            let mut response = SplitResponse::from_pending(receiver, request);
            let at = reach_split_boundary(&mut response, &boundary);
            let deadline = response
                .receiver
                .receipts
                .pending_request_deadline(&link_id(), request);
            let rejected = match completion {
                Completion::Uncompressed => feed(&mut response.receiver, &served.frames[0].1, at),
                Completion::DeferredInflate => {
                    let mut capture = InboundCapture::default();
                    response.receiver.resume_resource_decompression(
                        ResourceDecompressionCompleted {
                            link_id: link_id(),
                            hash,
                            plaintext: &body,
                        },
                        InstantMillis(at),
                        &mut |bytes| bytes.fill(0xC9),
                        &mut |reaction: EngineReaction<'_, NoOwedWork>| match reaction {
                            EngineReaction::Directive(Directive::EmitFrame {
                                target,
                                fill,
                                ..
                            }) => capture.frames.push((target, filled_frame(fill).unwrap())),
                            _ => panic!("superseded completion must only emit cancellation"),
                        },
                    );
                    capture
                }
            };
            assert_cancelled(&mut competitor, rejected, at + 50);
            assert_eq!(
                response
                    .receiver
                    .receipts
                    .pending_request_deadline(&link_id(), request),
                deadline
            );
            assert!(response.receiver.incoming_resources.is_empty());
            if let SplitBoundary::Active = boundary {
                let pull = feed(&mut response.receiver, &response.continuation, 2_500);
                assert_eq!(pull.frames.len(), 1);
                response.complete(&pull.frames[0].1);
            }
        }
    }
}
