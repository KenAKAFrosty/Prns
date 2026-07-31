use std::sync::{Arc, Mutex, PoisonError};

use napi::bindgen_prelude::Object;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::Status;
use personal_rns::runtime::PrnsEvent;
use prns_host::{
    EventDeliveryAdmission as Admission, EventDeliveryQueue as QueueState, PrnsLimits,
};
use tokio::sync::Notify;

use super::owned::OwnedEvent;

pub type EventTsfn = ThreadsafeFunction<OwnedEvent, (), Object<'static>, Status, false>;

#[derive(Clone)]
pub struct EventQueue {
    state: Arc<Mutex<QueueState<OwnedEvent>>>,
}

impl EventQueue {
    #[must_use]
    pub fn new(limits: PrnsLimits) -> Self {
        Self {
            state: Arc::new(Mutex::new(QueueState::new(limits))),
        }
    }

    fn admit(&self, event: OwnedEvent) -> Admission<OwnedEvent> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .admit(event)
    }

    pub fn complete(&self, event: &OwnedEvent) {
        self.complete_parts(event.application_bytes(), event.terminal());
    }

    fn complete_parts(&self, application_bytes: Option<usize>, terminal: bool) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .complete_parts(application_bytes, terminal);
    }

    #[must_use]
    pub fn failed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .failed()
    }
}

#[derive(Clone)]
pub struct EventSink {
    tsfn: Arc<EventTsfn>,
    queue: EventQueue,
    failed: Arc<Notify>,
}

impl EventSink {
    pub fn new(tsfn: EventTsfn, queue: EventQueue) -> Self {
        Self {
            tsfn: Arc::new(tsfn),
            queue,
            failed: Arc::new(Notify::new()),
        }
    }

    fn call(&self, event: OwnedEvent) {
        let application_bytes = event.application_bytes();
        let terminal = event.terminal();
        if self
            .tsfn
            .call(event, ThreadsafeFunctionCallMode::NonBlocking)
            != Status::Ok
        {
            self.queue.complete_parts(application_bytes, terminal);
        }
    }

    pub fn dispatch(&self, event: PrnsEvent<'_>) {
        if let Some(owned) = OwnedEvent::capture(event) {
            self.emit(owned);
        }
    }

    pub fn emit(&self, event: OwnedEvent) {
        match self.queue.admit(event) {
            Admission::Accepted(events) => {
                for event in events {
                    self.call(event);
                }
            }
            Admission::ApplicationRejected(event) => {
                let rejected_event_bytes = event
                    .application_bytes()
                    .and_then(|bytes| u64::try_from(bytes).ok())
                    .unwrap_or(u64::MAX);
                let terminal = OwnedEvent::EventBackpressureExceeded {
                    rejected_event_bytes,
                };
                if let Admission::Accepted(events) = self.queue.admit(terminal) {
                    for event in events {
                        self.call(event);
                    }
                }
                self.failed.notify_waiters();
            }
            Admission::DroppedDiagnostic | Admission::Ignored => {}
        }
    }

    pub fn node_stopped(&self, cause: &str) {
        self.emit(OwnedEvent::NodeStopped {
            cause: cause.to_string(),
        });
    }

    pub async fn wait_failed(&self) {
        loop {
            let failed = self.failed.notified();
            if self.queue.failed() {
                return;
            }
            failed.await;
        }
    }

    pub fn failed(&self) -> bool {
        self.queue.failed()
    }
}
