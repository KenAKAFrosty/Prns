//! Transactional user-visible display blanking with confirmed request/feedback state.

use core::sync::atomic::{AtomicU32, Ordering};

use super::presentation::{MonotonicMillis, NonZeroDuration};

static NEXT_BLANKING_STATE_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayAutoOff {
    Enabled,
    Disabled,
}

impl DisplayAutoOff {
    const fn toggled(self) -> Self {
        match self {
            Self::Enabled => Self::Disabled,
            Self::Disabled => Self::Enabled,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayVisibility {
    Visible,
    Blanked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayBlankReason {
    DisplayOnly,
    SystemSleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayBlankingCommand {
    Blank,
    Restore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayButtonOutcome {
    ForwardToUi,
    WakeAndConsume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DisplayBlankingTarget {
    Visible,
    Blanked(DisplayBlankReason),
}

impl DisplayBlankingTarget {
    const fn visibility(self) -> DisplayVisibility {
        match self {
            Self::Visible => DisplayVisibility::Visible,
            Self::Blanked(_) => DisplayVisibility::Blanked,
        }
    }

    const fn command(self) -> DisplayBlankingCommand {
        match self {
            Self::Visible => DisplayBlankingCommand::Restore,
            Self::Blanked(_) => DisplayBlankingCommand::Blank,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct DisplayBlankingAttempt {
    state_id: u32,
    attempt_id: u64,
    command: DisplayBlankingCommand,
}

impl DisplayBlankingAttempt {
    #[must_use]
    pub const fn command(&self) -> DisplayBlankingCommand {
        self.command
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum DisplayBlankingDecision {
    Settled,
    Start(DisplayBlankingAttempt),
    RetryAt(MonotonicMillis),
}

#[derive(Debug, Eq, PartialEq)]
pub struct DisplayButtonDecision {
    outcome: DisplayButtonOutcome,
    blanking: DisplayBlankingDecision,
}

impl DisplayButtonDecision {
    #[must_use]
    pub const fn outcome(&self) -> DisplayButtonOutcome {
        self.outcome
    }

    #[must_use]
    pub const fn blanking(self) -> DisplayBlankingDecision {
        self.blanking
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayBufferKnowledge {
    Preserved,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayOperationOutcome {
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayBlankingResult {
    pub outcome: DisplayOperationOutcome,
    pub buffer_knowledge: DisplayBufferKnowledge,
}

#[derive(Debug, Eq, PartialEq)]
pub struct DisplayBlankingFeedback {
    decision: DisplayBlankingDecision,
    buffer_knowledge: DisplayBufferKnowledge,
}

impl DisplayBlankingFeedback {
    #[must_use]
    pub const fn decision(self) -> DisplayBlankingDecision {
        self.decision
    }

    #[must_use]
    pub const fn buffer_knowledge(&self) -> DisplayBufferKnowledge {
        self.buffer_knowledge
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayBlankingError {
    ForeignAttempt,
    MissingAttempt,
    StaleAttempt,
    TimeWentBackward,
    AttemptIdentityExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DisplayBlankingPhase {
    Settled {
        confirmed: DisplayBlankingTarget,
    },
    InFlight {
        confirmed: DisplayBlankingTarget,
        desired: DisplayBlankingTarget,
        attempt_id: u64,
        target: DisplayBlankingTarget,
    },
    RetryPending {
        confirmed: DisplayBlankingTarget,
        desired: DisplayBlankingTarget,
        not_before: MonotonicMillis,
    },
}

/// Confirmed request/feedback state for a user-blankable display.
///
/// Visibility changes only after matching hardware feedback. Explicit notice deadlines and
/// automatic blanking remain semantic requests until the platform owner completes them.
pub struct DisplayBlankingState {
    state_id: u32,
    next_attempt_id: u64,
    phase: DisplayBlankingPhase,
    auto_off: DisplayAutoOff,
    auto_off_after: NonZeroDuration,
    auto_off_deadline: Option<MonotonicMillis>,
    explicit_blank: Option<(MonotonicMillis, DisplayBlankReason)>,
    retry_backoff: NonZeroDuration,
    last_observed: MonotonicMillis,
}

impl DisplayBlankingState {
    #[must_use]
    pub fn new(
        now: MonotonicMillis,
        auto_off_after: NonZeroDuration,
        retry_backoff: NonZeroDuration,
    ) -> Self {
        Self {
            state_id: NEXT_BLANKING_STATE_ID.fetch_add(1, Ordering::Relaxed),
            next_attempt_id: 1,
            phase: DisplayBlankingPhase::Settled {
                confirmed: DisplayBlankingTarget::Visible,
            },
            auto_off: DisplayAutoOff::Enabled,
            auto_off_after,
            auto_off_deadline: Some(saturating_deadline(now, auto_off_after)),
            explicit_blank: None,
            retry_backoff,
            last_observed: now,
        }
    }

    #[must_use]
    pub const fn visibility(&self) -> DisplayVisibility {
        self.confirmed().visibility()
    }

    #[must_use]
    pub const fn blank_reason(&self) -> Option<DisplayBlankReason> {
        match self.confirmed() {
            DisplayBlankingTarget::Visible => None,
            DisplayBlankingTarget::Blanked(reason) => Some(reason),
        }
    }

    #[must_use]
    pub const fn auto_off(&self) -> DisplayAutoOff {
        self.auto_off
    }

    pub fn schedule_display_off(&mut self, at: MonotonicMillis) {
        self.schedule_blank(at, DisplayBlankReason::DisplayOnly);
    }

    pub fn schedule_system_sleep(&mut self, at: MonotonicMillis) {
        self.schedule_blank(at, DisplayBlankReason::SystemSleep);
    }

    pub fn request_visible(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<DisplayBlankingDecision, DisplayBlankingError> {
        self.observe(now)?;
        self.explicit_blank = None;
        self.request(DisplayBlankingTarget::Visible)
    }

    pub fn request_blanked(
        &mut self,
        reason: DisplayBlankReason,
        now: MonotonicMillis,
    ) -> Result<DisplayBlankingDecision, DisplayBlankingError> {
        self.observe(now)?;
        self.auto_off_deadline = None;
        self.explicit_blank = None;
        self.request(DisplayBlankingTarget::Blanked(reason))
    }

    pub fn tick(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<DisplayBlankingDecision, DisplayBlankingError> {
        self.observe(now)?;
        if let DisplayBlankingPhase::RetryPending {
            confirmed,
            desired,
            not_before,
        } = self.phase
        {
            if now < not_before {
                return Ok(DisplayBlankingDecision::RetryAt(not_before));
            }
            if confirmed.visibility() == desired.visibility() {
                self.phase = DisplayBlankingPhase::Settled { confirmed: desired };
                return Ok(DisplayBlankingDecision::Settled);
            }
            return self.start_attempt(confirmed, desired, desired);
        }

        if let Some((deadline, reason)) = self.explicit_blank {
            if now >= deadline {
                self.explicit_blank = None;
                self.auto_off_deadline = None;
                return self.request(DisplayBlankingTarget::Blanked(reason));
            }
        }
        if self
            .auto_off_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.auto_off_deadline = None;
            return self.request(DisplayBlankingTarget::Blanked(
                DisplayBlankReason::DisplayOnly,
            ));
        }
        Ok(DisplayBlankingDecision::Settled)
    }

    pub fn button_pressed(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<DisplayButtonDecision, DisplayBlankingError> {
        self.observe(now)?;
        let confirmed = self.confirmed();
        match confirmed {
            DisplayBlankingTarget::Visible => {
                self.explicit_blank = None;
                self.rearm_auto_off(now);
                let blanking = self.request(DisplayBlankingTarget::Visible)?;
                Ok(DisplayButtonDecision {
                    outcome: DisplayButtonOutcome::ForwardToUi,
                    blanking,
                })
            }
            DisplayBlankingTarget::Blanked(DisplayBlankReason::DisplayOnly) => {
                let blanking = self.request(DisplayBlankingTarget::Visible)?;
                Ok(DisplayButtonDecision {
                    outcome: DisplayButtonOutcome::WakeAndConsume,
                    blanking,
                })
            }
            DisplayBlankingTarget::Blanked(DisplayBlankReason::SystemSleep) => {
                Ok(DisplayButtonDecision {
                    outcome: DisplayButtonOutcome::ForwardToUi,
                    blanking: DisplayBlankingDecision::Settled,
                })
            }
        }
    }

    pub fn toggle_auto_off(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<DisplayAutoOff, DisplayBlankingError> {
        self.observe(now)?;
        self.auto_off = self.auto_off.toggled();
        if matches!(
            self.phase,
            DisplayBlankingPhase::Settled {
                confirmed: DisplayBlankingTarget::Visible
            }
        ) && self.explicit_blank.is_none()
        {
            self.rearm_auto_off(now);
        }
        Ok(self.auto_off)
    }

    pub fn complete(
        &mut self,
        attempt: DisplayBlankingAttempt,
        completed_at: MonotonicMillis,
        result: DisplayBlankingResult,
    ) -> Result<DisplayBlankingFeedback, DisplayBlankingError> {
        if attempt.state_id != self.state_id {
            return Err(DisplayBlankingError::ForeignAttempt);
        }
        if completed_at < self.last_observed {
            return Err(DisplayBlankingError::TimeWentBackward);
        }
        let DisplayBlankingPhase::InFlight {
            confirmed,
            desired,
            attempt_id,
            target,
        } = self.phase
        else {
            return Err(DisplayBlankingError::MissingAttempt);
        };
        if attempt_id != attempt.attempt_id {
            return Err(DisplayBlankingError::StaleAttempt);
        }
        self.last_observed = completed_at;

        let decision = match result.outcome {
            DisplayOperationOutcome::Succeeded => {
                let confirmed = target;
                if confirmed.visibility() == desired.visibility() {
                    self.phase = DisplayBlankingPhase::Settled { confirmed: desired };
                    if desired == DisplayBlankingTarget::Visible {
                        self.rearm_auto_off(completed_at);
                    }
                    DisplayBlankingDecision::Settled
                } else {
                    self.start_attempt(confirmed, desired, desired)?
                }
            }
            DisplayOperationOutcome::Failed => {
                if confirmed.visibility() == desired.visibility() {
                    self.phase = DisplayBlankingPhase::Settled { confirmed: desired };
                    DisplayBlankingDecision::Settled
                } else {
                    let not_before = saturating_deadline(completed_at, self.retry_backoff);
                    self.phase = DisplayBlankingPhase::RetryPending {
                        confirmed,
                        desired,
                        not_before,
                    };
                    DisplayBlankingDecision::RetryAt(not_before)
                }
            }
        };
        Ok(DisplayBlankingFeedback {
            decision,
            buffer_knowledge: result.buffer_knowledge,
        })
    }

    const fn confirmed(&self) -> DisplayBlankingTarget {
        match self.phase {
            DisplayBlankingPhase::Settled { confirmed }
            | DisplayBlankingPhase::InFlight { confirmed, .. }
            | DisplayBlankingPhase::RetryPending { confirmed, .. } => confirmed,
        }
    }

    fn request(
        &mut self,
        target: DisplayBlankingTarget,
    ) -> Result<DisplayBlankingDecision, DisplayBlankingError> {
        match self.phase {
            DisplayBlankingPhase::Settled { confirmed } => {
                if confirmed.visibility() == target.visibility() {
                    self.phase = DisplayBlankingPhase::Settled { confirmed: target };
                    Ok(DisplayBlankingDecision::Settled)
                } else {
                    self.start_attempt(confirmed, target, target)
                }
            }
            DisplayBlankingPhase::InFlight {
                confirmed,
                attempt_id,
                target: in_flight_target,
                ..
            } => {
                self.phase = DisplayBlankingPhase::InFlight {
                    confirmed,
                    desired: target,
                    attempt_id,
                    target: in_flight_target,
                };
                Ok(DisplayBlankingDecision::Settled)
            }
            DisplayBlankingPhase::RetryPending {
                confirmed,
                not_before,
                ..
            } => {
                if confirmed.visibility() == target.visibility() {
                    self.phase = DisplayBlankingPhase::Settled { confirmed: target };
                    Ok(DisplayBlankingDecision::Settled)
                } else {
                    self.phase = DisplayBlankingPhase::RetryPending {
                        confirmed,
                        desired: target,
                        not_before,
                    };
                    Ok(DisplayBlankingDecision::RetryAt(not_before))
                }
            }
        }
    }

    fn start_attempt(
        &mut self,
        confirmed: DisplayBlankingTarget,
        desired: DisplayBlankingTarget,
        target: DisplayBlankingTarget,
    ) -> Result<DisplayBlankingDecision, DisplayBlankingError> {
        let attempt_id = self.next_attempt_id;
        self.next_attempt_id = self
            .next_attempt_id
            .checked_add(1)
            .ok_or(DisplayBlankingError::AttemptIdentityExhausted)?;
        let attempt = DisplayBlankingAttempt {
            state_id: self.state_id,
            attempt_id,
            command: target.command(),
        };
        self.phase = DisplayBlankingPhase::InFlight {
            confirmed,
            desired,
            attempt_id,
            target,
        };
        Ok(DisplayBlankingDecision::Start(attempt))
    }

    fn observe(&mut self, now: MonotonicMillis) -> Result<(), DisplayBlankingError> {
        if now < self.last_observed {
            return Err(DisplayBlankingError::TimeWentBackward);
        }
        self.last_observed = now;
        Ok(())
    }

    fn schedule_blank(&mut self, at: MonotonicMillis, reason: DisplayBlankReason) {
        if self.confirmed().visibility() != DisplayVisibility::Visible {
            return;
        }
        self.explicit_blank = Some((at, reason));
        self.auto_off_deadline = None;
    }

    fn rearm_auto_off(&mut self, now: MonotonicMillis) {
        self.auto_off_deadline = match self.auto_off {
            DisplayAutoOff::Enabled => Some(saturating_deadline(now, self.auto_off_after)),
            DisplayAutoOff::Disabled => None,
        };
    }
}

fn saturating_deadline(now: MonotonicMillis, duration: NonZeroDuration) -> MonotonicMillis {
    MonotonicMillis::new(now.get().saturating_add(duration.get()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> DisplayBlankingState {
        DisplayBlankingState::new(
            MonotonicMillis::new(0),
            NonZeroDuration::new(60).unwrap(),
            NonZeroDuration::new(5).unwrap(),
        )
    }

    fn start(decision: DisplayBlankingDecision) -> DisplayBlankingAttempt {
        let DisplayBlankingDecision::Start(attempt) = decision else {
            panic!("expected a physical attempt");
        };
        attempt
    }

    fn result(outcome: DisplayOperationOutcome) -> DisplayBlankingResult {
        DisplayBlankingResult {
            outcome,
            buffer_knowledge: DisplayBufferKnowledge::Preserved,
        }
    }

    #[test]
    fn auto_off_commits_only_after_matching_hardware_success() {
        let mut state = state();
        let attempt = start(state.tick(MonotonicMillis::new(60)).unwrap());
        assert_eq!(attempt.command(), DisplayBlankingCommand::Blank);
        assert_eq!(state.visibility(), DisplayVisibility::Visible);
        state
            .complete(
                attempt,
                MonotonicMillis::new(61),
                result(DisplayOperationOutcome::Succeeded),
            )
            .unwrap();
        assert_eq!(state.visibility(), DisplayVisibility::Blanked);
        assert_eq!(state.blank_reason(), Some(DisplayBlankReason::DisplayOnly));
    }

    #[test]
    fn failed_restore_keeps_the_wake_press_consumed_through_retry() {
        let mut state = state();
        let blank = start(
            state
                .request_blanked(DisplayBlankReason::DisplayOnly, MonotonicMillis::new(1))
                .unwrap(),
        );
        state
            .complete(
                blank,
                MonotonicMillis::new(2),
                result(DisplayOperationOutcome::Succeeded),
            )
            .unwrap();
        let button = state.button_pressed(MonotonicMillis::new(3)).unwrap();
        assert_eq!(button.outcome(), DisplayButtonOutcome::WakeAndConsume);
        let restore = start(button.blanking());
        let feedback = state
            .complete(
                restore,
                MonotonicMillis::new(4),
                result(DisplayOperationOutcome::Failed),
            )
            .unwrap();
        assert_eq!(
            feedback.decision(),
            DisplayBlankingDecision::RetryAt(MonotonicMillis::new(9))
        );
        assert_eq!(
            state
                .button_pressed(MonotonicMillis::new(5))
                .unwrap()
                .outcome(),
            DisplayButtonOutcome::WakeAndConsume
        );
    }

    #[test]
    fn in_flight_desire_reversal_runs_the_required_followup_only() {
        let mut state = state();
        let blank = start(
            state
                .request_blanked(DisplayBlankReason::DisplayOnly, MonotonicMillis::new(1))
                .unwrap(),
        );
        assert_eq!(
            state.request_visible(MonotonicMillis::new(2)).unwrap(),
            DisplayBlankingDecision::Settled
        );
        let feedback = state
            .complete(
                blank,
                MonotonicMillis::new(3),
                result(DisplayOperationOutcome::Succeeded),
            )
            .unwrap();
        assert_eq!(
            start(feedback.decision()).command(),
            DisplayBlankingCommand::Restore
        );
    }

    #[test]
    fn explicit_notice_deadline_is_cancelled_by_visible_input() {
        let mut state = state();
        state.schedule_system_sleep(MonotonicMillis::new(10));
        assert_eq!(
            state
                .button_pressed(MonotonicMillis::new(5))
                .unwrap()
                .outcome(),
            DisplayButtonOutcome::ForwardToUi
        );
        assert_eq!(
            state.tick(MonotonicMillis::new(10)).unwrap(),
            DisplayBlankingDecision::Settled
        );
    }

    #[test]
    fn stale_or_foreign_feedback_does_not_commit_visibility() {
        let mut first = state();
        let mut second = state();
        let attempt = start(
            first
                .request_blanked(DisplayBlankReason::DisplayOnly, MonotonicMillis::new(1))
                .unwrap(),
        );
        assert_eq!(
            second.complete(
                attempt,
                MonotonicMillis::new(1),
                result(DisplayOperationOutcome::Succeeded),
            ),
            Err(DisplayBlankingError::ForeignAttempt)
        );
        assert_eq!(second.visibility(), DisplayVisibility::Visible);
    }

    #[test]
    fn toggling_auto_off_does_not_replace_an_explicit_reason() {
        let mut state = state();
        state.schedule_system_sleep(MonotonicMillis::new(10));
        assert_eq!(
            state.toggle_auto_off(MonotonicMillis::new(5)).unwrap(),
            DisplayAutoOff::Disabled
        );
        let attempt = start(state.tick(MonotonicMillis::new(10)).unwrap());
        state
            .complete(
                attempt,
                MonotonicMillis::new(11),
                result(DisplayOperationOutcome::Succeeded),
            )
            .unwrap();
        assert_eq!(state.blank_reason(), Some(DisplayBlankReason::SystemSleep));
        assert_eq!(state.auto_off(), DisplayAutoOff::Disabled);
    }
}
