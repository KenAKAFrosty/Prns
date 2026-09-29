use super::completion::{complete_admitted_split, reach_split_boundary, SplitBoundary};
use super::*;
use crate::engine::test_support::routable_descriptor;
use crate::engine::{
    IngestIo, OpenedResourceSpan, OwedWork, ResourceOpenCompleted, ResourceOpenSpanResidence,
};
use crate::interfaces::{AttachedInterfaces, InboundPacket};
use crate::routing::links::resources::streamed_open::StreamedOpen;

struct PendingSpan {
    hash: ResourceHash,
    start: usize,
    state: StreamedOpen,
    bytes: std::vec::Vec<u8>,
    residence: ResourceOpenSpanResidence,
}

enum SplitPhase {
    BetweenSegments,
    ReceivingContinuation,
    CompletedBeforeFirstReturn,
    ExpiredBeforeFirstReturn,
    CompletedBeforeTailReturn,
    ExpiredBeforeTailReturn,
}

#[test]
fn copied_streamed_tail_cannot_revive_an_expired_request() {
    check_streamed_overlap::<TestStorageLayout>(
        SplitPhase::ExpiredBeforeTailReturn,
        TailWorkspace::Copied,
    );
}

#[cfg(all(feature = "resource-work-offload", feature = "alloc"))]
#[test]
fn detached_streamed_tail_cannot_revive_an_expired_request() {
    check_streamed_overlap::<crate::storage::GrowableHeap>(
        SplitPhase::ExpiredBeforeTailReturn,
        TailWorkspace::Detached,
    );
}

#[test]
fn delayed_streamed_open_cannot_revive_a_completed_request() {
    check_streamed_overlap::<TestStorageLayout>(
        SplitPhase::CompletedBeforeFirstReturn,
        TailWorkspace::Copied,
    );
}

#[test]
fn delayed_streamed_open_cannot_revive_an_expired_request() {
    check_streamed_overlap::<TestStorageLayout>(
        SplitPhase::ExpiredBeforeFirstReturn,
        TailWorkspace::Copied,
    );
}

#[test]
fn copied_streamed_tail_cannot_revive_a_completed_request() {
    check_streamed_overlap::<TestStorageLayout>(
        SplitPhase::CompletedBeforeTailReturn,
        TailWorkspace::Copied,
    );
}

#[cfg(all(feature = "resource-work-offload", feature = "alloc"))]
#[test]
fn detached_streamed_tail_cannot_revive_a_completed_request() {
    check_streamed_overlap::<crate::storage::GrowableHeap>(
        SplitPhase::CompletedBeforeTailReturn,
        TailWorkspace::Detached,
    );
}

enum TailWorkspace {
    Copied,
    #[cfg(all(feature = "resource-work-offload", feature = "alloc"))]
    Detached,
}

#[test]
fn delayed_streamed_open_cannot_publish_over_a_split_response() {
    for phase in [
        SplitPhase::BetweenSegments,
        SplitPhase::ReceivingContinuation,
    ] {
        check_streamed_overlap::<TestStorageLayout>(phase, TailWorkspace::Copied);
    }
}

#[cfg(all(feature = "resource-work-offload", feature = "alloc"))]
#[test]
fn detached_streamed_tail_cannot_publish_over_a_split_response() {
    check_streamed_overlap::<crate::storage::GrowableHeap>(
        SplitPhase::ReceivingContinuation,
        TailWorkspace::Detached,
    );
}

fn check_streamed_overlap<S: crate::storage::StorageLayout>(
    phase: SplitPhase,
    workspace: TailWorkspace,
) {
    let mut receiver = active_engine::<S>();
    let request = track_pending_request(&mut receiver, REQUEST, 1_800, 20_000);
    let mut competitor = engine_with_active_link();
    let body = four_part_payload();
    let advertisement = advertise_response_segment_from(
        &mut competitor,
        CommandId(99),
        request,
        &body,
        None,
        ResourceSegment::whole(body.len() as u64),
        1_810,
    );
    let pull = feed(&mut receiver, &advertisement, 1_820);
    assert_eq!(pull.frames.len(), 1);
    let served = feed(&mut competitor, &pull.frames[0].1, 1_830);
    assert_eq!(served.frames.len(), 4);
    let mut job = None;
    for (index, (_, frame)) in served.frames.iter().enumerate() {
        let at = InstantMillis(1_840 + index as u64);
        let mut bytes = frame.clone();
        receiver.ingest_packet_into(
            InboundPacket {
                arrived_at: at,
                source_interface: lane(),
                bytes: &mut bytes,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[routable_descriptor(lane())]),
                now: at,
                fill_random: &mut |bytes| bytes.fill(0xC7),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::Fulfill(OwedWork::ResourceOpen(owed))) => {
                        assert_eq!(index, 0);
                        assert!(job.is_none());
                        assert!(!owed.other_transfers_in_flight);
                        job = Some(PendingSpan {
                            hash: owed.hash,
                            start: owed.span_start,
                            state: owed.state,
                            bytes: owed.bytes.to_vec(),
                            residence: owed.residence,
                        });
                    }
                    _ => panic!("parts arriving behind a parked open must not publish"),
                },
            },
        );
    }
    let mut pending = job.expect("first ciphertext span is held by worker");
    assert_eq!(pending.residence, ResourceOpenSpanResidence::Resident);
    let mut response = SplitResponse::from_pending(receiver, request);
    let mut continuation = None;
    if matches!(
        phase,
        SplitPhase::ReceivingContinuation
            | SplitPhase::CompletedBeforeTailReturn
            | SplitPhase::ExpiredBeforeTailReturn
    ) {
        let pull = feed(&mut response.receiver, &response.continuation, 2_350);
        assert_eq!(pull.frames.len(), 1);
        continuation = Some(pull.frames[0].1.clone());
    }
    let mut at = match phase {
        SplitPhase::CompletedBeforeFirstReturn => {
            reach_split_boundary(&mut response, &SplitBoundary::Completed)
        }
        SplitPhase::ExpiredBeforeFirstReturn => {
            reach_split_boundary(&mut response, &SplitBoundary::Expired)
        }
        SplitPhase::BetweenSegments
        | SplitPhase::ReceivingContinuation
        | SplitPhase::CompletedBeforeTailReturn
        | SplitPhase::ExpiredBeforeTailReturn => 2_400,
    };
    let mut deadline = response
        .receiver
        .receipts
        .pending_request_deadline(&link_id(), request);
    let mut capture = InboundCapture::default();
    let mut completions = 0;
    loop {
        completions += 1;
        assert!(
            completions <= 2,
            "the held span and at most one remaining span"
        );
        let opened = match (&workspace, completions) {
            #[cfg(all(feature = "resource-work-offload", feature = "alloc"))]
            (TailWorkspace::Detached, 2) => {
                use crate::engine::ResourceOpenWorkspace;
                let ResourceOpenSpanResidence::Transferable(reservation) = pending.residence else {
                    panic!("a completed tail must carry a transfer reservation");
                };
                let mut transfer = match response.receiver.take_resource_open_workspace(reservation)
                {
                    ResourceOpenWorkspace::DetachedTransfer(transfer) => transfer,
                    ResourceOpenWorkspace::CopiedSpan(_) | ResourceOpenWorkspace::Stale => {
                        panic!("heap workspace must detach rather than copy")
                    }
                };
                let index = response
                    .receiver
                    .incoming_resources
                    .lookup(&link_id(), &pending.hash)
                    .unwrap();
                assert!(!response
                    .receiver
                    .incoming_resources
                    .transfer_is_resident(index));
                assert!(matches!(
                    response.receiver.take_resource_open_workspace(reservation),
                    ResourceOpenWorkspace::Stale
                ));
                response
                    .receiver
                    .emit_resource_open(&link_id(), &pending.hash, &mut |_| {
                        panic!("detached work cannot dispatch twice")
                    });
                let span = reservation.span_start()..reservation.span_end();
                assert_eq!(span.start, pending.start);
                assert_eq!(transfer[span.clone()], pending.bytes);
                pending.state.chew_span(&mut transfer[span.clone()]);
                OpenedResourceSpan::ReturnedTransfer {
                    transfer,
                    span_byte_len: span.len(),
                }
            }
            _ => {
                pending.state.chew_span(&mut pending.bytes);
                OpenedResourceSpan::Returned(&pending.bytes)
            }
        };
        if completions == 2 && matches!(phase, SplitPhase::CompletedBeforeTailReturn) {
            at = complete_admitted_split(&mut response, continuation.as_ref().unwrap());
            deadline = None;
        }
        if completions == 2 && matches!(phase, SplitPhase::ExpiredBeforeTailReturn) {
            at = exhaust_transfer_deadlines(&mut response, pending.hash);
            deadline = None;
        }
        let mut next = None;
        response.receiver.resume_resource_open(
            ResourceOpenCompleted {
                link_id: link_id(),
                hash: pending.hash,
                span_start: pending.start,
                state: pending.state,
                opened,
                residence: pending.residence,
            },
            InstantMillis(at + completions),
            &mut |bytes| {
                assert!(
                    !matches!(phase, SplitPhase::ExpiredBeforeTailReturn),
                    "retired worker results need no entropy"
                );
                bytes.fill(0xC9);
            },
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Fulfill(OwedWork::ResourceOpen(owed))) => {
                    assert!(next.is_none());
                    assert!(owed.other_transfers_in_flight);
                    next = Some(PendingSpan {
                        hash: owed.hash,
                        start: owed.span_start,
                        state: owed.state,
                        bytes: owed.bytes.to_vec(),
                        residence: owed.residence,
                    });
                }
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    capture.frames.push((target, filled_frame(fill).unwrap()))
                }
                _ => panic!("streamed competitor must never publish or settle the request"),
            },
        );
        match next {
            Some(job) => pending = job,
            None => break,
        }
    }
    assert_eq!(
        completions,
        match phase {
            SplitPhase::BetweenSegments
            | SplitPhase::CompletedBeforeFirstReturn
            | SplitPhase::ExpiredBeforeFirstReturn => 1,
            SplitPhase::ReceivingContinuation
            | SplitPhase::CompletedBeforeTailReturn
            | SplitPhase::ExpiredBeforeTailReturn => 2,
        }
    );
    if matches!(phase, SplitPhase::ExpiredBeforeTailReturn) {
        assert_eq!(capture, InboundCapture::default());
    } else {
        assert_cancelled(&mut competitor, capture, at + 50);
    }
    assert_eq!(
        response
            .receiver
            .receipts
            .pending_request_deadline(&link_id(), request),
        deadline
    );
    if matches!(phase, SplitPhase::ExpiredBeforeTailReturn) {
        assert!(response.receiver.incoming_resources.is_empty());
        let late = serve_pull(
            &mut response.sender,
            &mut response.receiver,
            continuation.as_ref().unwrap(),
            at + 100,
        );
        assert_eq!(late, InboundCapture::default());
        assert!(response.receiver.incoming_resources.is_empty());
        assert_eq!(
            response.receiver.incoming_resources.active_buffer_bytes(),
            0
        );
        return;
    }
    if matches!(
        phase,
        SplitPhase::CompletedBeforeFirstReturn
            | SplitPhase::ExpiredBeforeFirstReturn
            | SplitPhase::CompletedBeforeTailReturn
    ) {
        assert!(response.receiver.incoming_resources.is_empty());
        assert_eq!(
            response.receiver.incoming_resources.active_buffer_bytes(),
            0
        );
        return;
    }
    let pull = match continuation {
        Some(pull) => pull,
        None => {
            let pull = feed(&mut response.receiver, &response.continuation, 2_500);
            assert_eq!(pull.frames.len(), 1);
            pull.frames[0].1.clone()
        }
    };
    response.complete(&pull);
}

fn exhaust_transfer_deadlines<S: crate::storage::StorageLayout>(
    response: &mut SplitResponse<S>,
    whole_hash: ResourceHash,
) -> u64 {
    use crate::engine::SendRequestFailure;
    use crate::routing::links::resources::ResourceFailureCause;

    assert_eq!(
        response
            .receiver
            .receipts
            .pending_request_deadline(&link_id(), response.request),
        None
    );
    assert!(response
        .receiver
        .receipts
        .has_pending_request(&link_id(), response.request));
    let continuation_hash = (0..response.receiver.incoming_resources.len())
        .map(|index| *response.receiver.incoming_resources.hash_at(index))
        .find(|hash| *hash != whole_hash)
        .unwrap();
    let mut observed = InboundCapture::default();
    let mut at = 2_402;
    let mut rounds = 0;
    while !response.receiver.incoming_resources.is_empty() {
        rounds += 1;
        assert!(
            rounds <= 32,
            "watchdogs must exhaust their bounded retry budgets"
        );
        let due = response
            .receiver
            .incoming_resources
            .earliest_timeout_at()
            .unwrap();
        assert!(due.0 >= at);
        at = due.0;
        response.receiver.fire_due_resource_deadlines(
            due,
            &mut |bytes| bytes.fill(0xCB),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    let frame = filled_frame(fill).unwrap();
                    assert_eq!(
                        WirePacketHeader::parse(&frame).unwrap().0.context,
                        WireContext::ResourceRequest
                    );
                }
                EngineReaction::Journaled(Journaled::ResourceFailed { hash, cause, .. }) => {
                    observed.failed.push((hash, cause))
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    observed.settlements.push((id, settlement))
                }
                _ => panic!("silent transfer expiry must only retry or fail"),
            },
        );
    }
    observed.failed.sort_by_key(|(hash, _)| *hash.as_bytes());
    let mut expected_failures = std::vec![
        (whole_hash, ResourceFailureCause::OpenTimedOut),
        (continuation_hash, ResourceFailureCause::RetriesExhausted)
    ];
    expected_failures.sort_by_key(|(hash, _)| *hash.as_bytes());
    assert_eq!(
        observed,
        InboundCapture {
            failed: expected_failures,
            settlements: std::vec![(
                REQUEST,
                Settlement::SendRequest(Err(SendRequestFailure::ResponseTransferFailed(
                    ResourceFailureCause::RetriesExhausted
                )))
            )],
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
    at + 100
}
