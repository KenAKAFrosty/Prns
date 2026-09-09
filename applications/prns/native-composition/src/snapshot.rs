use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::contract::{
    DevelopmentNodeFailure, DevelopmentNodeOperation, DevelopmentNodeOperationKind,
    DevelopmentNodeRuntime, DevelopmentNodeSnapshot, LocalHostState, LxmfHealth, LxmfHealthState,
    PrimaryIdentityState, RemoteControlAnnounceOperation, RemoteControlAnnounceStatus,
    RemoteControlAnnounceUnknownReason, U64String,
};

pub struct SnapshotStore {
    inner: Mutex<DevelopmentNodeSnapshot>,
    explicit_stop_in_progress: AtomicBool,
}

impl SnapshotStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(DevelopmentNodeSnapshot::stopped()),
            explicit_stop_in_progress: AtomicBool::new(false),
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
        let next = snapshot.revision.0.saturating_add(1);
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
        let next = snapshot.revision.0.saturating_add(1);
        snapshot.primary_identity = primary_identity;
        snapshot.revision = U64String::from(next);
    }

    pub fn set_local_host(&self, local_host: LocalHostState) {
        let mut snapshot = self.lock();
        if snapshot.local_host == local_host {
            return;
        }
        let next = snapshot.revision.0.saturating_add(1);
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
        let next = snapshot.revision.0.saturating_add(1);
        snapshot.local_host = local_host;
        snapshot.revision = U64String::from(next);
    }

    /// An asynchronous read from a retired generation must not mark its successor
    /// unavailable when its own actor or timeout completes later.
    #[cfg(feature = "uniffi-bindings")]
    pub fn set_local_host_unavailable_for_generation(&self, generation: U64String, detail: String) {
        let mut snapshot = self.lock();
        if snapshot.runtime != DevelopmentNodeRuntime::Running
            || snapshot.generation_id != generation
        {
            return;
        }
        snapshot.local_host = LocalHostState::Unavailable { detail };
        snapshot.revision = U64String(snapshot.revision.0.saturating_add(1));
    }

    /// Publish an LXMF refresh hint through the aggregate's existing revision.
    ///
    /// The health value can remain unchanged while peer or message query state
    /// changes, so every service notification deliberately advances revision.
    pub fn refresh_lxmf(&self, lxmf: LxmfHealth) {
        self.update(|snapshot| snapshot.lxmf = lxmf);
    }

    /// Retain the last aggregate data while marking transient LXMF access loss.
    pub fn set_lxmf_degraded(&self) {
        let mut snapshot = self.lock();
        if snapshot.lxmf.state == LxmfHealthState::Degraded {
            return;
        }
        let next = snapshot.revision.0.saturating_add(1);
        snapshot.lxmf.state = LxmfHealthState::Degraded;
        snapshot.revision = U64String::from(next);
    }

    pub fn fail(&self, failure: DevelopmentNodeFailure) {
        self.update(|snapshot| {
            interrupt_announcement(&mut snapshot.last_announcement);
            let explicit_stop_in_progress = self.explicit_stop_in_progress.load(Ordering::Acquire);
            if !explicit_stop_in_progress {
                snapshot.runtime = DevelopmentNodeRuntime::Failed;
                if !matches!(
                    &snapshot.local_host,
                    LocalHostState::DevelopmentResetRequired { .. }
                ) {
                    snapshot.local_host = LocalHostState::Stopped {
                        last_start_failure: Some(failure.detail.clone()),
                    };
                }
                snapshot.active_operation = None;
            }
            snapshot.failure = Some(failure);
        });
    }

    pub fn terminal_fail(&self, failure: DevelopmentNodeFailure) {
        self.explicit_stop_in_progress
            .store(false, Ordering::Release);
        self.update(|snapshot| {
            interrupt_announcement(&mut snapshot.last_announcement);
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

    pub fn begin_stop(&self, started_at_millis: U64String) {
        self.explicit_stop_in_progress
            .store(true, Ordering::Release);
        self.update(|snapshot| {
            snapshot.runtime = DevelopmentNodeRuntime::Stopping;
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Shutdown,
                started_at_millis,
            });
        });
    }

    pub fn incomplete_stop(&self, failure: DevelopmentNodeFailure, started_at_millis: U64String) {
        self.explicit_stop_in_progress
            .store(true, Ordering::Release);
        self.update(|snapshot| {
            snapshot.runtime = DevelopmentNodeRuntime::Stopping;
            snapshot.failure = Some(failure);
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Shutdown,
                started_at_millis,
            });
        });
    }

    pub fn is_explicit_stop_in_progress(&self) -> bool {
        self.explicit_stop_in_progress.load(Ordering::Acquire)
    }

    pub fn begin_generation(&self, primary_identity: PrimaryIdentityState) {
        self.explicit_stop_in_progress
            .store(false, Ordering::Release);
        self.update(|snapshot| {
            let mut next = DevelopmentNodeSnapshot::stopped();
            next.generation_id = U64String::from(snapshot.revision.0.saturating_add(1));
            next.last_announcement = snapshot.last_announcement.clone();
            interrupt_announcement(&mut next.last_announcement);
            next.runtime = DevelopmentNodeRuntime::Starting;
            next.primary_identity = primary_identity;
            *snapshot = next;
        });
    }

    pub fn stopped(&self) {
        self.explicit_stop_in_progress
            .store(false, Ordering::Release);
        self.update(|snapshot| {
            let primary_identity = snapshot.primary_identity.clone();
            let generation_id = snapshot.generation_id.clone();
            let mut last_announcement = snapshot.last_announcement.clone();
            interrupt_announcement(&mut last_announcement);
            *snapshot = DevelopmentNodeSnapshot::stopped();
            snapshot.primary_identity = primary_identity;
            snapshot.generation_id = generation_id;
            snapshot.last_announcement = last_announcement;
        });
    }

    pub fn reset(&self) {
        self.explicit_stop_in_progress
            .store(false, Ordering::Release);
        self.update(|snapshot| *snapshot = DevelopmentNodeSnapshot::stopped());
    }

    pub fn interrupt_announcement(&self) {
        self.update(|snapshot| interrupt_announcement(&mut snapshot.last_announcement));
    }
}

fn interrupt_announcement(operation: &mut Option<RemoteControlAnnounceOperation>) {
    if let Some(operation) = operation {
        if operation.status == RemoteControlAnnounceStatus::Pending {
            operation.status = RemoteControlAnnounceStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::NodeStopped,
            };
        }
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
    fn generation_identity_changes_only_when_a_native_generation_begins() {
        let store = SnapshotStore::new();
        store.begin_generation(PrimaryIdentityState::Missing);
        let first = store.read().generation_id;
        store.set_runtime(DevelopmentNodeRuntime::Running);
        assert_eq!(store.read().generation_id, first);
        store.fail(DevelopmentNodeFailure {
            stage: crate::contract::DevelopmentNodeFailureStage::Runtime,
            detail: "stopped unexpectedly".to_owned(),
        });
        assert_eq!(store.read().generation_id, first);
        store.stopped();
        assert_eq!(store.read().generation_id, first);
        store.begin_generation(PrimaryIdentityState::Missing);
        let second = store.read().generation_id;
        assert_ne!(second, first);
        store.reset();
        store.begin_generation(PrimaryIdentityState::Missing);
        assert_ne!(store.read().generation_id, first);
        assert_ne!(store.read().generation_id, second);
    }

    #[test]
    fn announcement_result_survives_stop_and_restart_but_reset_clears_it() {
        let store = SnapshotStore::new();
        store.update(|snapshot| {
            snapshot.last_announcement = Some(RemoteControlAnnounceOperation {
                operation_id: U64String::from(7),
                target_identity_fingerprint: vec![1; 16],
                status: RemoteControlAnnounceStatus::Pending,
            })
        });
        store.stopped();
        let interrupted = store.read().last_announcement;
        assert!(matches!(
            interrupted.as_ref().map(|operation| &operation.status),
            Some(RemoteControlAnnounceStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::NodeStopped
            })
        ));
        store.begin_generation(PrimaryIdentityState::Missing);
        assert_eq!(store.read().last_announcement, interrupted);
        store.reset();
        assert!(store.read().last_announcement.is_none());
    }

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
    fn incomplete_stop_retains_the_transition_and_failure() {
        let store = SnapshotStore::new();
        let local_host = LocalHostState::Unavailable {
            detail: "last observed host".to_owned(),
        };
        store.set_runtime(DevelopmentNodeRuntime::Running);
        store.set_local_host(local_host.clone());
        let failure = DevelopmentNodeFailure {
            stage: crate::contract::DevelopmentNodeFailureStage::Runtime,
            detail: "shutdown remains incomplete".to_owned(),
        };

        store.incomplete_stop(failure.clone(), U64String::from(42));

        let snapshot = store.read();
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Stopping);
        assert_eq!(snapshot.local_host, local_host);
        assert_eq!(snapshot.failure, Some(failure));
        assert_eq!(
            snapshot.active_operation,
            Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Shutdown,
                started_at_millis: U64String::from(42),
            })
        );
    }

    #[test]
    fn failure_during_stop_does_not_collapse_the_transition() {
        let store = SnapshotStore::new();
        let local_host = LocalHostState::Unavailable {
            detail: "last observed host".to_owned(),
        };
        store.set_runtime(DevelopmentNodeRuntime::Running);
        store.set_local_host(local_host.clone());
        store.begin_stop(U64String::from(42));
        store.update(|snapshot| snapshot.active_operation = None);
        let failure = DevelopmentNodeFailure {
            stage: crate::contract::DevelopmentNodeFailureStage::Runtime,
            detail: "late teardown failure".to_owned(),
        };

        store.fail(failure.clone());

        let snapshot = store.read();
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Stopping);
        assert_eq!(snapshot.local_host, local_host);
        assert_eq!(snapshot.failure, Some(failure));
        assert!(snapshot.active_operation.is_none());
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
