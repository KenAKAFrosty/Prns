#![forbid(unsafe_code)]

use prns_host::{
    ApplicationEvent, ApplicationEventPushError, BoundedHostQueue, DiagnosticBatch,
    DiagnosticEvent, DiagnosticPushOutcome, LifecycleSnapshot, LifecycleState,
    LifecycleTransitionError, PrnsLimits, QueueDepths, SubmitError,
};

pub const MINIMUM_ENTROPY_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonotonicMillis(u64);

impl MonotonicMillis {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entropy(Vec<u8>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InsufficientEntropy {
    pub minimum: usize,
    pub actual: usize,
}

impl Entropy {
    pub fn try_new(bytes: Vec<u8>) -> Result<Self, InsufficientEntropy> {
        if bytes.len() < MINIMUM_ENTROPY_BYTES {
            return Err(InsufficientEntropy {
                minimum: MINIMUM_ENTROPY_BYTES,
                actual: bytes.len(),
            });
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CooperativeStep {
    pub now: MonotonicMillis,
    pub entropy: Entropy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeMovedBackwards {
    pub previous: MonotonicMillis,
    pub supplied: MonotonicMillis,
}

pub struct CooperativeHost<C> {
    queue: BoundedHostQueue<C>,
    last_now: Option<MonotonicMillis>,
}

impl<C> CooperativeHost<C> {
    #[must_use]
    pub fn new(limits: PrnsLimits) -> Self {
        Self {
            queue: BoundedHostQueue::new(limits),
            last_now: None,
        }
    }

    pub fn begin_step(
        &mut self,
        now: MonotonicMillis,
        entropy: Entropy,
    ) -> Result<CooperativeStep, TimeMovedBackwards> {
        self.observe_time(now)?;
        Ok(CooperativeStep { now, entropy })
    }

    pub fn observe_time(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<(), TimeMovedBackwards> {
        if let Some(previous) = self.last_now {
            if now < previous {
                return Err(TimeMovedBackwards {
                    previous,
                    supplied: now,
                });
            }
        }
        self.last_now = Some(now);
        Ok(())
    }

    pub fn submit(&mut self, command: C) -> Result<(), SubmitError> {
        self.queue.submit(command)
    }

    pub fn pop_command(&mut self) -> Option<C> {
        self.queue.pop_command()
    }

    pub fn publish_application_event(
        &mut self,
        event: ApplicationEvent,
    ) -> Result<(), ApplicationEventPushError> {
        self.queue.push_application_event(event)
    }

    pub fn pop_application_event(&mut self) -> Option<ApplicationEvent> {
        self.queue.pop_application_event()
    }

    pub fn publish_diagnostic(&mut self, event: DiagnosticEvent) -> DiagnosticPushOutcome {
        self.queue.push_diagnostic(event)
    }

    #[must_use]
    pub fn drain_diagnostics(&mut self, maximum_events: usize) -> DiagnosticBatch {
        self.queue.drain_diagnostics(maximum_events)
    }

    pub fn transition(
        &mut self,
        state: LifecycleState,
    ) -> Result<LifecycleSnapshot, LifecycleTransitionError> {
        self.queue.transition(state)
    }

    #[must_use]
    pub fn lifecycle(&self) -> LifecycleSnapshot {
        self.queue.lifecycle()
    }

    #[must_use]
    pub fn depths(&self) -> QueueDepths {
        self.queue.depths()
    }
}

#[cfg(test)]
mod tests {
    use prns_host::{
        ApplicationEvent, DestinationHash, HostFailure, InterfaceId, LifecycleState, PrnsLimits,
        SingleDelivery,
    };

    use super::*;

    #[test]
    fn time_and_entropy_are_explicit_and_monotonic() {
        let entropy = match Entropy::try_new(vec![1; MINIMUM_ENTROPY_BYTES]) {
            Ok(entropy) => entropy,
            Err(_) => return,
        };
        let mut host = CooperativeHost::<()>::new(PrnsLimits::balanced());
        assert!(host
            .begin_step(MonotonicMillis::new(5), entropy.clone())
            .is_ok());
        assert_eq!(
            host.begin_step(MonotonicMillis::new(4), entropy),
            Err(TimeMovedBackwards {
                previous: MonotonicMillis::new(5),
                supplied: MonotonicMillis::new(4),
            })
        );
    }

    #[test]
    fn cooperative_host_uses_the_shared_application_pressure_contract() {
        let limits = match PrnsLimits::try_new(1, 1, 1, 1) {
            Ok(limits) => limits,
            Err(_) => return,
        };
        let mut host = CooperativeHost::<()>::new(limits);
        assert!(host.transition(LifecycleState::Running).is_ok());
        let event = ApplicationEvent::SingleDelivery(SingleDelivery {
            destination: DestinationHash::new([0; 16]),
            source_interface: InterfaceId::new([0; 8]),
            plaintext: vec![0; 2],
        });
        assert!(host.publish_application_event(event).is_err());
        assert!(matches!(
            host.lifecycle().state,
            LifecycleState::Failed(HostFailure::EventBackpressureExceeded { .. })
        ));
    }
}
