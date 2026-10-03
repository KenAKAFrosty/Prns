use super::completion::{reach_split_boundary, SplitBoundary};
use super::*;
use crate::engine::test_support::routable_descriptor;
use crate::engine::{
    IngestIo, OwedWork, WholeResourceOpenCompleted, WholeResourceOpenLanding,
    WholeResourceOpenOutcome,
};
use crate::interfaces::{AttachedInterfaces, InboundPacket};
use crate::routing::links::resources::assemble_incoming::verify_and_prove;
use crate::routing::links::resources::streamed_open::ResourceOpenLane;
use crate::routing::links::resources::RESOURCE_NONCE_LEN;

enum WorkerResult {
    Opened,
    OpenedAndDigested,
    Unavailable,
}

#[test]
fn delayed_whole_open_verdicts_preserve_split_ownership_and_retire_the_reservation() {
    check_delayed_whole_open(SplitBoundary::Active);
}

#[test]
fn delayed_whole_open_verdicts_cannot_revive_a_completed_request() {
    check_delayed_whole_open(SplitBoundary::Completed);
}

#[test]
fn delayed_whole_open_verdicts_cannot_revive_an_expired_request() {
    check_delayed_whole_open(SplitBoundary::Expired);
}

fn check_delayed_whole_open(boundary: SplitBoundary) {
    for result in [
        WorkerResult::Opened,
        WorkerResult::OpenedAndDigested,
        WorkerResult::Unavailable,
    ] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request(&mut receiver, REQUEST, 1_800, 20_000);
        receiver.resource_open_lane = ResourceOpenLane::ExternalWhole;
        let mut competitor = engine_with_active_link();
        let advertisement = advertise_response_segment_from(
            &mut competitor,
            CommandId(99),
            request,
            &[0xE7; 128],
            None,
            ResourceSegment::whole(128),
            1_810,
        );
        let pull = feed(&mut receiver, &advertisement, 1_820);
        assert_eq!(pull.frames.len(), 1);
        let mut served = feed(&mut competitor, &pull.frames[0].1, 1_830);
        assert_eq!(served.frames.len(), 1);
        let mut job = None;
        receiver.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_840),
                source_interface: lane(),
                bytes: &mut served.frames[0].1,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[routable_descriptor(lane())]),
                now: InstantMillis(1_840),
                fill_random: &mut |bytes| bytes.fill(0xC7),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::Fulfill(OwedWork::WholeResourceOpen(
                        owed,
                    ))) => {
                        assert!(job.is_none());
                        let sealed = owed.sealed().to_vec();
                        job = Some((owed.into_plan(), sealed));
                    }
                    _ => panic!("the completed transfer must only dispatch its open job"),
                },
            },
        );
        let (plan, mut sealed) = job.expect("whole open is parked before split admission");
        let reservation = plan.reservation();
        let plaintext = plan.key().open_in_place(&mut sealed).unwrap();
        let proof = verify_and_prove(
            &plaintext[RESOURCE_NONCE_LEN..],
            &plan.salt_nonce(),
            &plan.hash(),
        )
        .unwrap();
        // Dispatch the split normally while retaining the already-issued job.
        receiver.resource_open_lane = ResourceOpenLane::EngineDirected;
        let mut response = SplitResponse::from_pending(receiver, request);
        let at = reach_split_boundary(&mut response, &boundary);
        response.receiver.resource_open_lane = ResourceOpenLane::ExternalWhole;
        let deadline = response
            .receiver
            .receipts
            .pending_request_deadline(&link_id(), request);
        let outcome = match result {
            WorkerResult::Opened => WholeResourceOpenOutcome::Opened(plaintext),
            WorkerResult::OpenedAndDigested => WholeResourceOpenOutcome::OpenedAndDigested {
                plaintext,
                calculated_hash: plan.hash(),
                proof,
            },
            WorkerResult::Unavailable => WholeResourceOpenOutcome::Unavailable,
        };
        let mut capture = InboundCapture::default();
        assert_eq!(
            response.receiver.resume_whole_resource_open(
                WholeResourceOpenCompleted {
                    reservation,
                    outcome
                },
                InstantMillis(at),
                &mut |bytes| bytes.fill(0xC9),
                &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) =>
                        capture.frames.push((target, filled_frame(fill).unwrap())),
                    _ => panic!("superseded worker completion must only emit cancellation"),
                },
            ),
            WholeResourceOpenLanding::Applied
        );
        assert_cancelled(&mut competitor, capture, at + 50);
        assert_eq!(
            response
                .receiver
                .receipts
                .pending_request_deadline(&link_id(), request),
            deadline
        );
        assert!(response.receiver.incoming_resources.is_empty());
        assert!(matches!(
            response.receiver.resource_open_lane,
            ResourceOpenLane::ExternalWhole
        ));
        assert_eq!(
            response.receiver.resume_whole_resource_open(
                WholeResourceOpenCompleted {
                    reservation,
                    outcome: WholeResourceOpenOutcome::Opened(plaintext)
                },
                InstantMillis(at + 60),
                &mut |_| panic!("stale work needs no entropy"),
                &mut |_| panic!("stale work must have no effect"),
            ),
            WholeResourceOpenLanding::Stale
        );
        if !matches!(boundary, SplitBoundary::Active) {
            continue;
        }
        response.receiver.resource_open_lane = ResourceOpenLane::EngineDirected;
        let pull = feed(&mut response.receiver, &response.continuation, 2_500);
        assert_eq!(pull.frames.len(), 1);
        assert_eq!(
            response.receiver.resume_whole_resource_open(
                WholeResourceOpenCompleted {
                    reservation,
                    outcome: WholeResourceOpenOutcome::Opened(plaintext),
                },
                InstantMillis(2_550),
                &mut |_| panic!("stale work needs no entropy after transfer-slot reuse"),
                &mut |_| panic!("stale work must not disturb the active continuation"),
            ),
            WholeResourceOpenLanding::Stale
        );
        response.complete(&pull.frames[0].1);
    }
}
