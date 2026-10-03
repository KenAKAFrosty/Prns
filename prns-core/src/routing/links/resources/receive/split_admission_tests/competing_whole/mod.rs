use super::*;
use crate::engine::{RespondFailure, SendResourceFailure};
use crate::units::ByteLimit;
use crate::wire::{WireContext, WirePacketHeader};
mod completion;
mod streamed_completion;
#[cfg(feature = "resource-work-offload")]
mod worker_completion;

fn assert_cancelled(
    competitor: &mut EngineState<TestStorageLayout>,
    mut rejected: InboundCapture,
    at: u64,
) {
    assert_eq!(rejected.frames.len(), 1);
    let (_, cancel) = rejected.frames.pop().unwrap();
    assert_eq!(
        WirePacketHeader::parse(&cancel).unwrap().0.context,
        WireContext::ResourceReceiverCancel
    );
    assert_eq!(rejected, InboundCapture::default());
    assert_eq!(
        feed(competitor, &cancel, at),
        InboundCapture {
            settlements: std::vec![(
                CommandId(99),
                Settlement::Respond(Err(RespondFailure::Resource(
                    SendResourceFailure::RejectedByPeer
                )))
            )],
            ..InboundCapture::default()
        }
    );
    assert!(competitor.outgoing_resources.is_empty());
}

#[test]
fn competing_whole_offers_preserve_the_admitted_split_response() {
    for declared_bytes in [128, 257, u64::MAX] {
        let mut response = SplitResponse::with_limit(ByteLimit::Maximum(256));
        // Independent peer state deliberately bypasses the normal sender's
        // same-link exclusion while producing authentic protocol frames.
        let mut competitor = engine_with_active_link();
        let ad = advertise_response_segment_from(
            &mut competitor,
            CommandId(99),
            response.request,
            &[0xE7; 128],
            None,
            ResourceSegment::whole(128),
            2_350,
        );
        let ad = rewrite_advertisement(&ad, |ad| ad.data_bytes = declared_bytes);
        let deadline = response
            .receiver
            .receipts
            .pending_request_deadline(&link_id(), response.request);
        let rejected = feed(&mut response.receiver, &ad, 2_400);
        assert_cancelled(&mut competitor, rejected, 2_450);
        assert!(response.receiver.incoming_resources.is_empty());
        assert!(response.receiver.pending_resource_offers.is_empty());
        assert_eq!(
            response
                .receiver
                .receipts
                .pending_request_deadline(&link_id(), response.request),
            deadline
        );
        let pull = feed(&mut response.receiver, &response.continuation, 2_500);
        assert_eq!(pull.frames.len(), 1);
        response.complete(&pull.frames[0].1);
    }
}

#[test]
fn whole_response_exclusion_requires_matching_link_request_and_whole_shape() {
    use crate::routing::links::resources::ResourceCorrelation;
    use crate::routing::links::LinkId;
    let response = SplitResponse::after_first_segment();
    for (link, segments, correlation, expected) in [
        (
            link_id(),
            1,
            ResourceCorrelation::Response(response.request),
            true,
        ),
        (
            LinkId::new([0xF4; 16]),
            1,
            ResourceCorrelation::Response(response.request),
            false,
        ),
        (
            link_id(),
            1,
            ResourceCorrelation::Response(RequestId([0xF3; 16])),
            false,
        ),
        (
            link_id(),
            2,
            ResourceCorrelation::Response(response.request),
            false,
        ),
        (link_id(), 1, ResourceCorrelation::Unsolicited, false),
    ] {
        assert_eq!(
            response
                .receiver
                .whole_response_is_superseded(&link, segments, correlation),
            expected
        );
    }
}

#[test]
fn queued_whole_offers_cannot_claim_or_expire_a_new_split_owner() {
    enum QueueState {
        Available,
        Full,
        Expired,
    }
    for queue_state in [QueueState::Available, QueueState::Full, QueueState::Expired] {
        let mut response = SplitResponse::with_limit(ByteLimit::Maximum(256));
        let mut competitor = engine_with_active_link();
        let ad = advertise_response_segment_from(
            &mut competitor,
            CommandId(99),
            response.request,
            &[0xE7; 128],
            None,
            ResourceSegment::whole(128),
            2_350,
        );
        // Model an offer queued before the current split acquired ownership.
        response.receiver.incoming_assemblies.clear(&link_id());
        accept_everything(&mut response.receiver);
        let mut blockers = std::vec::Vec::new();
        for byte in [0xC1, 0xC2] {
            assert_eq!(
                feed(
                    &mut response.receiver,
                    &advertisement_frame(&[byte; 128], None),
                    2_350
                )
                .frames
                .len(),
                1
            );
            blockers.push(
                *response
                    .receiver
                    .incoming_resources
                    .hash_at(response.receiver.incoming_resources.len() - 1),
            );
        }
        assert_eq!(
            feed(&mut response.receiver, &ad, 2_400),
            InboundCapture::default()
        );
        assert_eq!(response.receiver.pending_resource_offers.len(), 1);
        response.receiver.incoming_assemblies.begin(
            link_id(),
            response.original,
            2,
            256,
            AssemblyCorrelation::Response(response.request),
        );
        assert_eq!(
            response.receiver.incoming_assemblies.advance(
                &link_id(),
                &response.original,
                ResourceSegment {
                    index: 1,
                    total_segments: 2,
                    total_data_bytes: 256
                },
                AssemblyBytes {
                    stream: 128,
                    value: 128
                },
                AssemblyCorrelation::Response(response.request),
            ),
            Some(AssemblyProgress::Assembling)
        );
        assert_eq!(
            response.receiver.pending_resource_deadline(),
            Some(InstantMillis(0))
        );
        let deadline = response
            .receiver
            .receipts
            .pending_request_deadline(&link_id(), response.request);
        if !matches!(queue_state, QueueState::Full) {
            for hash in &blockers {
                response.receiver.retire_incoming_resource(&link_id(), hash);
            }
        }
        let mut rejected = InboundCapture::default();
        let at = if matches!(queue_state, QueueState::Expired) {
            7_400
        } else {
            2_500
        };
        response.receiver.fire_due_resource_deadlines(
            InstantMillis(at),
            &mut |bytes| bytes.fill(0xC5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    rejected.frames.push((target, filled_frame(fill).unwrap()))
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    rejected.settlements.push((id, settlement))
                }
                _ => panic!("superseded offer must not produce any other effect"),
            },
        );
        assert_cancelled(&mut competitor, rejected, at + 50);
        assert!(response.receiver.pending_resource_offers.is_empty());
        assert_eq!(
            response
                .receiver
                .receipts
                .pending_request_deadline(&link_id(), response.request),
            deadline
        );
        for hash in &blockers {
            response.receiver.retire_incoming_resource(&link_id(), hash);
        }
        let pull = feed(&mut response.receiver, &response.continuation, at + 200);
        assert_eq!(pull.frames.len(), 1);
        response.complete_at(&pull.frames[0].1, at + 500);
    }
}
