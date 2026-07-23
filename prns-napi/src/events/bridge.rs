use std::sync::Arc;

use napi::bindgen_prelude::Object;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::Status;
use personal_rns::runtime::PrnsEvent;

use super::owned::OwnedEvent;

pub type EventTsfn = ThreadsafeFunction<OwnedEvent, (), Object<'static>, Status, false>;

#[derive(Clone)]
pub struct EventSink {
    tsfn: Arc<EventTsfn>,
}

impl EventSink {
    pub fn new(tsfn: EventTsfn) -> Self {
        Self {
            tsfn: Arc::new(tsfn),
        }
    }

    pub fn dispatch(&self, event: PrnsEvent<'_>) {
        if let Some(owned) = OwnedEvent::capture(event) {
            self.tsfn
                .call(owned, ThreadsafeFunctionCallMode::NonBlocking);
        }
    }

    pub fn emit(&self, event: OwnedEvent) {
        self.tsfn
            .call(event, ThreadsafeFunctionCallMode::NonBlocking);
    }

    pub fn node_stopped(&self, cause: &str) {
        self.tsfn.call(
            OwnedEvent::NodeStopped {
                cause: cause.to_string(),
            },
            ThreadsafeFunctionCallMode::NonBlocking,
        );
    }
}
