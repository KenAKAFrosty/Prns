use super::time::{DisplayDuration, MonotonicMillis};
use crate::UserBlanking;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayAutoOff {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayButtonOutcome {
    ForwardToUi,
    WakeAndConsume,
}

#[derive(Debug, Eq, PartialEq)]
pub enum UserBlankingPolicy {
    Unavailable,
    Available {
        initially_visible_at: MonotonicMillis,
        auto_off_after: DisplayDuration,
        retry_backoff: DisplayDuration,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayVisibility {
    Unavailable,
    Visible,
    Blanked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayBlankReason {
    DisplayOnly,
    SystemSleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlankingCommand {
    Blank,
    Restore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlankingDecision {
    Settled,
    Apply(BlankingCommand),
    RetryAt(MonotonicMillis),
}

#[derive(Debug, Eq, PartialEq)]
pub enum BlankingResult {
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferRetention {
    Preserved,
    Lost,
}

#[derive(Debug, Eq, PartialEq)]
pub struct BlankingOutcome {
    pub result: BlankingResult,
    pub buffer_retention: BufferRetention,
}

#[derive(Debug, Eq, PartialEq)]
pub enum BlankingError {
    DisplayUnavailable,
    UserBlankingUnavailable,
    OperationInFlight,
    PresentationInFlight,
    MissingOperation,
    TimeWentBackward,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ButtonDecision {
    outcome: DisplayButtonOutcome,
    blanking: BlankingDecision,
}

impl ButtonDecision {
    pub(super) const fn forwarded() -> Self {
        Self {
            outcome: DisplayButtonOutcome::ForwardToUi,
            blanking: BlankingDecision::Settled,
        }
    }

    #[must_use]
    pub const fn outcome(&self) -> DisplayButtonOutcome {
        self.outcome
    }

    #[must_use]
    pub const fn blanking(&self) -> BlankingDecision {
        self.blanking
    }
}

pub(super) enum BlankingCapability {
    Unavailable,
    Available(BlankingState),
}

impl BlankingCapability {
    pub(super) fn from_policy(policy: UserBlankingPolicy) -> Self {
        match policy {
            UserBlankingPolicy::Unavailable => Self::Unavailable,
            UserBlankingPolicy::Available {
                initially_visible_at,
                auto_off_after,
                retry_backoff,
            } => Self::Available(BlankingState::new(
                initially_visible_at,
                auto_off_after,
                retry_backoff,
            )),
        }
    }

    pub(super) const fn user_blanking(&self) -> UserBlanking {
        match self {
            Self::Unavailable => UserBlanking::unavailable(),
            Self::Available(_) => UserBlanking::available(),
        }
    }

    pub(super) const fn visibility(&self) -> DisplayVisibility {
        match self {
            Self::Unavailable => DisplayVisibility::Visible,
            Self::Available(blanking) => blanking.visibility(),
        }
    }

    pub(super) const fn presentation_is_allowed(&self) -> bool {
        match self {
            Self::Unavailable => true,
            Self::Available(blanking) => blanking.is_settled_visible(),
        }
    }

    pub(super) fn state_mut(&mut self) -> Result<&mut BlankingState, BlankingError> {
        match self {
            Self::Unavailable => Err(BlankingError::UserBlankingUnavailable),
            Self::Available(blanking) => Ok(blanking),
        }
    }

    pub(super) fn button_pressed(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<ButtonDecision, BlankingError> {
        match self {
            Self::Unavailable => Ok(ButtonDecision {
                outcome: DisplayButtonOutcome::ForwardToUi,
                blanking: BlankingDecision::Settled,
            }),
            Self::Available(blanking) => blanking.button_pressed(now),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlankingTarget {
    Visible,
    Blanked(DisplayBlankReason),
}

impl BlankingTarget {
    const fn visibility(self) -> DisplayVisibility {
        match self {
            Self::Visible => DisplayVisibility::Visible,
            Self::Blanked(_) => DisplayVisibility::Blanked,
        }
    }

    const fn command(self) -> BlankingCommand {
        match self {
            Self::Visible => BlankingCommand::Restore,
            Self::Blanked(_) => BlankingCommand::Blank,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlankingPhase {
    Settled(BlankingTarget),
    InFlight {
        confirmed: BlankingTarget,
        target: BlankingTarget,
    },
    RetryPending {
        confirmed: BlankingTarget,
        target: BlankingTarget,
        not_before: MonotonicMillis,
    },
}

#[derive(Clone, Copy)]
struct ScheduledBlanking {
    at: MonotonicMillis,
    reason: DisplayBlankReason,
}

#[derive(Clone, Copy)]
enum AutoOffState {
    Disabled,
    Disarmed,
    Armed(MonotonicMillis),
}

impl AutoOffState {
    const fn setting(self) -> DisplayAutoOff {
        match self {
            Self::Disabled => DisplayAutoOff::Disabled,
            Self::Disarmed | Self::Armed(_) => DisplayAutoOff::Enabled,
        }
    }
}

pub(super) struct BlankingState {
    phase: BlankingPhase,
    auto_off: AutoOffState,
    auto_off_after: DisplayDuration,
    scheduled: Option<ScheduledBlanking>,
    retry_backoff: DisplayDuration,
    last_observed: MonotonicMillis,
}

impl BlankingState {
    fn new(
        now: MonotonicMillis,
        auto_off_after: DisplayDuration,
        retry_backoff: DisplayDuration,
    ) -> Self {
        Self {
            phase: BlankingPhase::Settled(BlankingTarget::Visible),
            auto_off: AutoOffState::Armed(now.saturating_add(auto_off_after)),
            auto_off_after,
            scheduled: None,
            retry_backoff,
            last_observed: now,
        }
    }

    const fn visibility(&self) -> DisplayVisibility {
        match self.phase {
            BlankingPhase::Settled(confirmed)
            | BlankingPhase::InFlight { confirmed, .. }
            | BlankingPhase::RetryPending { confirmed, .. } => confirmed.visibility(),
        }
    }

    const fn is_settled_visible(&self) -> bool {
        matches!(self.phase, BlankingPhase::Settled(BlankingTarget::Visible))
    }

    fn observe(&mut self, now: MonotonicMillis) -> Result<(), BlankingError> {
        if now < self.last_observed {
            return Err(BlankingError::TimeWentBackward);
        }
        self.last_observed = now;
        Ok(())
    }

    pub(super) fn schedule(
        &mut self,
        at: MonotonicMillis,
        reason: DisplayBlankReason,
    ) -> Result<(), BlankingError> {
        let BlankingPhase::Settled(confirmed) = self.phase else {
            return Err(BlankingError::OperationInFlight);
        };
        if confirmed == BlankingTarget::Visible {
            self.scheduled = Some(ScheduledBlanking { at, reason });
            self.disarm_auto_off();
        }
        Ok(())
    }

    pub(super) fn request_visible(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        self.observe(now)?;
        self.scheduled = None;
        self.request(BlankingTarget::Visible)
    }

    pub(super) fn request_blanked(
        &mut self,
        reason: DisplayBlankReason,
        now: MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        self.observe(now)?;
        self.disarm_auto_off();
        self.scheduled = None;
        self.request(BlankingTarget::Blanked(reason))
    }

    fn request(&mut self, target: BlankingTarget) -> Result<BlankingDecision, BlankingError> {
        let BlankingPhase::Settled(confirmed) = self.phase else {
            return Err(BlankingError::OperationInFlight);
        };
        if confirmed.visibility() == target.visibility() {
            self.phase = BlankingPhase::Settled(target);
            return Ok(BlankingDecision::Settled);
        }
        self.phase = BlankingPhase::InFlight { confirmed, target };
        Ok(BlankingDecision::Apply(target.command()))
    }

    pub(super) fn tick(&mut self, now: MonotonicMillis) -> Result<BlankingDecision, BlankingError> {
        self.observe(now)?;
        match self.phase {
            BlankingPhase::InFlight { .. } => return Err(BlankingError::OperationInFlight),
            BlankingPhase::RetryPending {
                confirmed,
                target,
                not_before,
            } => {
                if now < not_before {
                    return Ok(BlankingDecision::RetryAt(not_before));
                }
                self.phase = BlankingPhase::InFlight { confirmed, target };
                return Ok(BlankingDecision::Apply(target.command()));
            }
            BlankingPhase::Settled(_) => {}
        }
        if let Some(scheduled) = self.scheduled {
            if now >= scheduled.at {
                self.scheduled = None;
                return self.request(BlankingTarget::Blanked(scheduled.reason));
            }
        }
        if let AutoOffState::Armed(deadline) = self.auto_off {
            if now >= deadline {
                self.auto_off = AutoOffState::Disarmed;
                return self.request(BlankingTarget::Blanked(DisplayBlankReason::DisplayOnly));
            }
        }
        Ok(BlankingDecision::Settled)
    }

    pub(super) fn complete(
        &mut self,
        completed_at: MonotonicMillis,
        result: BlankingResult,
    ) -> Result<BlankingDecision, BlankingError> {
        self.observe(completed_at)?;
        let BlankingPhase::InFlight { confirmed, target } = self.phase else {
            return Err(BlankingError::MissingOperation);
        };
        match result {
            BlankingResult::Succeeded => {
                self.phase = BlankingPhase::Settled(target);
                if target == BlankingTarget::Visible {
                    self.rearm_auto_off(completed_at);
                }
                Ok(BlankingDecision::Settled)
            }
            BlankingResult::Failed => {
                let not_before = completed_at.saturating_add(self.retry_backoff);
                self.phase = BlankingPhase::RetryPending {
                    confirmed,
                    target,
                    not_before,
                };
                Ok(BlankingDecision::RetryAt(not_before))
            }
        }
    }

    pub(super) fn button_pressed(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<ButtonDecision, BlankingError> {
        self.observe(now)?;
        match self.phase {
            BlankingPhase::Settled(BlankingTarget::Visible) => {
                self.scheduled = None;
                self.rearm_auto_off(now);
                Ok(ButtonDecision {
                    outcome: DisplayButtonOutcome::ForwardToUi,
                    blanking: BlankingDecision::Settled,
                })
            }
            BlankingPhase::Settled(BlankingTarget::Blanked(DisplayBlankReason::DisplayOnly)) => {
                Ok(ButtonDecision {
                    outcome: DisplayButtonOutcome::WakeAndConsume,
                    blanking: self.request(BlankingTarget::Visible)?,
                })
            }
            BlankingPhase::Settled(BlankingTarget::Blanked(DisplayBlankReason::SystemSleep)) => {
                Ok(ButtonDecision {
                    outcome: DisplayButtonOutcome::ForwardToUi,
                    blanking: BlankingDecision::Settled,
                })
            }
            BlankingPhase::InFlight { .. } | BlankingPhase::RetryPending { .. } => {
                Err(BlankingError::OperationInFlight)
            }
        }
    }

    pub(super) fn toggle_auto_off(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<DisplayAutoOff, BlankingError> {
        self.observe(now)?;
        if !matches!(self.phase, BlankingPhase::Settled(_)) {
            return Err(BlankingError::OperationInFlight);
        }
        match self.auto_off.setting() {
            DisplayAutoOff::Enabled => self.auto_off = AutoOffState::Disabled,
            DisplayAutoOff::Disabled => {
                self.auto_off = AutoOffState::Disarmed;
                if self.visibility() == DisplayVisibility::Visible && self.scheduled.is_none() {
                    self.rearm_auto_off(now);
                }
            }
        }
        Ok(self.auto_off.setting())
    }

    fn disarm_auto_off(&mut self) {
        if self.auto_off.setting() == DisplayAutoOff::Enabled {
            self.auto_off = AutoOffState::Disarmed;
        }
    }

    fn rearm_auto_off(&mut self, now: MonotonicMillis) {
        if self.auto_off.setting() == DisplayAutoOff::Enabled {
            self.auto_off = AutoOffState::Armed(now.saturating_add(self.auto_off_after));
        }
    }
}
