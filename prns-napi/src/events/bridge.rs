use std::sync::{Arc, Mutex, PoisonError};

use napi::bindgen_prelude::Object;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::Status;
use prns_host::{
    EventDeliveryAdmission as Admission, EventDeliveryQueue as QueueState, PrnsLimits,
};

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
}

impl EventSink {
    pub fn new(tsfn: EventTsfn, queue: EventQueue) -> Self {
        Self {
            tsfn: Arc::new(tsfn),
            queue,
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
            }
            Admission::DroppedDiagnostic | Admission::Ignored => {}
        }
    }

    pub fn node_stopped(&self, cause: &str) {
        self.emit(OwnedEvent::NodeStopped {
            cause: cause.to_string(),
        });
    }

    pub fn failed(&self) -> bool {
        self.queue.failed()
    }
}

impl prns_host_native::NativeEventSink for EventSink {
    fn running(&self) {}

    fn publish_application(&self, event: prns_host::ApplicationEvent) -> bool {
        if let Some(event) = OwnedEvent::capture_host_application(event) {
            self.emit(event);
        }
        !self.failed()
    }

    fn publish_resource(&self, event: prns_host::ResourceAvailable, body: Vec<u8>) -> bool {
        self.emit(OwnedEvent::capture_host_resource(event, body));
        !self.failed()
    }

    fn publish_diagnostic(&self, event: prns_host::DiagnosticEvent) {
        self.emit(OwnedEvent::capture_host_diagnostic(event));
    }

    fn stopped(&self) {
        self.node_stopped("stopped");
    }

    fn failed(&self, detail: String) {
        self.node_stopped(&detail);
    }
}
