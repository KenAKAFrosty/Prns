#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use prns_host::{
    ApplicationEvent, ApplicationEventPushError, BoundedHostQueue, ConsumerLane,
    ConsumerUnavailable, DiagnosticBatch, DiagnosticEvent, DiagnosticPushOutcome,
    LifecycleSnapshot, LifecycleState, LifecycleTransitionError, PrnsLimits, QueueDepths,
    SubmitError,
};
use tokio::sync::{Mutex, Notify};

struct Shared<C> {
    queue: Mutex<BoundedHostQueue<C>>,
    command_ready: Notify,
    application_ready: Notify,
    diagnostics_ready: Notify,
    application_claimed: AtomicBool,
    diagnostics_claimed: AtomicBool,
}

pub struct TokioHostHandle<C> {
    shared: Arc<Shared<C>>,
}

pub struct TokioHostDriver<C> {
    shared: Arc<Shared<C>>,
}

pub struct ApplicationEventStream<C> {
    shared: Arc<Shared<C>>,
}

pub struct DiagnosticStream<C> {
    shared: Arc<Shared<C>>,
}

#[must_use]
pub fn channel<C>(limits: PrnsLimits) -> (TokioHostHandle<C>, TokioHostDriver<C>) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(BoundedHostQueue::new(limits)),
        command_ready: Notify::new(),
        application_ready: Notify::new(),
        diagnostics_ready: Notify::new(),
        application_claimed: AtomicBool::new(false),
        diagnostics_claimed: AtomicBool::new(false),
    });
    (
        TokioHostHandle {
            shared: Arc::clone(&shared),
        },
        TokioHostDriver { shared },
    )
}

impl<C> Clone for TokioHostHandle<C> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<C> TokioHostHandle<C> {
    pub async fn submit(&self, command: C) -> Result<(), SubmitError> {
        let result = self.shared.queue.lock().await.submit(command);
        if result.is_ok() {
            self.shared.command_ready.notify_one();
        }
        result
    }

    #[must_use]
    pub async fn lifecycle(&self) -> LifecycleSnapshot {
        self.shared.queue.lock().await.lifecycle()
    }

    #[must_use]
    pub async fn depths(&self) -> QueueDepths {
        self.shared.queue.lock().await.depths()
    }

    pub fn application_events(&self) -> Result<ApplicationEventStream<C>, ConsumerUnavailable> {
        claim(
            &self.shared.application_claimed,
            ConsumerLane::ApplicationEvents,
        )?;
        Ok(ApplicationEventStream {
            shared: Arc::clone(&self.shared),
        })
    }

    pub fn diagnostics(&self) -> Result<DiagnosticStream<C>, ConsumerUnavailable> {
        claim(&self.shared.diagnostics_claimed, ConsumerLane::Diagnostics)?;
        Ok(DiagnosticStream {
            shared: Arc::clone(&self.shared),
        })
    }
}

impl<C> TokioHostDriver<C> {
    pub async fn next_command(&self) -> Option<C> {
        loop {
            let ready = self.shared.command_ready.notified();
            {
                let mut queue = self.shared.queue.lock().await;
                if let Some(command) = queue.pop_command() {
                    return Some(command);
                }
                if queue.lifecycle().state.is_terminal() {
                    return None;
                }
            }
            ready.await;
        }
    }

    pub async fn publish_application_event(
        &self,
        event: ApplicationEvent,
    ) -> Result<(), ApplicationEventPushError> {
        let result = self.shared.queue.lock().await.push_application_event(event);
        self.shared.application_ready.notify_one();
        if result.is_err() {
            self.notify_all();
        }
        result
    }

    pub async fn publish_diagnostic(&self, event: DiagnosticEvent) -> DiagnosticPushOutcome {
        let outcome = self.shared.queue.lock().await.push_diagnostic(event);
        self.shared.diagnostics_ready.notify_one();
        outcome
    }

    pub async fn transition(
        &self,
        state: LifecycleState,
    ) -> Result<LifecycleSnapshot, LifecycleTransitionError> {
        let result = self.shared.queue.lock().await.transition(state);
        if result.is_ok() {
            self.notify_all();
        }
        result
    }

    fn notify_all(&self) {
        self.shared.command_ready.notify_waiters();
        self.shared.application_ready.notify_waiters();
        self.shared.diagnostics_ready.notify_waiters();
    }
}

impl<C> ApplicationEventStream<C> {
    pub async fn next(&mut self) -> Option<ApplicationEvent> {
        loop {
            let ready = self.shared.application_ready.notified();
            {
                let mut queue = self.shared.queue.lock().await;
                if let Some(event) = queue.pop_application_event() {
                    return Some(event);
                }
                if queue.lifecycle().state.is_terminal() {
                    return None;
                }
            }
            ready.await;
        }
    }
}

impl<C> Drop for ApplicationEventStream<C> {
    fn drop(&mut self) {
        self.shared
            .application_claimed
            .store(false, Ordering::Release);
    }
}

impl<C> DiagnosticStream<C> {
    pub async fn next_batch(&mut self, maximum_events: usize) -> Option<DiagnosticBatch> {
        loop {
            let ready = self.shared.diagnostics_ready.notified();
            {
                let mut queue = self.shared.queue.lock().await;
                let depths = queue.depths();
                if depths.diagnostics > 0 || depths.dropped_diagnostics > 0 {
                    return Some(queue.drain_diagnostics(maximum_events));
                }
                if queue.lifecycle().state.is_terminal() {
                    return None;
                }
            }
            ready.await;
        }
    }
}

impl<C> Drop for DiagnosticStream<C> {
    fn drop(&mut self) {
        self.shared
            .diagnostics_claimed
            .store(false, Ordering::Release);
    }
}

fn claim(flag: &AtomicBool, lane: ConsumerLane) -> Result<(), ConsumerUnavailable> {
    flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| ConsumerUnavailable { lane })
}

#[cfg(test)]
mod tests {
    use prns_host::{DestinationHash, InterfaceId, SingleDelivery};

    use super::*;

    #[tokio::test]
    async fn command_backpressure_and_single_consumers_are_typed() {
        let limits = match PrnsLimits::try_new(1, 2, 16, 2) {
            Ok(limits) => limits,
            Err(_) => return,
        };
        let (handle, driver) = channel(limits);
        assert!(driver.transition(LifecycleState::Running).await.is_ok());
        assert_eq!(handle.submit(1u8).await, Ok(()));
        assert_eq!(handle.submit(2u8).await, Err(SubmitError::Busy));
        assert_eq!(driver.next_command().await, Some(1));
        let first = handle.application_events();
        assert!(first.is_ok());
        assert!(matches!(
            handle.application_events(),
            Err(ConsumerUnavailable {
                lane: ConsumerLane::ApplicationEvents
            })
        ));
        drop(first);
        assert!(handle.application_events().is_ok());
    }

    #[tokio::test]
    async fn application_event_delivery_is_lossless_before_terminal_state() {
        let (handle, driver) = channel::<()>(PrnsLimits::balanced());
        assert!(driver.transition(LifecycleState::Running).await.is_ok());
        let mut stream = match handle.application_events() {
            Ok(stream) => stream,
            Err(_) => return,
        };
        let event = ApplicationEvent::SingleDelivery(SingleDelivery {
            destination: DestinationHash::new([1; 16]),
            source_interface: InterfaceId::new([2; 8]),
            plaintext: vec![3],
        });
        assert!(driver
            .publish_application_event(event.clone())
            .await
            .is_ok());
        assert_eq!(stream.next().await, Some(event));
    }
}
