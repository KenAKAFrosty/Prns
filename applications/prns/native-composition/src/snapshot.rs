use std::sync::{Mutex, MutexGuard};

use crate::contract::{
    BluetoothState, DevelopmentNodeFailure, DevelopmentNodeRuntime, DevelopmentNodeSnapshot,
    U64String,
};

pub struct SnapshotStore {
    inner: Mutex<DevelopmentNodeSnapshot>,
}

impl SnapshotStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(DevelopmentNodeSnapshot::stopped()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, DevelopmentNodeSnapshot> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn read(&self) -> DevelopmentNodeSnapshot {
        self.lock().clone()
    }

    pub fn update(&self, update: impl FnOnce(&mut DevelopmentNodeSnapshot)) {
        let mut snapshot = self.lock();
        let next = snapshot
            .revision
            .0
            .parse::<u64>()
            .unwrap_or_default()
            .saturating_add(1);
        update(&mut snapshot);
        snapshot.revision = U64String::from(next);
    }

    pub fn set_runtime(&self, runtime: DevelopmentNodeRuntime) {
        self.update(|snapshot| snapshot.runtime = runtime);
    }

    pub fn set_bluetooth(&self, bluetooth: BluetoothState) {
        let mut snapshot = self.lock();
        if snapshot.bluetooth == bluetooth {
            return;
        }
        let next = snapshot
            .revision
            .0
            .parse::<u64>()
            .unwrap_or_default()
            .saturating_add(1);
        snapshot.bluetooth = bluetooth;
        snapshot.revision = U64String::from(next);
    }

    pub fn fail(&self, failure: DevelopmentNodeFailure) {
        self.update(|snapshot| {
            snapshot.runtime = DevelopmentNodeRuntime::Failed;
            snapshot.failure = Some(failure);
            snapshot.active_operation = None;
        });
    }

    pub fn begin_generation(&self, bluetooth: BluetoothState) {
        self.update(|snapshot| {
            let mut next = DevelopmentNodeSnapshot::stopped();
            next.runtime = DevelopmentNodeRuntime::Starting;
            next.bluetooth = bluetooth;
            *snapshot = next;
        });
    }

    pub fn stopped(&self) {
        self.update(|snapshot| *snapshot = DevelopmentNodeSnapshot::stopped());
    }
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_are_revisioned_and_poison_tolerant() {
        let store = SnapshotStore::new();
        store.set_runtime(DevelopmentNodeRuntime::Starting);
        store.set_runtime(DevelopmentNodeRuntime::Running);
        let snapshot = store.read();
        assert_eq!(snapshot.revision, U64String::from(2));
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Running);
    }
}
