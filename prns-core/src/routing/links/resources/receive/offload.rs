//! The receive-side streamed-open continuation. Core emits a typed [`ResourceOpenOwed`] directive
//! containing the exact mutable span and its owned crypto midstate. A runtime may chew that borrow
//! inline or materialize an owning worker job, then returns [`ResourceOpenCompleted`] through
//! [`EngineState::resume_resource_open`].

use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, OpenedResourceSpan, OwedWork,
    ResourceOpenCompleted, ResourceOpenOwed, WakeSchedules, WholeResourceOpenCompleted,
    WholeResourceOpenLanding, WholeResourceOpenOutcome,
};
use crate::routing::links::resources::streamed_open::{ExternalOpenVerification, OpenProgress};
use crate::routing::links::resources::table::IncomingResourceStatus;
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;

impl<S: StorageLayout> EngineState<S> {
    pub fn resume_whole_resource_open(
        &mut self,
        completed: WholeResourceOpenCompleted<'_>,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_, OwedWork<'_>>),
    ) -> WholeResourceOpenLanding {
        let reservation = completed.reservation;
        let Some(index) = self
            .incoming_resources
            .lookup(&reservation.link_id, &reservation.hash)
        else {
            return WholeResourceOpenLanding::Stale;
        };
        let state = *self.incoming_resources.state(index);
        if state.status != IncomingResourceStatus::AwaitingOpen
            || state.open_generation != Some(reservation.generation)
        {
            return WholeResourceOpenLanding::Stale;
        }
        let (_, open) = self.incoming_resources.transfer_and_streamed_open(index);
        if !matches!(
            open,
            OpenProgress::Chewing { dispatched }
                if *dispatched == (0..state.sealed_transfer_bytes)
        ) {
            return WholeResourceOpenLanding::Stale;
        }
        match completed.outcome {
            WholeResourceOpenOutcome::Opened(plaintext) => {
                if plaintext.len() < crate::routing::links::resources::RESOURCE_NONCE_LEN
                    || plaintext.len() > state.sealed_transfer_bytes
                {
                    return WholeResourceOpenLanding::Invalid;
                }
                let (transfer, open) = self
                    .incoming_resources
                    .transfer_and_streamed_open_mut(index);
                transfer[..plaintext.len()].copy_from_slice(plaintext);
                *open = OpenProgress::ExternallyOpened {
                    plaintext_byte_len: plaintext.len(),
                    verification: ExternalOpenVerification::Rehash,
                };
                self.incoming_resources.state_mut(index).open_generation = None;
                self.conclude_resource(&reservation.link_id, &reservation.hash, now, sink);
            }
            WholeResourceOpenOutcome::OpenedAndDigested {
                plaintext,
                calculated_hash,
                proof,
            } => {
                if state.compression
                    != crate::routing::links::resources::ResourceCompression::Uncompressed
                    || plaintext.len() < crate::routing::links::resources::RESOURCE_NONCE_LEN
                    || plaintext.len() > state.sealed_transfer_bytes
                    || calculated_hash != reservation.hash
                {
                    return WholeResourceOpenLanding::Invalid;
                }
                let (transfer, open) = self
                    .incoming_resources
                    .transfer_and_streamed_open_mut(index);
                transfer[..plaintext.len()].copy_from_slice(plaintext);
                *open = OpenProgress::ExternallyOpened {
                    plaintext_byte_len: plaintext.len(),
                    verification: ExternalOpenVerification::Verified(proof),
                };
                self.incoming_resources.state_mut(index).open_generation = None;
                self.conclude_resource(&reservation.link_id, &reservation.hash, now, sink);
            }
            WholeResourceOpenOutcome::Refused => {
                self.fail_incoming_resource(
                    &reservation.link_id,
                    &reservation.hash,
                    crate::routing::links::resources::ResourceFailureCause::TransferUnopenable,
                    sink,
                );
            }
            WholeResourceOpenOutcome::Unavailable => {
                {
                    let state = self.incoming_resources.state_mut(index);
                    state.status = IncomingResourceStatus::Transferring;
                    state.open_generation = None;
                }
                let (_, open) = self
                    .incoming_resources
                    .transfer_and_streamed_open_mut(index);
                *open = OpenProgress::NotBegun;
                let lane = core::mem::replace(
                    &mut self.resource_open_lane,
                    crate::routing::links::resources::streamed_open::ResourceOpenLane::EngineDirected,
                );
                self.conclude_resource(&reservation.link_id, &reservation.hash, now, sink);
                self.resource_open_lane = lane;
            }
        }
        WholeResourceOpenLanding::Applied
    }
}

impl<S: StorageLayout> EngineState<S> {
    /// Emit the next contiguous open for this row, moving its midstate into the directive and
    /// marking the exact borrowed span as in flight in one transition.
    pub(crate) fn emit_resource_open(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        sink: &mut impl FnMut(EngineReaction<'_, OwedWork<'_>>),
    ) {
        let Some(index) = self.incoming_resources.lookup(link_id, hash) else {
            return;
        };
        if self.incoming_resources.state(index).status
            == IncomingResourceStatus::AwaitingDecompression
        {
            return;
        }
        let other_transfers_in_flight = self.incoming_resources.len() > 1;
        let contiguous = self.incoming_resources.state(index).contiguous_byte_len();
        let (transfer, slot) = self
            .incoming_resources
            .transfer_and_streamed_open_mut(index);
        if !matches!(slot, OpenProgress::Parked(_)) {
            return;
        }
        let OpenProgress::Parked(open) = core::mem::take(slot) else {
            return;
        };
        let span = open.pending_span(contiguous);
        if span.is_empty() {
            *slot = OpenProgress::Parked(open);
            return;
        }
        let span_start = span.start;
        *slot = OpenProgress::Chewing {
            dispatched: span.clone(),
        };
        sink(EngineReaction::Directive(Directive::Fulfill(
            OwedWork::ResourceOpen(ResourceOpenOwed {
                link_id: *link_id,
                hash: *hash,
                span_start,
                state: open,
                bytes: &mut transfer[span],
                other_transfers_in_flight,
            }),
        )));
    }

    /// Land a fulfilled open only on the row still marked with exactly this span. A completion
    /// for a retired, replaced, or differently advanced row is stale and has no effect.
    ///
    /// A transfer that finished arriving while the worker chewed parked as `AwaitingOpen`; its
    /// verdict concludes it here, chewing any small remainder inline — the proof is gated on it
    /// and the engine thread has nothing else to run first.
    pub fn resume_resource_open(
        &mut self,
        completed: ResourceOpenCompleted<'_>,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_, OwedWork<'_>>),
    ) -> WakeSchedules {
        let ResourceOpenCompleted {
            link_id,
            hash,
            span_start,
            state,
            opened,
        } = completed;
        let mut wake_schedule_changes = WakeSchedules::UNCHANGED;
        let Some(index) = self.incoming_resources.lookup(&link_id, &hash) else {
            return wake_schedule_changes;
        };
        {
            let (transfer, slot) = self
                .incoming_resources
                .transfer_and_streamed_open_mut(index);
            let byte_len = match &opened {
                OpenedResourceSpan::InPlace { byte_len } => *byte_len,
                OpenedResourceSpan::Returned(bytes) => bytes.len(),
            };
            let Some(span_end) = span_start.checked_add(byte_len) else {
                return wake_schedule_changes;
            };
            let expected = span_start..span_end;
            let OpenProgress::Chewing { dispatched } = slot else {
                return wake_schedule_changes;
            };
            if *dispatched != expected {
                return wake_schedule_changes;
            }
            if let OpenedResourceSpan::Returned(bytes) = opened {
                transfer[expected].copy_from_slice(bytes);
            }
            *slot = OpenProgress::Parked(state);
        }
        if self.incoming_resources.state(index).status == IncomingResourceStatus::AwaitingOpen {
            self.conclude_resource(&link_id, &hash, now, sink);
            wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
        } else {
            self.emit_resource_open(&link_id, &hash, sink);
        }
        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
        wake_schedule_changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{filled_frame, routable_descriptor};
    use crate::engine::{
        CommandId, Directive, IngestIo, Journaled, OpenedResourceSpan, ResourceOpenCompleted,
        Settlement, WholeResourceOpenPlan,
    };
    use crate::interfaces::{AttachedInterfaces, InboundPacket};
    use crate::routing::links::resources::receive::tests_support::*;
    use crate::routing::links::resources::streamed_open::{ResourceOpenLane, StreamedOpen};
    use crate::routing::links::resources::table::IncomingResourceStatus;
    use crate::routing::links::resources::{
        ResourceBody, ResourceFailureCause, ResourceMetadata, ResourceSend, OPEN_VERDICT_GRACE_MS,
    };

    fn advertise(
        sender: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        data: &[u8],
        at: u64,
    ) -> std::vec::Vec<u8> {
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            crate::engine::InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let crate::engine::EngineReaction::Directive(Directive::EmitFrame {
                    fill, ..
                }) = reaction
                {
                    advertisement = filled_frame(fill);
                }
            },
        );
        advertisement.expect("the sender advertises")
    }

    struct OwnedOpenJob {
        link_id: LinkId,
        hash: ResourceHash,
        span_start: usize,
        state: StreamedOpen,
        bytes: std::vec::Vec<u8>,
        other_transfers_in_flight: bool,
    }

    struct OwnedWholeOpenJob {
        plan: WholeResourceOpenPlan,
        sealed: std::vec::Vec<u8>,
    }

    fn feed_deferring_whole_open(
        receiver: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        frame: &[u8],
        at: u64,
    ) -> Option<OwnedWholeOpenJob> {
        let mut job = None;
        let mut raw = frame.to_vec();
        receiver.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(at),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[routable_descriptor(lane())]),
                now: InstantMillis(at),
                fill_random: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Directive(Directive::Fulfill(
                        OwedWork::WholeResourceOpen(owed),
                    )) = reaction
                    {
                        job = Some(OwnedWholeOpenJob {
                            sealed: owed.sealed().to_vec(),
                            plan: owed.into_plan(),
                        });
                    }
                },
            },
        );
        job
    }

    fn park_external_transfer(
        sender: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        receiver: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        data: &[u8],
    ) -> OwnedWholeOpenJob {
        receiver.resource_open_lane = ResourceOpenLane::ExternalWhole;
        accept_everything(receiver);
        let advertisement = advertise(sender, data, 1_500);
        let pull = feed(receiver, &advertisement, 2_000);
        let serve = feed(sender, &pull.frames[0].1, 2_100);
        let mut job = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let next = feed_deferring_whole_open(receiver, part, 2_200 + arrived as u64);
            assert!(job.is_none() || next.is_none());
            if next.is_some() {
                job = next;
            }
        }
        job.expect("the completed transfer owes one whole-resource open")
    }

    #[test]
    fn an_external_whole_open_delivers_and_proves() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        let data = four_part_payload();
        let job = park_external_transfer(&mut sender, &mut receiver, &data);
        let mut sealed = job.sealed;
        let plaintext = link_key().open_in_place(&mut sealed).unwrap().to_vec();

        let mut frames = std::vec::Vec::new();
        let mut received = std::vec::Vec::new();
        let landing = receiver.resume_whole_resource_open(
            WholeResourceOpenCompleted {
                reservation: job.plan.reservation(),
                outcome: WholeResourceOpenOutcome::Opened(&plaintext),
            },
            InstantMillis(2_500),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        frames.push(frame);
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceReceived { data, .. }) => {
                    received.push(data.to_vec());
                }
                _ => {}
            },
        );

        assert_eq!(landing, WholeResourceOpenLanding::Applied);
        assert_eq!(received, [data]);
        assert_eq!(frames.len(), 1);
        assert!(receiver.incoming_resources.is_empty());
        let settled = feed(&mut sender, &frames[0], 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
    }

    #[test]
    fn an_externally_verified_open_delivers_with_the_supplied_proof() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        let data = four_part_payload();
        let job = park_external_transfer(&mut sender, &mut receiver, &data);
        let hash = job.plan.hash();
        let salt_nonce = job.plan.salt_nonce();
        let mut sealed = job.sealed;
        let plaintext = link_key().open_in_place(&mut sealed).unwrap().to_vec();
        let proof = crate::routing::links::resources::assemble_incoming::verify_and_prove(
            &plaintext[crate::routing::links::resources::RESOURCE_NONCE_LEN..],
            &salt_nonce,
            &hash,
        )
        .unwrap();
        let mut frames = std::vec::Vec::new();
        let mut received = std::vec::Vec::new();
        let landing = receiver.resume_whole_resource_open(
            WholeResourceOpenCompleted {
                reservation: job.plan.reservation(),
                outcome: WholeResourceOpenOutcome::OpenedAndDigested {
                    plaintext: &plaintext,
                    calculated_hash: hash,
                    proof,
                },
            },
            InstantMillis(2_500),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        frames.push(frame);
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceReceived { data, .. }) => {
                    received.push(data.to_vec());
                }
                _ => {}
            },
        );

        assert_eq!(landing, WholeResourceOpenLanding::Applied);
        assert_eq!(received, [data]);
        assert_eq!(frames.len(), 1);
        let settled = feed(&mut sender, &frames[0], 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
    }

    #[test]
    fn an_externally_verified_open_rejects_a_mismatched_calculated_hash() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        let data = four_part_payload();
        let job = park_external_transfer(&mut sender, &mut receiver, &data);
        let hash = job.plan.hash();
        let mut sealed = job.sealed;
        let plaintext = link_key().open_in_place(&mut sealed).unwrap().to_vec();

        let mut failed = std::vec::Vec::new();
        let landing = receiver.resume_whole_resource_open(
            WholeResourceOpenCompleted {
                reservation: job.plan.reservation(),
                outcome: WholeResourceOpenOutcome::OpenedAndDigested {
                    plaintext: &plaintext,
                    calculated_hash: ResourceHash::new([0xA7; 32]),
                    proof: crate::routing::links::resources::ResourceProof::new([0xB8; 32]),
                },
            },
            InstantMillis(2_500),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::ResourceFailed { cause, .. }) = reaction
                {
                    failed.push(cause);
                }
            },
        );

        assert_eq!(landing, WholeResourceOpenLanding::Invalid);
        assert!(failed.is_empty());
        assert!(receiver
            .incoming_resources
            .lookup(&job.plan.link_id(), &hash)
            .is_some());
    }

    #[test]
    fn an_external_open_refusal_fails_only_the_reserved_job() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        let data = four_part_payload();
        let job = park_external_transfer(&mut sender, &mut receiver, &data);
        let mut failed = std::vec::Vec::new();

        let landing = receiver.resume_whole_resource_open(
            WholeResourceOpenCompleted {
                reservation: job.plan.reservation(),
                outcome: WholeResourceOpenOutcome::Refused,
            },
            InstantMillis(2_500),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::ResourceFailed { cause, .. }) = reaction
                {
                    failed.push(cause);
                }
            },
        );

        assert_eq!(landing, WholeResourceOpenLanding::Applied);
        assert_eq!(failed, [ResourceFailureCause::TransferUnopenable]);
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn an_external_open_runtime_failure_retries_the_same_row_inline() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        let data = four_part_payload();
        let job = park_external_transfer(&mut sender, &mut receiver, &data);

        let mut received = std::vec::Vec::new();
        let landing = receiver.resume_whole_resource_open(
            WholeResourceOpenCompleted {
                reservation: job.plan.reservation(),
                outcome: WholeResourceOpenOutcome::Unavailable,
            },
            InstantMillis(2_500),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::ResourceReceived { data, .. }) =
                    reaction
                {
                    received.push(data.to_vec());
                }
            },
        );

        assert_eq!(landing, WholeResourceOpenLanding::Applied);
        assert_eq!(received, [data]);
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(receiver.resource_open_lane, ResourceOpenLane::ExternalWhole);
    }

    fn feed_deferring_open(
        receiver: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        frame: &[u8],
        at: u64,
    ) -> std::vec::Vec<OwnedOpenJob> {
        let mut jobs = std::vec::Vec::new();
        let mut raw = frame.to_vec();
        receiver.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(at),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[routable_descriptor(lane())]),
                now: InstantMillis(at),
                fill_random: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Directive(Directive::Fulfill(OwedWork::ResourceOpen(
                        owed,
                    ))) = reaction
                    {
                        jobs.push(OwnedOpenJob {
                            link_id: owed.link_id,
                            hash: owed.hash,
                            span_start: owed.span_start,
                            state: owed.state,
                            bytes: owed.bytes.to_vec(),
                            other_transfers_in_flight: owed.other_transfers_in_flight,
                        });
                    }
                },
            },
        );
        jobs
    }

    fn park_incomplete_transfer(
        receiver: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
    ) {
        let mut sender = engine_with_active_link();
        let data = b"a second live resource supplies real overlap ".repeat(40);
        let advertisement = advertise(&mut sender, &data, 1_000);
        let pull = feed(receiver, &advertisement, 1_100);
        let serve = feed(&mut sender, &pull.frames[0].1, 1_200);
        feed(receiver, &serve.frames[0].1, 1_300);
        assert!(!receiver.incoming_resources.is_empty());
    }

    #[test]
    fn owed_work_reports_real_overlap_without_prescribing_a_lane() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        park_incomplete_transfer(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let job = feed_deferring_open(&mut receiver, &serve.frames[0].1, 2_200)
            .pop()
            .expect("the new row owes its first open span");

        assert!(job.other_transfers_in_flight);
    }

    #[test]
    fn a_transfer_completing_while_work_is_owed_parks_until_resume() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(serve.frames.len(), 4);

        let mut jobs = feed_deferring_open(&mut receiver, &serve.frames[0].1, 2_200);
        assert_eq!(jobs.len(), 1, "the packet step emits one typed open");
        let mut job = jobs.pop().unwrap();
        assert!(!job.other_transfers_in_flight);

        for (arrived, (_, part)) in serve.frames[1..].iter().enumerate() {
            assert!(feed_deferring_open(&mut receiver, part, 2_300 + arrived as u64).is_empty());
        }
        let index = receiver
            .incoming_resources
            .lookup(&job.link_id, &job.hash)
            .unwrap();
        assert_eq!(
            receiver.incoming_resources.state(index).status,
            IncomingResourceStatus::AwaitingOpen,
            "completion waits for the explicitly owed work",
        );

        job.state.chew_span(&mut job.bytes);
        let mut frames = std::vec::Vec::new();
        let mut received = std::vec::Vec::new();
        receiver.resume_resource_open(
            ResourceOpenCompleted {
                link_id: job.link_id,
                hash: job.hash,
                span_start: job.span_start,
                state: job.state,
                opened: OpenedResourceSpan::Returned(&job.bytes),
            },
            InstantMillis(2_400),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        frames.push(frame);
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceReceived { data, .. }) => {
                    received.push(data.to_vec());
                }
                _ => {}
            },
        );
        assert_eq!(received, [data]);
        assert_eq!(frames.len(), 1, "the proof rides back");
        assert!(receiver
            .incoming_resources
            .lookup(&job.link_id, &job.hash)
            .is_none());

        let settled = feed(&mut sender, &frames[0], 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
    }

    #[test]
    fn a_wrong_shape_completion_is_stale_and_leaves_the_real_work_owed() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut job = feed_deferring_open(&mut receiver, &serve.frames[0].1, 2_200)
            .pop()
            .unwrap();
        job.state.chew_span(&mut job.bytes);

        let wrong_span = receiver.resume_resource_open(
            ResourceOpenCompleted {
                link_id: job.link_id,
                hash: job.hash,
                span_start: job.span_start + 16,
                state: job.state,
                opened: OpenedResourceSpan::Returned(&job.bytes[16..]),
            },
            InstantMillis(2_250),
            &mut |_| panic!("a mismatched span touches nothing"),
        );
        assert_eq!(wrong_span, WakeSchedules::UNCHANGED);
        let index = receiver
            .incoming_resources
            .lookup(&job.link_id, &job.hash)
            .unwrap();
        assert!(
            matches!(
                receiver
                    .incoming_resources
                    .transfer_and_streamed_open(index)
                    .1,
                OpenProgress::Chewing { .. },
            ),
            "the row still waits for the real verdict",
        );
    }

    #[test]
    fn a_completion_for_a_retired_row_is_a_no_op() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut job = feed_deferring_open(&mut receiver, &serve.frames[0].1, 2_200)
            .pop()
            .unwrap();
        job.state.chew_span(&mut job.bytes);
        receiver.retire_incoming_resource(&job.link_id, &job.hash);

        let wake = receiver.resume_resource_open(
            ResourceOpenCompleted {
                link_id: job.link_id,
                hash: job.hash,
                span_start: job.span_start,
                state: job.state,
                opened: OpenedResourceSpan::Returned(&job.bytes),
            },
            InstantMillis(2_250),
            &mut |_| panic!("stale work emits nothing"),
        );

        assert_eq!(wake, WakeSchedules::UNCHANGED);
    }

    #[test]
    fn work_that_never_completes_fails_at_the_open_grace_deadline() {
        use crate::engine::WakeSchedule;
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(
            feed_deferring_open(&mut receiver, &serve.frames[0].1, 2_200).len(),
            1,
        );
        for (arrived, (_, part)) in serve.frames[1..].iter().enumerate() {
            assert!(feed_deferring_open(&mut receiver, part, 2_300 + arrived as u64).is_empty());
        }
        assert_eq!(
            receiver.resource_deadlines_wake(),
            WakeSchedule::At(crate::engine::InstantMillis(2_302 + OPEN_VERDICT_GRACE_MS)),
            "the parked conclusion holds the verdict grace deadline",
        );

        let mut failed = std::vec::Vec::new();
        receiver.fire_due_resource_deadlines(
            crate::engine::InstantMillis(2_302 + OPEN_VERDICT_GRACE_MS + 1),
            &mut |bytes: &mut [u8]| bytes.fill(0xF2),
            &mut |reaction| {
                if let crate::engine::EngineReaction::Journaled(Journaled::ResourceFailed {
                    cause,
                    ..
                }) = reaction
                {
                    failed.push(cause);
                }
            },
        );
        assert_eq!(failed, [ResourceFailureCause::OpenTimedOut]);
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn inline_fulfillment_uses_the_same_resume_boundary() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.received.is_empty() {
                conclusion = Some(capture);
            }
        }
        let conclusion = conclusion.expect("the inline test runtime resumes every typed open");
        assert_eq!(conclusion.received[0].1, data);
        assert!(receiver
            .incoming_resources
            .lookup(&link_id(), &conclusion.received[0].0)
            .is_none());
    }
}
