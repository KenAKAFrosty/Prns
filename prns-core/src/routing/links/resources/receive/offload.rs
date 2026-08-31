//! The receive-side pool seam, mirror of the staged seal's ([`send`](super::super::send)):
//! [`owed_open_span`](EngineState::owed_open_span) names a row whose parked streamed open
//! trails the frontier, [`open_span_job_view`](EngineState::open_span_job_view) +
//! [`begin_open_chew`](EngineState::begin_open_chew) hand a worker its span and state, and
//! [`apply_opened_span`](EngineState::apply_opened_span) lands the verdict — re-parking the
//! state, or concluding a transfer that finished arriving while the worker chewed.

use crate::engine::{EngineReaction, EngineState, InstantMillis, WakeSchedules};
use crate::routing::links::resources::streamed_open::{OpenProgress, StreamedOpen};
use crate::routing::links::resources::table::IncomingResourceStatus;
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;

/// What a pool chew job copies before [`begin_open_chew`](EngineState::begin_open_chew) parks
/// the row as `Chewing`: the row's identity and the next contiguous ciphertext span.
pub struct OpenSpanJobView<'a> {
    pub link_id: LinkId,
    pub hash: ResourceHash,
    pub span_start: usize,
    pub bytes: &'a [u8],
}

/// A worker's finished chew, exactly as it returns: the identity that finds the row, the
/// state that re-parks on it, and the decrypted bytes that land back over their ciphertext.
pub struct OffloadedOpenSpan<'a> {
    pub link_id: LinkId,
    pub hash: ResourceHash,
    pub span_start: usize,
    pub state: StreamedOpen,
    pub bytes: &'a [u8],
}

impl<S: StorageLayout> EngineState<S> {
    /// The next incoming row whose parked open trails its frontier — one chew is dispatchable
    /// per row at a time (the spans chain through the state), and a row already `Chewing`
    /// waits for its verdict.
    pub fn owed_open_span(&self) -> Option<(LinkId, ResourceHash)> {
        (0..self.incoming_resources.len()).find_map(|index| {
            let state = self.incoming_resources.state(index);
            if state.status == IncomingResourceStatus::AwaitingDecompression {
                return None;
            }
            let (_, slot) = self.incoming_resources.transfer_and_streamed_open(index);
            let OpenProgress::Parked(open) = slot else {
                return None;
            };
            (!open.pending_span(state.contiguous_byte_len()).is_empty()).then(|| {
                (
                    *self.incoming_resources.link_at(index),
                    *self.incoming_resources.hash_at(index),
                )
            })
        })
    }

    /// The owed chew's worker inputs, borrowed for the runtime to copy into a pool job;
    /// [`begin_open_chew`](Self::begin_open_chew) then moves the state out to ride with them.
    pub fn open_span_job_view(
        &self,
        link_id: &LinkId,
        hash: &ResourceHash,
    ) -> Option<OpenSpanJobView<'_>> {
        let index = self.incoming_resources.lookup(link_id, hash)?;
        let contiguous = self.incoming_resources.state(index).contiguous_byte_len();
        let (transfer, slot) = self.incoming_resources.transfer_and_streamed_open(index);
        let OpenProgress::Parked(open) = slot else {
            return None;
        };
        let span = open.pending_span(contiguous);
        if span.is_empty() {
            return None;
        }
        Some(OpenSpanJobView {
            link_id: *link_id,
            hash: *hash,
            span_start: span.start,
            bytes: &transfer[span],
        })
    }

    /// Move the parked state out for the worker, leaving the dispatched span behind as the
    /// verdict's identity check.
    pub fn begin_open_chew(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
    ) -> Option<StreamedOpen> {
        let index = self.incoming_resources.lookup(link_id, hash)?;
        let contiguous = self.incoming_resources.state(index).contiguous_byte_len();
        let (_, slot) = self
            .incoming_resources
            .transfer_and_streamed_open_mut(index);
        let OpenProgress::Parked(open) = core::mem::take(slot) else {
            return None;
        };
        let span = open.pending_span(contiguous);
        if span.is_empty() {
            *slot = OpenProgress::Parked(open);
            return None;
        }
        *slot = OpenProgress::Chewing { dispatched: span };
        Some(open)
    }

    /// A worker's span verdict, landing only on the row still marked with exactly this span —
    /// one for a row that died or was replaced mid-chew drops silently. The impossible near-miss
    /// (a replacement row that dispatched the identical span within one pool round trip) still
    /// cannot deliver wrong bytes: the returned state's MAC midstate would refuse the mismatched
    /// transfer at its conclusion.
    ///
    /// A transfer that finished arriving while the worker chewed parked as `AwaitingOpen`; its
    /// verdict concludes it here, chewing any small remainder inline — the proof is gated on it
    /// and the engine thread has nothing else to run first.
    pub fn apply_opened_span(
        &mut self,
        verdict: OffloadedOpenSpan<'_>,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_, crate::engine::OwedWork<'_>>),
    ) -> WakeSchedules {
        let OffloadedOpenSpan {
            link_id,
            hash,
            span_start,
            state,
            bytes,
        } = verdict;
        let mut wake_schedule_changes = WakeSchedules::UNCHANGED;
        let Some(index) = self.incoming_resources.lookup(&link_id, &hash) else {
            return wake_schedule_changes;
        };
        {
            let (transfer, slot) = self
                .incoming_resources
                .transfer_and_streamed_open_mut(index);
            let expected = span_start..span_start + bytes.len();
            let OpenProgress::Chewing { dispatched } = slot else {
                return wake_schedule_changes;
            };
            if *dispatched != expected {
                return wake_schedule_changes;
            }
            transfer[expected].copy_from_slice(bytes);
            *slot = OpenProgress::Parked(state);
        }
        if self.incoming_resources.state(index).status == IncomingResourceStatus::AwaitingOpen {
            self.conclude_resource(&link_id, &hash, now, sink);
            wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
        }
        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
        wake_schedule_changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::{CommandId, Directive, Journaled, Settlement};
    use crate::routing::links::resources::receive::tests_support::*;
    use crate::routing::links::resources::streamed_open::ResourceOpenLane;
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

    fn decoy_payload() -> std::vec::Vec<u8> {
        b"a second live open keeps the pool lane contended! ".repeat(31)
    }

    /// Stand up the contention [`ResourceOpenLane::PoolWhenContended`] gates on: a second
    /// incoming transfer (its own sender, same link) with a begun open and parts still owed.
    /// The returned frames are its remaining parts — feeding them retires the decoy.
    fn park_a_decoy_open(
        receiver: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
    ) -> std::vec::Vec<(crate::interfaces::InterfaceId, std::vec::Vec<u8>)> {
        let mut decoy_sender = engine_with_active_link();
        let advertisement = advertise(&mut decoy_sender, &decoy_payload(), 1_000);
        let pull = feed(receiver, &advertisement, 1_100);
        let serve = feed(&mut decoy_sender, &pull.frames[0].1, 1_200);
        feed(receiver, &serve.frames[0].1, 1_300);
        serve.frames[1..].to_vec()
    }

    #[test]
    fn a_transfer_completing_mid_chew_parks_and_the_verdict_concludes_it() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        receiver.resource_open_lane = ResourceOpenLane::PoolWhenContended;
        accept_everything(&mut receiver);
        park_a_decoy_open(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(serve.frames.len(), 4);

        assert!(
            receiver.owed_open_span().is_none(),
            "nothing is owed before a part lands",
        );
        let first = feed(&mut receiver, &serve.frames[0].1, 2_200);
        assert!(first.received.is_empty());
        let (link_id, hash) = receiver
            .owed_open_span()
            .expect("the first placed part leaves its span parked for the pool");

        let view = receiver.open_span_job_view(&link_id, &hash).unwrap();
        let span_start = view.span_start;
        let mut bytes = view.bytes.to_vec();
        let mut state = receiver.begin_open_chew(&link_id, &hash).unwrap();
        assert!(
            receiver.owed_open_span().is_none(),
            "a chewing row waits for its verdict before the next span",
        );

        for (arrived, (_, part)) in serve.frames[1..].iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_300 + arrived as u64);
            assert!(
                capture.received.is_empty(),
                "no delivery before the verdict"
            );
        }
        let index = receiver.incoming_resources.lookup(&link_id, &hash).unwrap();
        assert_eq!(
            receiver.incoming_resources.state(index).status,
            IncomingResourceStatus::AwaitingOpen,
            "a transfer completing mid-chew parks for the pool",
        );

        state.chew_span(&mut bytes);
        let mut frames = std::vec::Vec::new();
        let mut received = std::vec::Vec::new();
        receiver.apply_opened_span(
            OffloadedOpenSpan {
                link_id,
                hash,
                span_start,
                state,
                bytes: &bytes,
            },
            crate::engine::InstantMillis(2_400),
            &mut |reaction| match reaction {
                crate::engine::EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        frames.push(frame);
                    }
                }
                crate::engine::EngineReaction::Journaled(Journaled::ResourceReceived {
                    data,
                    ..
                }) => received.push(data.to_vec()),
                _ => {}
            },
        );
        assert_eq!(
            received,
            [data],
            "the verdict concludes the parked transfer"
        );
        assert_eq!(frames.len(), 1, "the proof rides back");
        assert!(receiver
            .incoming_resources
            .lookup(&link_id, &hash)
            .is_none());

        let settled = feed(&mut sender, &frames[0], 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
    }

    #[test]
    fn a_verdict_for_a_dead_or_mismatched_row_drops_silently() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        receiver.resource_open_lane = ResourceOpenLane::PoolWhenContended;
        accept_everything(&mut receiver);
        park_a_decoy_open(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        feed(&mut receiver, &serve.frames[0].1, 2_200);

        let (link_id, hash) = receiver.owed_open_span().unwrap();
        let view = receiver.open_span_job_view(&link_id, &hash).unwrap();
        let span_start = view.span_start;
        let mut bytes = view.bytes.to_vec();
        let mut state = receiver.begin_open_chew(&link_id, &hash).unwrap();
        state.chew_span(&mut bytes);

        let wrong_span = receiver.apply_opened_span(
            OffloadedOpenSpan {
                link_id,
                hash,
                span_start: span_start + 16,
                state,
                bytes: &bytes[16..],
            },
            crate::engine::InstantMillis(2_250),
            &mut |_| panic!("a mismatched span touches nothing"),
        );
        assert_eq!(wrong_span, crate::engine::WakeSchedules::UNCHANGED);
        let index = receiver.incoming_resources.lookup(&link_id, &hash).unwrap();
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
    fn a_pool_that_never_answers_fails_the_parked_transfer_at_its_grace_deadline() {
        use crate::engine::WakeSchedule;
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        receiver.resource_open_lane = ResourceOpenLane::PoolWhenContended;
        accept_everything(&mut receiver);
        let decoy_parts = park_a_decoy_open(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        feed(&mut receiver, &serve.frames[0].1, 2_200);
        let (link_id, hash) = receiver.owed_open_span().unwrap();
        receiver.begin_open_chew(&link_id, &hash).unwrap();

        for (arrived, (_, part)) in decoy_parts.iter().enumerate() {
            feed(&mut receiver, part, 2_210 + arrived as u64);
        }
        for (arrived, (_, part)) in serve.frames[1..].iter().enumerate() {
            feed(&mut receiver, part, 2_300 + arrived as u64);
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
    fn a_contended_pool_no_one_walks_still_delivers_at_the_conclusion() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        receiver.resource_open_lane = ResourceOpenLane::PoolWhenContended;
        accept_everything(&mut receiver);
        park_a_decoy_open(&mut receiver);
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
        let conclusion = conclusion.expect("the conclusion's catch-up chews the whole backlog");
        assert_eq!(conclusion.received[0].1, data);
        let (link_id, hash) = (link_id(), conclusion.received[0].0);
        assert!(receiver
            .incoming_resources
            .lookup(&link_id, &hash)
            .is_none());
    }

    #[test]
    fn a_lone_transfer_on_the_pool_lane_chews_inline() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        receiver.resource_open_lane = ResourceOpenLane::PoolWhenContended;
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            assert!(
                receiver.owed_open_span().is_none(),
                "a lone open never parks a span for the pool",
            );
            if !capture.received.is_empty() {
                conclusion = Some(capture);
            }
        }
        assert_eq!(conclusion.expect("delivered inline").received[0].1, data);
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn the_last_contender_standing_returns_to_the_inline_chew() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        receiver.resource_open_lane = ResourceOpenLane::PoolWhenContended;
        accept_everything(&mut receiver);
        let decoy_parts = park_a_decoy_open(&mut receiver);
        let data = four_part_payload();

        let advertisement = advertise(&mut sender, &data, 1_500);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        feed(&mut receiver, &serve.frames[0].1, 2_200);
        assert!(
            receiver.owed_open_span().is_some(),
            "a contended open parks its spans for the pool",
        );

        for (arrived, (_, part)) in decoy_parts.iter().enumerate() {
            feed(&mut receiver, part, 2_210 + arrived as u64);
        }
        feed(&mut receiver, &serve.frames[1].1, 2_300);
        assert!(
            receiver.owed_open_span().is_none(),
            "the survivor's next advance catches the backlog up inline",
        );

        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames[2..].iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_400 + arrived as u64);
            if !capture.received.is_empty() {
                conclusion = Some(capture);
            }
        }
        assert_eq!(conclusion.expect("delivered inline").received[0].1, data);
        assert!(receiver.incoming_resources.is_empty());
    }
}
