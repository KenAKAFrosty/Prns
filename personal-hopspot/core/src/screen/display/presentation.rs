use super::time::{DisplayDuration, MonotonicMillis, PartialRefreshLimit};
use crate::face_64x128::Frame;

#[derive(Debug, Eq, PartialEq)]
pub enum EinkPolicyError {
    TelemetryMinimumExceedsFullMaximumAge,
}

#[derive(Debug, Eq, PartialEq)]
pub struct EinkPolicyConfiguration {
    pub telemetry_minimum: DisplayDuration,
    pub refresh: EinkRefreshPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EinkRefreshPolicy {
    FullOnly,
    Partial {
        maximum_consecutive: PartialRefreshLimit,
        full_maximum_age: DisplayDuration,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EinkPolicy {
    telemetry_minimum: DisplayDuration,
    refresh: EinkRefreshPolicy,
}

impl EinkPolicy {
    pub const fn new(configuration: EinkPolicyConfiguration) -> Result<Self, EinkPolicyError> {
        match configuration.refresh {
            EinkRefreshPolicy::FullOnly => {}
            EinkRefreshPolicy::Partial {
                full_maximum_age, ..
            } => {
                if configuration.telemetry_minimum.as_millis() > full_maximum_age.as_millis() {
                    return Err(EinkPolicyError::TelemetryMinimumExceedsFullMaximumAge);
                }
            }
        }
        Ok(Self {
            telemetry_minimum: configuration.telemetry_minimum,
            refresh: configuration.refresh,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationPolicy {
    Immediate,
    RetainedEink(EinkPolicy),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationUrgency {
    Immediate,
    Telemetry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshKind {
    Immediate,
    Full,
    Partial,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PresentationOutcome {
    Succeeded,
    Failed,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PresentationError {
    DisplayUnavailable,
    DisplayNotVisible,
    AttemptInFlight,
    MissingAttempt,
    StaleAttempt,
    TimeWentBackward,
    AttemptIdentityExhausted,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PresentationDecision {
    Unchanged,
    DeferredUntil(MonotonicMillis),
    Present(PresentationAttempt),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PresentationAttempt {
    id: u64,
    planned_at: MonotonicMillis,
    kind: RefreshKind,
}

impl PresentationAttempt {
    #[must_use]
    pub const fn refresh_kind(&self) -> RefreshKind {
        self.kind
    }
}

struct PresentationFrames {
    candidate: Frame,
    presented: Frame,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PresentationKnowledge {
    Unknown,
    Known,
}

enum PresentationPhase {
    Ready,
    Presenting { attempt_id: u64 },
}

pub(super) struct Presenter {
    policy: PresentationPolicy,
    frames: PresentationFrames,
    phase: PresentationPhase,
    knowledge: PresentationKnowledge,
    next_attempt_id: u64,
    last_completion: Option<MonotonicMillis>,
    last_success: Option<MonotonicMillis>,
    last_full: Option<MonotonicMillis>,
    partials_since_full: u32,
}

impl Presenter {
    pub(super) fn new(policy: PresentationPolicy) -> Self {
        Self {
            policy,
            frames: PresentationFrames {
                candidate: Frame::new(),
                presented: Frame::new(),
            },
            phase: PresentationPhase::Ready,
            knowledge: PresentationKnowledge::Unknown,
            next_attempt_id: 1,
            last_completion: None,
            last_success: None,
            last_full: None,
            partials_since_full: 0,
        }
    }

    pub(super) fn candidate_mut(&mut self) -> Result<&mut Frame, PresentationError> {
        match self.phase {
            PresentationPhase::Ready => Ok(&mut self.frames.candidate),
            PresentationPhase::Presenting { .. } => Err(PresentationError::AttemptInFlight),
        }
    }

    pub(super) const fn has_pending_attempt(&self) -> bool {
        matches!(self.phase, PresentationPhase::Presenting { .. })
    }

    pub(super) fn plan(
        &mut self,
        now: MonotonicMillis,
        urgency: PresentationUrgency,
    ) -> Result<PresentationDecision, PresentationError> {
        if self.has_pending_attempt() {
            return Err(PresentationError::AttemptInFlight);
        }
        if self.last_completion.is_some_and(|history| now < history) {
            return Err(PresentationError::TimeWentBackward);
        }
        if matches!(self.policy, PresentationPolicy::RetainedEink(_))
            && self.knowledge == PresentationKnowledge::Known
            && self.frames.presented == self.frames.candidate
        {
            return Ok(PresentationDecision::Unchanged);
        }
        if let Some(deadline) = self.telemetry_deadline(urgency) {
            if now < deadline {
                return Ok(PresentationDecision::DeferredUntil(deadline));
            }
        }

        let id = self.next_attempt_id;
        self.next_attempt_id = self
            .next_attempt_id
            .checked_add(1)
            .ok_or(PresentationError::AttemptIdentityExhausted)?;
        let kind = self.refresh_kind(now);
        self.phase = PresentationPhase::Presenting { attempt_id: id };
        Ok(PresentationDecision::Present(PresentationAttempt {
            id,
            planned_at: now,
            kind,
        }))
    }

    pub(super) fn pending_frame(
        &self,
        attempt: &PresentationAttempt,
    ) -> Result<&Frame, PresentationError> {
        match self.phase {
            PresentationPhase::Ready => Err(PresentationError::MissingAttempt),
            PresentationPhase::Presenting { attempt_id } if attempt_id != attempt.id => {
                Err(PresentationError::StaleAttempt)
            }
            PresentationPhase::Presenting { .. } => Ok(&self.frames.candidate),
        }
    }

    pub(super) fn complete(
        &mut self,
        attempt: PresentationAttempt,
        completed_at: MonotonicMillis,
        outcome: PresentationOutcome,
    ) -> Result<(), PresentationError> {
        match self.phase {
            PresentationPhase::Ready => return Err(PresentationError::MissingAttempt),
            PresentationPhase::Presenting { attempt_id } if attempt_id != attempt.id => {
                return Err(PresentationError::StaleAttempt);
            }
            PresentationPhase::Presenting { .. } => {}
        }
        if completed_at < attempt.planned_at
            || self
                .last_completion
                .is_some_and(|history| completed_at < history)
        {
            return Err(PresentationError::TimeWentBackward);
        }

        self.phase = PresentationPhase::Ready;
        self.last_completion = Some(completed_at);
        match outcome {
            PresentationOutcome::Succeeded => {
                if matches!(self.policy, PresentationPolicy::RetainedEink(_)) {
                    self.frames.presented.clone_from(&self.frames.candidate);
                }
                self.knowledge = PresentationKnowledge::Known;
                self.last_success = Some(completed_at);
                match attempt.kind {
                    RefreshKind::Immediate => {}
                    RefreshKind::Full => {
                        self.last_full = Some(completed_at);
                        self.partials_since_full = 0;
                    }
                    RefreshKind::Partial => {
                        self.partials_since_full = self.partials_since_full.saturating_add(1);
                    }
                }
            }
            PresentationOutcome::Failed => {
                self.knowledge = PresentationKnowledge::Unknown;
            }
        }
        Ok(())
    }

    pub(super) fn invalidate(&mut self) {
        self.phase = PresentationPhase::Ready;
        self.knowledge = PresentationKnowledge::Unknown;
    }

    fn telemetry_deadline(&self, urgency: PresentationUrgency) -> Option<MonotonicMillis> {
        let PresentationPolicy::RetainedEink(policy) = self.policy else {
            return None;
        };
        if urgency == PresentationUrgency::Immediate
            || self.knowledge == PresentationKnowledge::Unknown
        {
            return None;
        }
        self.last_success
            .map(|last| last.saturating_add(policy.telemetry_minimum))
    }

    fn refresh_kind(&self, now: MonotonicMillis) -> RefreshKind {
        let PresentationPolicy::RetainedEink(policy) = self.policy else {
            return RefreshKind::Immediate;
        };
        let EinkRefreshPolicy::Partial {
            maximum_consecutive,
            full_maximum_age,
        } = policy.refresh
        else {
            return RefreshKind::Full;
        };
        let full_expired = self.last_full.is_some_and(|last| {
            now.as_millis().saturating_sub(last.as_millis()) >= full_maximum_age.as_millis()
        });
        if self.knowledge == PresentationKnowledge::Unknown
            || self.last_full.is_none()
            || self.partials_since_full >= maximum_consecutive.get()
            || full_expired
        {
            RefreshKind::Full
        } else {
            RefreshKind::Partial
        }
    }
}
