use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::contract::{
    DevelopmentNodeFailure, DevelopmentNodeOperation, DevelopmentNodeOperationKind,
    DevelopmentNodeRuntime, DevelopmentNodeSnapshot, LocalBluetoothSnapshot, LocalBluetoothState,
    LocalHostState, LocalNetworkSnapshot, LocalNetworkState, LxmfHealth, LxmfHealthState,
    PrimaryIdentityState, RemoteChangeOperation, RemoteChangeStatus,
    RemoteControlAnnounceOperation, RemoteControlAnnounceStatus,
    RemoteControlAnnounceUnknownReason, RemoteWifiOperation, RemoteWifiStatus,
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
        let next = snapshot.revision.saturating_add(1);
        update(&mut snapshot);
        snapshot.revision = next;
    }

    pub fn set_runtime(&self, runtime: DevelopmentNodeRuntime) {
        self.update(|snapshot| {
            snapshot.runtime = runtime;
            if matches!(
                runtime,
                DevelopmentNodeRuntime::Stopping
                    | DevelopmentNodeRuntime::Stopped
                    | DevelopmentNodeRuntime::Failed
            ) {
                snapshot.network = LocalNetworkSnapshot::stopped();
            }
        });
    }

    /// One coherent host/network capture. The generation and phase fence also
    /// prevents a late successful inspector from reviving a stopping node.
    pub(crate) fn publish_host_inspection(
        &self,
        generation: u64,
        host: prns_host::HostSnapshot,
        network: LocalNetworkSnapshot,
        bluetooth: impl FnOnce(Option<bool>) -> LocalBluetoothSnapshot,
    ) -> bool {
        let mut snapshot = self.lock();
        if !can_publish_network(&snapshot, generation, network.activity_revision) {
            return false;
        }
        snapshot.bluetooth = bluetooth(snapshot.bluetooth.desired_enabled);
        snapshot.local_host = LocalHostState::Running {
            host: Box::new(host),
        };
        snapshot.network = network;
        snapshot.revision = snapshot.revision.saturating_add(1);
        true
    }

    /// Actor-only synchronous publication, including the clear revision barrier.
    /// Ring capture and this call must have no intervening await.
    pub(crate) fn publish_network_activity(
        &self,
        generation: u64,
        activity: crate::network::ActivityProjection,
    ) -> bool {
        let mut snapshot = self.lock();
        if !can_publish_network(&snapshot, generation, activity.revision) {
            return false;
        }
        snapshot.network.announces = activity.rows;
        snapshot.network.activity_revision = activity.revision;
        snapshot.network.dropped_announce_count = activity.evictions;
        snapshot.revision = snapshot.revision.saturating_add(1);
        true
    }

    pub fn set_primary_identity(&self, primary_identity: PrimaryIdentityState) {
        let mut snapshot = self.lock();
        if snapshot.primary_identity == primary_identity {
            return;
        }
        let next = snapshot.revision.saturating_add(1);
        snapshot.primary_identity = primary_identity;
        snapshot.revision = next;
    }

    pub fn set_local_host(&self, local_host: LocalHostState) {
        let mut snapshot = self.lock();
        if snapshot.local_host == local_host {
            return;
        }
        let next = snapshot.revision.saturating_add(1);
        snapshot.local_host = local_host;
        snapshot.revision = next;
    }

    #[cfg(test)]
    pub fn set_local_host_unavailable_if_running(&self, detail: String) {
        let mut snapshot = self.lock();
        if snapshot.runtime != DevelopmentNodeRuntime::Running {
            return;
        }
        let local_host = LocalHostState::Unavailable {
            detail: detail.clone(),
        };
        if snapshot.local_host == local_host {
            return;
        }
        let next = snapshot.revision.saturating_add(1);
        snapshot.bluetooth.state = LocalBluetoothState::Unavailable {
            detail: "Local Bluetooth inspection is unavailable.".into(),
        };
        snapshot.bluetooth.peers.clear();
        snapshot.network.state = LocalNetworkState::Unavailable {
            detail: detail.clone(),
        };
        snapshot.network.routes.clear();
        snapshot.local_host = local_host;
        snapshot.revision = next;
    }

    /// An asynchronous read from a retired generation must not mark its successor
    /// unavailable when its own actor or timeout completes later.
    pub fn set_local_host_unavailable_for_generation(&self, generation: u64, detail: String) {
        let mut snapshot = self.lock();
        if snapshot.runtime != DevelopmentNodeRuntime::Running
            || snapshot.generation_id != generation
        {
            return;
        }
        snapshot.network.state = LocalNetworkState::Unavailable {
            detail: detail.clone(),
        };
        snapshot.network.routes.clear();
        snapshot.local_host = LocalHostState::Unavailable { detail };
        snapshot.bluetooth.state = LocalBluetoothState::Unavailable {
            detail: "Local Bluetooth inspection is unavailable.".into(),
        };
        snapshot.bluetooth.peers.clear();
        snapshot.revision = snapshot.revision.saturating_add(1);
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
        let next = snapshot.revision.saturating_add(1);
        snapshot.lxmf.state = LxmfHealthState::Degraded;
        snapshot.revision = next;
    }

    pub fn fail(&self, failure: DevelopmentNodeFailure) {
        self.update(|snapshot| {
            interrupt_announcement(&mut snapshot.last_announcement);
            interrupt_change(&mut snapshot.last_remote_change);
            interrupt_wifi(&mut snapshot.last_remote_wifi);
            let explicit_stop_in_progress = self.explicit_stop_in_progress.load(Ordering::Acquire);
            if !explicit_stop_in_progress {
                snapshot.runtime = DevelopmentNodeRuntime::Failed;
                snapshot.network = LocalNetworkSnapshot::stopped();
                snapshot.bluetooth =
                    LocalBluetoothSnapshot::stopped(snapshot.bluetooth.desired_enabled);
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
            interrupt_change(&mut snapshot.last_remote_change);
            interrupt_wifi(&mut snapshot.last_remote_wifi);
            snapshot.runtime = DevelopmentNodeRuntime::Failed;
            snapshot.network = LocalNetworkSnapshot::stopped();
            snapshot.bluetooth =
                LocalBluetoothSnapshot::stopped(snapshot.bluetooth.desired_enabled);
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

    pub fn begin_stop(&self, started_at_millis: u64) {
        self.explicit_stop_in_progress
            .store(true, Ordering::Release);
        self.update(|snapshot| {
            snapshot.runtime = DevelopmentNodeRuntime::Stopping;
            snapshot.network = LocalNetworkSnapshot::stopped();
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Shutdown,
                started_at_millis,
            });
        });
    }

    pub fn incomplete_stop(&self, failure: DevelopmentNodeFailure, started_at_millis: u64) {
        self.explicit_stop_in_progress
            .store(true, Ordering::Release);
        self.update(|snapshot| {
            snapshot.runtime = DevelopmentNodeRuntime::Stopping;
            snapshot.network = LocalNetworkSnapshot::stopped();
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
            next.bluetooth.desired_enabled = snapshot.bluetooth.desired_enabled;
            next.bluetooth.state = LocalBluetoothState::Starting;
            next.network.state = LocalNetworkState::Starting;
            next.generation_id = snapshot.revision.saturating_add(1);
            next.last_announcement = snapshot.last_announcement.clone();
            interrupt_announcement(&mut next.last_announcement);
            next.last_remote_change = snapshot.last_remote_change.clone();
            interrupt_change(&mut next.last_remote_change);
            next.last_remote_wifi = snapshot.last_remote_wifi.clone();
            interrupt_wifi(&mut next.last_remote_wifi);
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
            let bluetooth_enabled = snapshot.bluetooth.desired_enabled;
            let generation_id = snapshot.generation_id;
            let mut last_announcement = snapshot.last_announcement.clone();
            interrupt_announcement(&mut last_announcement);
            let mut last_remote_change = snapshot.last_remote_change.clone();
            interrupt_change(&mut last_remote_change);
            let mut last_remote_wifi = snapshot.last_remote_wifi.clone();
            interrupt_wifi(&mut last_remote_wifi);
            *snapshot = DevelopmentNodeSnapshot::stopped();
            snapshot.bluetooth.desired_enabled = bluetooth_enabled;
            snapshot.primary_identity = primary_identity;
            snapshot.generation_id = generation_id;
            snapshot.last_announcement = last_announcement;
            snapshot.last_remote_change = last_remote_change;
            snapshot.last_remote_wifi = last_remote_wifi;
        });
    }

    pub fn reset(&self) {
        self.explicit_stop_in_progress
            .store(false, Ordering::Release);
        self.update(|snapshot| *snapshot = DevelopmentNodeSnapshot::stopped());
    }

    pub fn interrupt_pending_operations(&self) {
        self.update(|snapshot| {
            interrupt_announcement(&mut snapshot.last_announcement);
            interrupt_change(&mut snapshot.last_remote_change);
            interrupt_wifi(&mut snapshot.last_remote_wifi);
        });
    }
}

fn can_publish_network(snapshot: &DevelopmentNodeSnapshot, generation: u64, revision: u64) -> bool {
    snapshot.runtime == DevelopmentNodeRuntime::Running
        && snapshot.generation_id == generation
        && revision >= snapshot.network.activity_revision
}

fn interrupt_change(operation: &mut Option<RemoteChangeOperation>) {
    if let Some(operation) = operation {
        if operation.status == RemoteChangeStatus::Pending {
            operation.status = RemoteChangeStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::NodeStopped,
            };
        }
    }
}

fn interrupt_wifi(operation: &mut Option<RemoteWifiOperation>) {
    if let Some(operation) = operation {
        if operation.status == RemoteWifiStatus::Pending {
            operation.status = RemoteWifiStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::NodeStopped,
            };
        }
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

    fn activity(revision: u64) -> crate::network::ActivityProjection {
        crate::network::ActivityProjection {
            revision,
            evictions: 4,
            rows: vec![crate::contract::LocalAnnounceActivity {
                record_id: 10,
                destination: [1; 16],
                announced_identity: [2; 16],
                source_interface: vec![3; 8],
                hops: 2,
                age_millis: 50,
                is_path_response: false,
            }],
        }
    }

    #[test]
    fn activity_publication_rejects_pre_clear_and_retired_generation_captures() {
        let store = SnapshotStore::new();
        store.begin_generation(PrimaryIdentityState::Missing);
        let generation = store.read().generation_id;
        store.set_runtime(DevelopmentNodeRuntime::Running);
        assert!(store.publish_network_activity(generation, activity(1)));
        let stale = activity(1);
        assert!(store.publish_network_activity(
            generation,
            crate::network::ActivityProjection {
                revision: 2,
                evictions: 0,
                rows: Vec::new(),
            }
        ));
        assert!(!store.publish_network_activity(generation, stale));
        assert!(store.read().network.announces.is_empty());
        store.begin_generation(PrimaryIdentityState::Missing);
        store.set_runtime(DevelopmentNodeRuntime::Running);
        assert!(!store.publish_network_activity(generation, activity(100)));
        assert!(store.read().network.announces.is_empty());
    }

    #[test]
    fn network_activity_clears_on_lifecycle_transitions_and_cannot_be_revived() {
        for phase in [
            DevelopmentNodeRuntime::Stopping,
            DevelopmentNodeRuntime::Stopped,
            DevelopmentNodeRuntime::Failed,
        ] {
            let store = SnapshotStore::new();
            store.begin_generation(PrimaryIdentityState::Missing);
            let generation = store.read().generation_id;
            store.set_runtime(DevelopmentNodeRuntime::Running);
            assert!(store.publish_network_activity(generation, activity(1)));
            store.set_runtime(phase);
            assert_eq!(store.read().network, LocalNetworkSnapshot::stopped());
            assert!(!store.publish_network_activity(generation, activity(2)));
        }
        let store = SnapshotStore::new();
        store.set_runtime(DevelopmentNodeRuntime::Running);
        assert!(store.publish_network_activity(0, activity(1)));
        store.begin_stop(1);
        assert_eq!(store.read().network, LocalNetworkSnapshot::stopped());
        store.stopped();
        store.begin_generation(PrimaryIdentityState::Missing);
        assert_eq!(store.read().network.state, LocalNetworkState::Starting);
        store.set_runtime(DevelopmentNodeRuntime::Running);
        assert!(store.publish_network_activity(store.read().generation_id, activity(1)));
        store.fail(DevelopmentNodeFailure {
            stage: crate::contract::DevelopmentNodeFailureStage::Node,
            detail: "failed".into(),
        });
        assert_eq!(store.read().network, LocalNetworkSnapshot::stopped());
        store.reset();
        assert_eq!(store.read().network, LocalNetworkSnapshot::stopped());
    }

    #[test]
    fn inspection_failure_preserves_activity_but_removes_stale_routes() {
        let store = SnapshotStore::new();
        store.set_runtime(DevelopmentNodeRuntime::Running);
        assert!(store.publish_network_activity(0, activity(7)));
        store.update(|snapshot| {
            snapshot.network.state = LocalNetworkState::Ready;
            snapshot
                .network
                .routes
                .push(crate::contract::LocalNetworkRouteSnapshot {
                    destination: [1; 16],
                    via_identity: None,
                    interface_id: vec![3; 8],
                    hops: 1,
                    learned_age_millis: 10,
                    last_activity_age_millis: 0,
                    expires_in_millis: 100,
                    expired: false,
                });
        });
        let history = store.read().network.announces;
        store.set_local_host_unavailable_for_generation(0, "inspection timed out".into());
        let network = store.read().network;
        assert_eq!(network.announces, history);
        assert_eq!(network.activity_revision, 7);
        assert_eq!(network.dropped_announce_count, 4);
        assert!(network.routes.is_empty());
        assert_eq!(
            network.state,
            LocalNetworkState::Unavailable {
                detail: "inspection timed out".into()
            }
        );
    }

    #[test]
    fn stopping_never_rewrites_a_settled_remote_change() {
        for status in [
            RemoteChangeStatus::Applied,
            RemoteChangeStatus::Unchanged,
            RemoteChangeStatus::Scheduled,
            RemoteChangeStatus::Failed {
                stage: crate::contract::RemoteManagementFailureStage::Persistence,
                detail: "could not save".to_owned(),
            },
            RemoteChangeStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::ConnectionLost,
            },
        ] {
            let store = SnapshotStore::new();
            store.update(|snapshot| {
                snapshot.last_remote_change = Some(RemoteChangeOperation {
                    operation_id: 10,
                    generation_id: snapshot.generation_id,
                    target_identity_fingerprint: vec![3; 16],
                    change: crate::contract::RemoteNodeChange::WakeRadios,
                    status: status.clone(),
                })
            });
            store.interrupt_pending_operations();
            store.stopped();
            store.begin_generation(PrimaryIdentityState::Missing);
            assert_eq!(store.read().last_remote_change.unwrap().status, status);
        }
    }

    #[test]
    fn change_result_survives_stop_and_restart_but_reset_clears_it() {
        let store = SnapshotStore::new();
        store.update(|snapshot| {
            snapshot.last_remote_change = Some(RemoteChangeOperation {
                operation_id: 9,
                generation_id: snapshot.generation_id,
                target_identity_fingerprint: vec![2; 16],
                change: crate::contract::RemoteNodeChange::SleepRadios,
                status: RemoteChangeStatus::Pending,
            });
        });
        store.stopped();
        let interrupted = store.read().last_remote_change;
        assert!(matches!(
            interrupted.as_ref().map(|operation| &operation.status),
            Some(RemoteChangeStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::NodeStopped
            })
        ));
        store.begin_generation(PrimaryIdentityState::Missing);
        assert_eq!(store.read().last_remote_change, interrupted);
        store.reset();
        assert_eq!(store.read().last_remote_change, None);
    }

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
                operation_id: 7,
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
        assert_eq!(snapshot.revision, 2);
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

        store.incomplete_stop(failure.clone(), 42);

        let snapshot = store.read();
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Stopping);
        assert_eq!(snapshot.local_host, local_host);
        assert_eq!(snapshot.failure, Some(failure));
        assert_eq!(
            snapshot.active_operation,
            Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Shutdown,
                started_at_millis: 42,
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
        store.begin_stop(42);
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
