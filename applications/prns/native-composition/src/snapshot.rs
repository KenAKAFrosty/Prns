use std::sync::{Mutex, MutexGuard};

use crate::contract::{
    DevelopmentNodeFailure, DevelopmentNodeRuntime, DevelopmentNodeSnapshot, LocalHostState,
    LxmfHealth, PrimaryIdentityState, U64String,
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

    pub fn set_primary_identity(&self, primary_identity: PrimaryIdentityState) {
        let mut snapshot = self.lock();
        if snapshot.primary_identity == primary_identity {
            return;
        }
        let next = snapshot
            .revision
            .0
            .parse::<u64>()
            .unwrap_or_default()
            .saturating_add(1);
        snapshot.primary_identity = primary_identity;
        snapshot.revision = U64String::from(next);
    }

    pub fn set_local_host(&self, local_host: LocalHostState) {
        let mut snapshot = self.lock();
        if snapshot.local_host == local_host {
            return;
        }
        let next = snapshot
            .revision
            .0
            .parse::<u64>()
            .unwrap_or_default()
            .saturating_add(1);
        snapshot.local_host = local_host;
        snapshot.revision = U64String::from(next);
    }

    pub fn set_local_host_unavailable_if_running(&self, detail: String) {
        let mut snapshot = self.lock();
        if snapshot.runtime != DevelopmentNodeRuntime::Running {
            return;
        }
        let local_host = LocalHostState::Unavailable { detail };
        if snapshot.local_host == local_host {
            return;
        }
        let next = snapshot
            .revision
            .0
            .parse::<u64>()
            .unwrap_or_default()
            .saturating_add(1);
        snapshot.local_host = local_host;
        snapshot.revision = U64String::from(next);
    }

    /// Publish an LXMF refresh hint through the aggregate's existing revision.
    ///
    /// The health value can remain unchanged while peer or message query state
    /// changes, so every service notification deliberately advances revision.
    pub fn refresh_lxmf(&self, lxmf: LxmfHealth) {
        self.update(|snapshot| snapshot.lxmf = lxmf);
    }

    pub fn fail(&self, failure: DevelopmentNodeFailure) {
        self.update(|snapshot| {
            snapshot.runtime = DevelopmentNodeRuntime::Failed;
            if !matches!(
                &snapshot.local_host,
                LocalHostState::DevelopmentResetRequired { .. }
            ) {
                snapshot.local_host = LocalHostState::Stopped {
                    last_start_failure: Some(failure.detail.clone()),
                };
            }
            snapshot.failure = Some(failure);
            snapshot.active_operation = None;
        });
    }

    pub fn begin_generation(&self, primary_identity: PrimaryIdentityState) {
        self.update(|snapshot| {
            let mut next = DevelopmentNodeSnapshot::stopped();
            next.runtime = DevelopmentNodeRuntime::Starting;
            next.primary_identity = primary_identity;
            *snapshot = next;
        });
    }

    pub fn stopped(&self) {
        self.update(|snapshot| {
            let primary_identity = snapshot.primary_identity.clone();
            *snapshot = DevelopmentNodeSnapshot::stopped();
            snapshot.primary_identity = primary_identity;
        });
    }

    pub fn reset(&self) {
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

    #[test]
    fn snapshot_unavailability_only_replaces_a_running_generation() {
        let store = SnapshotStore::new();
        store.set_local_host_unavailable_if_running("stale failure".to_owned());
        assert!(matches!(
            store.read().local_host,
            LocalHostState::Stopped { .. }
        ));

        store.set_runtime(DevelopmentNodeRuntime::Running);
        store.set_local_host_unavailable_if_running("active failure".to_owned());
        assert_eq!(
            store.read().local_host,
            LocalHostState::Unavailable {
                detail: "active failure".to_owned(),
            }
        );

        store.stopped();
        store.set_local_host_unavailable_if_running("late failure".to_owned());
        assert!(matches!(
            store.read().local_host,
            LocalHostState::Stopped { .. }
        ));
    }
}
