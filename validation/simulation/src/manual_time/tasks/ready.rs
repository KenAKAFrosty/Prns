use std::collections::BTreeSet;
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::Wake;

use super::ManualTaskId;

pub(super) struct ReadyTasks {
    live: BTreeSet<ManualTaskId>,
    ready: BTreeSet<ManualTaskId>,
    last_polled: Option<ManualTaskId>,
}

impl ReadyTasks {
    pub(super) fn new() -> Self {
        Self {
            live: BTreeSet::new(),
            ready: BTreeSet::new(),
            last_polled: None,
        }
    }

    pub(super) fn lock(shared: &Mutex<Self>) -> MutexGuard<'_, Self> {
        shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn insert(&mut self, id: ManualTaskId) {
        self.live.insert(id);
        self.ready.insert(id);
    }

    pub(super) fn retire(&mut self, id: ManualTaskId) {
        self.live.remove(&id);
        self.ready.remove(&id);
    }

    pub(super) fn ready_count(&self) -> usize {
        self.ready.len()
    }

    pub(super) fn pop_next(&mut self) -> Option<ManualTaskId> {
        let next = self
            .last_polled
            .and_then(|last| self.ready.range((Excluded(last), Unbounded)).next())
            .or_else(|| self.ready.first())
            .copied()?;
        self.ready.remove(&next);
        self.last_polled = Some(next);
        Some(next)
    }

    fn wake(&mut self, id: ManualTaskId) {
        if self.live.contains(&id) {
            self.ready.insert(id);
        }
    }

    #[cfg(test)]
    pub(super) fn counts(&self) -> (usize, usize) {
        (self.live.len(), self.ready.len())
    }
}

pub(super) struct TaskWake {
    id: ManualTaskId,
    ready: Weak<Mutex<ReadyTasks>>,
}

impl TaskWake {
    pub(super) fn new(id: ManualTaskId, ready: Weak<Mutex<ReadyTasks>>) -> Self {
        Self { id, ready }
    }
}

impl Wake for TaskWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(ready) = self.ready.upgrade() {
            ReadyTasks::lock(&ready).wake(self.id);
        }
    }
}
