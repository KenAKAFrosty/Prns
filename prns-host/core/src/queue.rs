use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::lifecycle::transition_allowed;
use crate::{
    ApplicationEvent, DiagnosticBatch, DiagnosticEvent, HostFailure, LifecycleSnapshot,
    LifecycleState, LifecycleTransitionError, PrnsLimits,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Busy,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsumerLane {
    ApplicationEvents,
    Diagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsumerUnavailable {
    pub lane: ConsumerLane,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationEventPushError {
    pub event: Box<ApplicationEvent>,
    pub failure: HostFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticPushOutcome {
    Queued,
    DroppedNewest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueDepths {
    pub pending_commands: usize,
    pub application_events: usize,
    pub retained_event_bytes: usize,
    pub diagnostics: usize,
    pub dropped_diagnostics: u128,
}

pub struct BoundedHostQueue<C> {
    limits: PrnsLimits,
    commands: VecDeque<C>,
    application_events: VecDeque<ApplicationEvent>,
    retained_event_bytes: usize,
    diagnostics: VecDeque<DiagnosticEvent>,
    dropped_diagnostics: u128,
    application_consumer_claimed: bool,
    diagnostics_consumer_claimed: bool,
    lifecycle: LifecycleSnapshot,
}

impl<C> BoundedHostQueue<C> {
    #[must_use]
    pub fn new(limits: PrnsLimits) -> Self {
        Self {
            limits,
            commands: VecDeque::with_capacity(limits.pending_commands()),
            application_events: VecDeque::with_capacity(limits.application_events()),
            retained_event_bytes: 0,
            diagnostics: VecDeque::with_capacity(limits.diagnostics()),
            dropped_diagnostics: 0,
            application_consumer_claimed: false,
            diagnostics_consumer_claimed: false,
            lifecycle: LifecycleSnapshot {
                revision: 0,
                state: LifecycleState::Starting,
            },
        }
    }

    #[must_use]
    pub const fn limits(&self) -> PrnsLimits {
        self.limits
    }

    #[must_use]
    pub fn lifecycle(&self) -> LifecycleSnapshot {
        self.lifecycle.clone()
    }

    pub fn transition(
        &mut self,
        state: LifecycleState,
    ) -> Result<LifecycleSnapshot, LifecycleTransitionError> {
        let from = self.lifecycle.state.phase();
        let to = state.phase();
        if !transition_allowed(from, to) {
            return Err(LifecycleTransitionError { from, to });
        }
        self.lifecycle.revision = self.lifecycle.revision.saturating_add(1);
        self.lifecycle.state = state;
        Ok(self.lifecycle.clone())
    }

    pub fn submit(&mut self, command: C) -> Result<(), SubmitError> {
        if self.lifecycle.state.is_terminal()
            || matches!(self.lifecycle.state, LifecycleState::Stopping)
        {
            return Err(SubmitError::Stopped);
        }
        if self.commands.len() == self.limits.pending_commands() {
            return Err(SubmitError::Busy);
        }
        self.commands.push_back(command);
        Ok(())
    }

    pub fn pop_command(&mut self) -> Option<C> {
        self.commands.pop_front()
    }

    pub fn push_application_event(
        &mut self,
        event: ApplicationEvent,
    ) -> Result<(), ApplicationEventPushError> {
        let event_bytes = event.retained_bytes();
        let count_exceeded = self.application_events.len() == self.limits.application_events();
        let byte_exceeded = self
            .retained_event_bytes
            .checked_add(event_bytes)
            .is_none_or(|total| total > self.limits.retained_event_bytes());
        if count_exceeded || byte_exceeded {
            let failure = HostFailure::EventBackpressureExceeded {
                limits: self.limits,
                rejected_event_bytes: event_bytes,
            };
            if !self.lifecycle.state.is_terminal() {
                self.lifecycle.revision = self.lifecycle.revision.saturating_add(1);
                self.lifecycle.state = LifecycleState::Failed(failure.clone());
            }
            return Err(ApplicationEventPushError {
                event: Box::new(event),
                failure,
            });
        }
        self.retained_event_bytes += event_bytes;
        self.application_events.push_back(event);
        Ok(())
    }

    pub fn pop_application_event(&mut self) -> Option<ApplicationEvent> {
        let event = self.application_events.pop_front()?;
        self.retained_event_bytes = self
            .retained_event_bytes
            .saturating_sub(event.retained_bytes());
        Some(event)
    }

    pub fn push_diagnostic(&mut self, event: DiagnosticEvent) -> DiagnosticPushOutcome {
        if self.diagnostics.len() == self.limits.diagnostics() {
            self.dropped_diagnostics = self.dropped_diagnostics.saturating_add(1);
            return DiagnosticPushOutcome::DroppedNewest;
        }
        self.diagnostics.push_back(event);
        DiagnosticPushOutcome::Queued
    }

    #[must_use]
    pub fn drain_diagnostics(&mut self, maximum_events: usize) -> DiagnosticBatch {
        let take = maximum_events.min(self.diagnostics.len());
        let events: Vec<DiagnosticEvent> = self.diagnostics.drain(..take).collect();
        let dropped_newest = core::mem::take(&mut self.dropped_diagnostics);
        DiagnosticBatch {
            events,
            dropped_newest,
        }
    }

    pub fn claim_consumer(&mut self, lane: ConsumerLane) -> Result<(), ConsumerUnavailable> {
        let claimed = match lane {
            ConsumerLane::ApplicationEvents => &mut self.application_consumer_claimed,
            ConsumerLane::Diagnostics => &mut self.diagnostics_consumer_claimed,
        };
        if *claimed {
            return Err(ConsumerUnavailable { lane });
        }
        *claimed = true;
        Ok(())
    }

    pub fn release_consumer(&mut self, lane: ConsumerLane) {
        match lane {
            ConsumerLane::ApplicationEvents => self.application_consumer_claimed = false,
            ConsumerLane::Diagnostics => self.diagnostics_consumer_claimed = false,
        }
    }

    #[must_use]
    pub fn depths(&self) -> QueueDepths {
        QueueDepths {
            pending_commands: self.commands.len(),
            application_events: self.application_events.len(),
            retained_event_bytes: self.retained_event_bytes,
            diagnostics: self.diagnostics.len(),
            dropped_diagnostics: self.dropped_diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::{DestinationHash, InterfaceId, PrnsLimitsError, SingleDelivery};

    fn event(bytes: usize) -> ApplicationEvent {
        ApplicationEvent::SingleDelivery(SingleDelivery {
            destination: DestinationHash::new([0; 16]),
            source_interface: InterfaceId::new([0; 8]),
            plaintext: vec![0; bytes],
        })
    }

    #[test]
    fn balanced_limits_are_release_contract() {
        let limits = PrnsLimits::balanced();
        assert_eq!(limits.pending_commands(), 256);
        assert_eq!(limits.application_events(), 1_024);
        assert_eq!(limits.retained_event_bytes(), 8 * 1_024 * 1_024);
        assert_eq!(limits.diagnostics(), 1_024);
    }

    #[test]
    fn application_pressure_fails_the_host_without_dropping_the_rejected_event() {
        let limits = match PrnsLimits::try_new(1, 2, 3, 1) {
            Ok(limits) => limits,
            Err(error) => {
                assert_eq!(error, PrnsLimitsError::PendingCommandsZero);
                return;
            }
        };
        let mut queue = BoundedHostQueue::<()>::new(limits);
        assert!(queue.transition(LifecycleState::Running).is_ok());
        assert!(queue.push_application_event(event(2)).is_ok());
        let rejected = match queue.push_application_event(event(2)) {
            Ok(()) => return,
            Err(rejected) => rejected,
        };
        assert_eq!(*rejected.event, event(2));
        assert!(matches!(
            queue.lifecycle().state,
            LifecycleState::Failed(HostFailure::EventBackpressureExceeded { .. })
        ));
        assert_eq!(queue.pop_application_event(), Some(event(2)));
    }

    #[test]
    fn diagnostics_drop_newest_and_report_the_exact_gap() {
        let limits = match PrnsLimits::try_new(1, 1, 1, 1) {
            Ok(limits) => limits,
            Err(error) => {
                assert_eq!(error, PrnsLimitsError::PendingCommandsZero);
                return;
            }
        };
        let mut queue = BoundedHostQueue::<()>::new(limits);
        let diagnostic = DiagnosticEvent::BackendDiagnostic {
            kind: "test".into(),
            detail: "first".into(),
        };
        assert_eq!(
            queue.push_diagnostic(diagnostic.clone()),
            DiagnosticPushOutcome::Queued
        );
        for _ in 0..3 {
            assert_eq!(
                queue.push_diagnostic(DiagnosticEvent::BackendDiagnostic {
                    kind: "test".into(),
                    detail: "dropped".into(),
                }),
                DiagnosticPushOutcome::DroppedNewest
            );
        }
        let batch = queue.drain_diagnostics(1);
        assert_eq!(batch.events, vec![diagnostic]);
        assert_eq!(batch.dropped_newest, 3);
    }

    #[test]
    fn consumers_are_single_owner_per_lane() {
        let mut queue = BoundedHostQueue::<()>::new(PrnsLimits::balanced());
        assert_eq!(
            queue.claim_consumer(ConsumerLane::ApplicationEvents),
            Ok(())
        );
        assert_eq!(
            queue.claim_consumer(ConsumerLane::ApplicationEvents),
            Err(ConsumerUnavailable {
                lane: ConsumerLane::ApplicationEvents
            })
        );
        queue.release_consumer(ConsumerLane::ApplicationEvents);
        assert_eq!(
            queue.claim_consumer(ConsumerLane::ApplicationEvents),
            Ok(())
        );
    }
}
