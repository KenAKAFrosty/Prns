//! The deadline watchdog: silent rounds re-request and shrink the window, exhausted retry budgets fail the transfer, and every retired slot bequeaths its window and rate to the link.

use super::rounds::{expected_inflight_bits_per_second, shrink_window_after_silent_round};
use crate::engine::{EngineReaction, EngineState, InstantMillis};
use crate::routing::links::resources::table::IncomingResourceStatus;
use crate::routing::links::resources::{ResourceFailureCause, ResourceHash};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.3.5's watchdog TRANSFERRING branch. A receiver that gives up goes silent, like the reference; the sender discovers through its own watchdog.
    pub(crate) fn fire_due_incoming_resources<F>(
        &mut self,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        while let Some(index) = self.incoming_resources.due_index(now) {
            let link_id = *self.incoming_resources.link_at(index);
            let hash = *self.incoming_resources.hash_at(index);
            let state = *self.incoming_resources.state(index);
            let expired = if state.status == IncomingResourceStatus::AwaitingDecompression {
                Some(ResourceFailureCause::DecompressionTimedOut)
            } else if state.status == IncomingResourceStatus::AwaitingOpen {
                Some(ResourceFailureCause::OpenTimedOut)
            } else if self.links.phase_for(&link_id).is_none() {
                Some(ResourceFailureCause::LinkVanished)
            } else if state.retries_left == 0 {
                Some(ResourceFailureCause::RetriesExhausted)
            } else {
                None
            };
            if let Some(cause) = expired {
                self.fail_incoming_resource(&link_id, &hash, cause, sink);
                continue;
            }
            {
                let state = self.incoming_resources.state_mut(index);
                shrink_window_after_silent_round(state);
                state.waiting_for_hmu = false;
                state.outstanding_part_count = 0;
                state.retries_left -= 1;
            }
            self.emit_resource_pull(&link_id, &hash, now, fill_entropy, sink);
        }
    }

    /// RNS 1.3.5 `Link.resource_concluded` stores the final window and expected rate for the next transfer to inherit, however this one ended.
    pub(crate) fn retire_incoming_resource(&mut self, link_id: &LinkId, hash: &ResourceHash) {
        if let Some(index) = self.incoming_resources.lookup(link_id, hash) {
            let state = *self.incoming_resources.state(index);
            let link_rtt_ms = match self.links.phase_for(link_id) {
                Some(LinkPhase::Active { rtt, .. }) => rtt.millis(),
                _ => 1,
            };
            let eifr = expected_inflight_bits_per_second(&state, link_rtt_ms);
            self.links
                .note_resource_concluded(link_id, state.window, eifr);
        }
        self.incoming_resources.remove(link_id, hash);
    }

    pub fn fire_due_resource_deadlines<F>(
        &mut self,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        self.fire_due_outgoing_resources(now, fill_entropy, sink);
        self.fire_due_incoming_resources(now, fill_entropy, sink);
        let mut wake_schedule_changes = crate::engine::WakeSchedules::UNCHANGED;
        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
        wake_schedule_changes
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::Directive;
    use crate::engine::{Journaled, WakeSchedule};
    use crate::routing::links::resources::receive::tests_support::*;
    use crate::routing::links::resources::PART_REQUEST_MAX_RETRIES;

    struct WatchCapture {
        frames: usize,
        failed: std::vec::Vec<ResourceFailureCause>,
    }

    fn fire(
        engine: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        at: u64,
    ) -> WatchCapture {
        let mut capture = WatchCapture {
            frames: 0,
            failed: std::vec::Vec::new(),
        };
        engine.fire_due_resource_deadlines(
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xF2),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if filled_frame(fill).is_some() {
                        capture.frames += 1;
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceFailed { cause, .. }) => {
                    capture.failed.push(cause);
                }
                _ => {}
            },
        );
        capture
    }

    #[test]
    fn a_starved_pull_shrinks_its_window_and_asks_again() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let pull = feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        assert_eq!(pull.frames.len(), 1);
        let bootstrap_eifr = 287 * 8_000 / 250;
        let unmeasured_wait = 4 * (464 * 8 * 3_000 / bootstrap_eifr);
        assert_eq!(
            receiver.resource_deadlines_wake(),
            WakeSchedule::At(InstantMillis(2_000 + unmeasured_wait + 250)),
            "an unmeasured pull waits three sdu of flight at the establishment-bootstrapped rate",
        );

        let retried = fire(&mut receiver, 2_000 + unmeasured_wait + 250);
        assert_eq!(retried.frames, 1, "the pull goes out again");
        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.window, 3, "the window eases down");
        assert_eq!(state.window_max, 8, "and its ceiling follows twice");
        assert_eq!(state.retries_left, 15);
        assert_eq!(
            receiver.resource_deadlines_wake(),
            WakeSchedule::At(InstantMillis(
                2_000 + unmeasured_wait + 250 + unmeasured_wait + 250 + 500,
            )),
            "the next deadline stretches by one per-retry delay",
        );
    }

    #[test]
    fn a_received_part_refills_the_retry_budget_like_the_reference() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let pull = feed(
            &mut receiver,
            &advertise_from(&mut sender, &four_part_payload(), None),
            2_000,
        );

        let bootstrap_eifr = 287 * 8_000 / 250;
        let unmeasured_wait = 4 * (464 * 8 * 3_000 / bootstrap_eifr);
        fire(&mut receiver, 2_000 + unmeasured_wait + 250);
        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        assert_eq!(receiver.incoming_resources.state(index).retries_left, 15);

        let serve = feed(&mut sender, &pull.frames[0].1, 30_000);
        feed(&mut receiver, &serve.frames[0].1, 30_100);
        assert_eq!(
            receiver.incoming_resources.state(index).retries_left,
            PART_REQUEST_MAX_RETRIES,
            "a placed part refills the budget so only consecutive dead rounds exhaust it",
        );
    }

    #[test]
    fn a_receiver_out_of_retries_goes_silent_and_fails() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        receiver.incoming_resources.state_mut(index).retries_left = 0;

        let gave_up = fire(&mut receiver, 60_000);
        assert_eq!(
            gave_up.frames, 0,
            "giving up sends nothing, like the reference"
        );
        assert_eq!(gave_up.failed, [ResourceFailureCause::RetriesExhausted]);
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(receiver.resource_deadlines_wake(), WakeSchedule::Idle);
    }
}
