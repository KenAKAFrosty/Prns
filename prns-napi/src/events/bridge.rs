use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use napi::bindgen_prelude::Object;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::Status;
use personal_rns::runtime::PrnsEvent;

use super::owned::OwnedEvent;

pub type EventTsfn = ThreadsafeFunction<OwnedEvent, (), Object<'static>, Status, false>;

pub const DEFAULT_EVENT_QUEUE_LIMIT: usize = 16384;

#[derive(Clone)]
pub struct EventSink {
    tsfn: Arc<EventTsfn>,
    queued: Arc<AtomicUsize>,
    dropped_diagnostics: Arc<AtomicUsize>,
    limit: usize,
}

impl EventSink {
    pub fn new(tsfn: EventTsfn, queued: Arc<AtomicUsize>, limit: usize) -> Self {
        Self {
            tsfn: Arc::new(tsfn),
            queued,
            dropped_diagnostics: Arc::new(AtomicUsize::new(0)),
            limit,
        }
    }

    fn call(&self, event: OwnedEvent) {
        self.queued.fetch_add(1, Ordering::Relaxed);
        if self
            .tsfn
            .call(event, ThreadsafeFunctionCallMode::NonBlocking)
            != Status::Ok
        {
            self.queued.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub fn dispatch(&self, event: PrnsEvent<'_>) {
        if let Some(owned) = OwnedEvent::capture(event) {
            self.emit(owned);
        }
    }

    pub fn emit(&self, event: OwnedEvent) {
        if event.droppable() && self.queued.load(Ordering::Relaxed) >= self.limit {
            self.dropped_diagnostics.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let dropped = self.dropped_diagnostics.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            self.call(OwnedEvent::EventOverflow {
                dropped_diagnostics: dropped as u64,
            });
        }
        self.call(event);
    }

    pub fn node_stopped(&self, cause: &str) {
        self.emit(OwnedEvent::NodeStopped {
            cause: cause.to_string(),
        });
    }
}
