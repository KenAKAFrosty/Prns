use std::num::NonZeroU8;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use prns_core::interfaces::wifi_auto::DiscoverySnapshot;
use tokio::sync::watch;

/// An acknowledged owner for a platform discovery pump. Dropping an AutoWifi
/// supervisor closes its channel; explicit retirement also joins this task so
/// native browsing/advertising resources are gone before a replacement starts.
#[derive(Clone)]
pub struct ServiceDiscoveryTask {
    inner: Arc<DiscoveryTaskInner>,
}

struct DiscoveryTaskInner {
    stop: watch::Sender<bool>,
    state: tokio::sync::Mutex<DiscoveryTaskState>,
    diagnostic: DiscoveryDiagnostic,
}

#[derive(Clone, Default)]
pub(crate) struct DiscoveryDiagnostic(Arc<Mutex<DiscoveryDiagnosticState>>);

#[derive(Default)]
struct DiscoveryDiagnosticState {
    detail: Option<String>,
    active: bool,
}

impl DiscoveryDiagnostic {
    #[cfg(any(
        test,
        all(
            feature = "wifi-auto-apple",
            any(target_os = "macos", target_os = "ios")
        )
    ))]
    pub(crate) fn set(&self, detail: Option<String>) {
        if let Ok(mut current) = self.0.lock() {
            current.detail = detail;
        }
    }

    fn detail(&self) -> Option<String> {
        self.0.lock().ok().and_then(|state| state.detail.clone())
    }

    #[cfg(any(
        test,
        all(
            feature = "wifi-auto-apple",
            any(target_os = "macos", target_os = "ios")
        )
    ))]
    pub(crate) fn set_active(&self, active: bool) {
        if let Ok(mut current) = self.0.lock() {
            current.active = active;
        }
    }
}

struct DiscoveryTaskState {
    task: Option<tokio::task::JoinHandle<()>>,
    result: Option<Result<(), String>>,
}

impl ServiceDiscoveryTask {
    #[cfg(test)]
    pub(crate) fn spawn(pump: impl std::future::Future<Output = ()> + Send + 'static) -> Self {
        Self::spawn_reporting(|_| pump)
    }

    #[cfg(any(
        test,
        all(
            feature = "wifi-auto-apple",
            any(target_os = "macos", target_os = "ios")
        )
    ))]
    pub(crate) fn spawn_reporting<F>(pump: impl FnOnce(DiscoveryDiagnostic) -> F) -> Self
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let (stop, mut stopping) = watch::channel(false);
        let diagnostic = DiscoveryDiagnostic::default();
        let pump = pump(diagnostic.clone());
        let task = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = stopping.wait_for(|requested| *requested) => {},
                _ = pump => {},
            }
        });
        Self {
            inner: Arc::new(DiscoveryTaskInner {
                stop,
                diagnostic,
                state: tokio::sync::Mutex::new(DiscoveryTaskState {
                    task: Some(task),
                    result: None,
                }),
            }),
        }
    }

    /// The latest observed provider failure, cleared when the backend recovers.
    /// This is a backend diagnostic, not an inferred permission state.
    pub fn detail(&self) -> Option<String> {
        self.inner.diagnostic.detail()
    }

    pub fn is_active(&self) -> bool {
        self.inner
            .diagnostic
            .0
            .lock()
            .is_ok_and(|state| state.active)
    }

    /// Cancellation of a caller's wait does not lose the task or its ownership.
    pub async fn stop(&self) -> Result<(), String> {
        self.inner.stop.send_replace(true);
        let mut state = self.inner.state.lock().await;
        if let Some(result) = &state.result {
            return result.clone();
        }
        let result = match state.task.as_mut() {
            Some(task) => task
                .await
                .map_err(|error| format!("platform discovery shutdown failed: {error}")),
            None => Ok(()),
        };
        state.task = None;
        state.result = Some(result.clone());
        if let Ok(mut diagnostic) = self.inner.diagnostic.0.lock() {
            diagnostic.active = false;
        }
        result
    }
}

impl Drop for DiscoveryTaskInner {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

/// Whether (or how) this process may consume platform service-discovery resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryParticipation {
    Inactive,
    Satellite,
    Central,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryLifecycleError {
    Closed,
}

impl std::fmt::Display for DiscoveryLifecycleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => formatter.write_str("service-discovery lifecycle closed"),
        }
    }
}

impl std::error::Error for DiscoveryLifecycleError {}

#[derive(Debug)]
pub(super) enum DiscoverySnapshotError {
    PublisherClosed,
}

/// Result of replacing the latest visible service-discovery state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPublication {
    Published,
    NotCentral(DiscoveryParticipation),
    CapacityMismatch {
        expected: NonZeroU8,
        actual: NonZeroU8,
    },
}

struct WorkSignal {
    generation: Mutex<u64>,
    generation_changed: Condvar,
}

impl WorkSignal {
    fn new() -> Self {
        Self {
            generation: Mutex::new(0),
            generation_changed: Condvar::new(),
        }
    }

    fn generation(&self) -> u64 {
        self.generation
            .lock()
            .map(|current_generation| *current_generation)
            .unwrap_or(0)
    }

    fn wake(&self) {
        if let Ok(mut current_generation) = self.generation.lock() {
            *current_generation = current_generation.wrapping_add(1);
            self.generation_changed.notify_all();
        }
    }

    fn wait(&self, observed_generation: u64, timeout: Option<Duration>) -> u64 {
        let Ok(mut current_generation) = self.generation.lock() else {
            return observed_generation.wrapping_add(1);
        };
        match timeout {
            Some(timeout) if *current_generation == observed_generation => {
                let Ok((next_generation, _timeout_result)) = self
                    .generation_changed
                    .wait_timeout(current_generation, timeout)
                else {
                    return observed_generation.wrapping_add(1);
                };
                current_generation = next_generation;
            }
            None => {
                while *current_generation == observed_generation {
                    let Ok(next_generation) = self.generation_changed.wait(current_generation)
                    else {
                        return observed_generation.wrapping_add(1);
                    };
                    current_generation = next_generation;
                }
            }
            Some(_) => {}
        }
        *current_generation
    }
}

/// Runtime-owned side of a bounded, latest-state service-discovery channel.
pub struct ServiceDiscovery {
    snapshot_receiver: watch::Receiver<DiscoverySnapshot>,
    participation_sender: watch::Sender<DiscoveryParticipation>,
    work_signal: Arc<WorkSignal>,
    network_changes: watch::Sender<u64>,
}

impl ServiceDiscovery {
    /// Creates a channel with a platform-selected capacity of 1–255 advertisements.
    #[must_use]
    pub fn channel(advertisement_capacity: NonZeroU8) -> (Self, ServiceDiscoveryPublisher) {
        let (snapshot_sender, snapshot_receiver) =
            watch::channel(DiscoverySnapshot::new(advertisement_capacity));
        let (participation_sender, participation_receiver) =
            watch::channel(DiscoveryParticipation::Inactive);
        let work_signal = Arc::new(WorkSignal::new());
        let (network_changes, network_change_receiver) = watch::channel(0u64);
        (
            Self {
                snapshot_receiver,
                participation_sender,
                work_signal: Arc::clone(&work_signal),
                network_changes,
            },
            ServiceDiscoveryPublisher {
                snapshot_sender,
                participation_receiver,
                work_signal,
                advertisement_capacity,
                network_changes: network_change_receiver,
            },
        )
    }

    pub(crate) fn set_participation(&self, new_participation: DiscoveryParticipation) {
        let previous_participation = self.participation_sender.send_replace(new_participation);
        if previous_participation != new_participation {
            self.work_signal.wake();
        }
    }

    pub(crate) fn network_changed(&self) {
        self.network_changes
            .send_modify(|revision| *revision = revision.saturating_add(1));
        self.work_signal.wake();
    }

    pub(super) async fn next_snapshot(
        &mut self,
    ) -> Result<DiscoverySnapshot, DiscoverySnapshotError> {
        self.snapshot_receiver
            .changed()
            .await
            .map_err(|_publisher_closed| DiscoverySnapshotError::PublisherClosed)?;
        Ok(self.snapshot_receiver.borrow_and_update().clone())
    }

    #[cfg(test)]
    pub(crate) fn current_snapshot(&self) -> DiscoverySnapshot {
        self.snapshot_receiver.borrow().clone()
    }
}

impl Drop for ServiceDiscovery {
    fn drop(&mut self) {
        let previous_participation = self
            .participation_sender
            .send_replace(DiscoveryParticipation::Inactive);
        if previous_participation != DiscoveryParticipation::Inactive {
            self.work_signal.wake();
        }
    }
}

/// Platform-owned side of a [`ServiceDiscovery`] channel.
#[derive(Clone)]
pub struct ServiceDiscoveryPublisher {
    snapshot_sender: watch::Sender<DiscoverySnapshot>,
    participation_receiver: watch::Receiver<DiscoveryParticipation>,
    work_signal: Arc<WorkSignal>,
    advertisement_capacity: NonZeroU8,
    network_changes: watch::Receiver<u64>,
}

impl ServiceDiscoveryPublisher {
    #[cfg(any(
        test,
        all(
            feature = "wifi-auto-apple",
            any(target_os = "macos", target_os = "ios")
        )
    ))]
    /// A replacement Apple session starts from the current network state. Only
    /// changes after this boundary should cancel its newly constructed backend.
    pub fn subscribe_network_changes(&self) -> watch::Receiver<u64> {
        let mut changes = self.network_changes.clone();
        changes.borrow_and_update();
        changes
    }
    /// Coalesces platform network changes so a provider can republish its current addresses.
    pub async fn wait_for_network_change(&mut self) -> Result<(), DiscoveryLifecycleError> {
        self.network_changes
            .changed()
            .await
            .map_err(|_| DiscoveryLifecycleError::Closed)
    }
    /// Returns the platform budget selected when the channel was created.
    #[must_use]
    pub const fn capacity(&self) -> NonZeroU8 {
        self.advertisement_capacity
    }

    #[must_use]
    pub fn participation(&self) -> DiscoveryParticipation {
        *self.participation_receiver.borrow()
    }

    /// Waits for the next lifecycle transition and returns its new state.
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryLifecycleError::Closed`] when the runtime-owned
    /// [`ServiceDiscovery`] has been dropped.
    pub async fn wait_for_participation_change(
        &mut self,
    ) -> Result<DiscoveryParticipation, DiscoveryLifecycleError> {
        self.participation_receiver
            .changed()
            .await
            .map_err(|_lifecycle_closed| DiscoveryLifecycleError::Closed)?;
        Ok(*self.participation_receiver.borrow_and_update())
    }

    /// Waits for a specific lifecycle state without polling or missing an
    /// already-published transition.
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryLifecycleError::Closed`] when the runtime-owned
    /// [`ServiceDiscovery`] is dropped before reaching `expected`.
    pub async fn wait_for_participation(
        &mut self,
        expected: DiscoveryParticipation,
    ) -> Result<(), DiscoveryLifecycleError> {
        self.participation_receiver
            .wait_for(|current_participation| *current_participation == expected)
            .await
            .map_err(|_lifecycle_closed| DiscoveryLifecycleError::Closed)?;
        Ok(())
    }

    /// Replaces the latest complete snapshot without queueing intermediate state.
    #[must_use]
    pub fn replace_snapshot(&self, discovery_snapshot: DiscoverySnapshot) -> SnapshotPublication {
        let current_participation = self.participation();
        if current_participation != DiscoveryParticipation::Central {
            return SnapshotPublication::NotCentral(current_participation);
        }
        if discovery_snapshot.capacity() != self.advertisement_capacity {
            return SnapshotPublication::CapacityMismatch {
                expected: self.advertisement_capacity,
                actual: discovery_snapshot.capacity(),
            };
        }
        self.snapshot_sender.send_replace(discovery_snapshot);
        SnapshotPublication::Published
    }

    /// Removes all currently visible advertisements while preserving the budget.
    pub fn clear_snapshot(&self) {
        self.snapshot_sender
            .send_replace(DiscoverySnapshot::new(self.advertisement_capacity));
    }

    #[must_use]
    pub fn work_generation(&self) -> u64 {
        self.work_signal.generation()
    }

    #[must_use]
    pub fn wait_for_work(&self, observed_generation: u64, timeout_millis: u64) -> u64 {
        let timeout = (timeout_millis != 0).then(|| Duration::from_millis(timeout_millis));
        self.work_signal.wait(observed_generation, timeout)
    }

    /// Wakes blocking platform pumps during an outer lifecycle transition.
    pub fn wake_waiters(&self) {
        self.work_signal.wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::wifi_auto::{
        DiscoveryEndpoint, DiscoveryServiceName, DiscoveryTransport, ServiceAdvertisement,
    };

    const TEST_CAPACITY: NonZeroU8 = NonZeroU8::new(4).unwrap();
    const OTHER_CAPACITY: NonZeroU8 = NonZeroU8::new(5).unwrap();

    #[tokio::test]
    async fn provider_diagnostic_reports_observation_and_clears_on_recovery() {
        let (ready, observed) = tokio::sync::oneshot::channel();
        let (recover, recovered) = tokio::sync::oneshot::channel();
        let (active, activated) = tokio::sync::oneshot::channel();
        let owner = ServiceDiscoveryTask::spawn_reporting(move |diagnostic| async move {
            diagnostic.set(Some("native backend closed".into()));
            let _ = ready.send(());
            let _ = recovered.await;
            diagnostic.set(None);
            diagnostic.set_active(true);
            let _ = active.send(());
            std::future::pending::<()>().await;
        });
        observed.await.expect("provider observed a failure");
        assert_eq!(owner.detail().as_deref(), Some("native backend closed"));
        assert!(!owner.is_active());
        recover.send(()).expect("provider remains active");
        activated.await.expect("backend activated after recovery");
        assert_eq!(owner.detail(), None);
        assert!(owner.is_active());
        owner.stop().await.expect("joined provider");
        assert!(!owner.is_active());
    }

    #[tokio::test]
    async fn managed_provider_retirement_joins_resources_and_survives_caller_loss() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Resource(Arc<AtomicBool>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let released = Arc::new(AtomicBool::new(false));
        let resource = Resource(Arc::clone(&released));
        let owner = ServiceDiscoveryTask::spawn(async move {
            let _resource = resource;
            std::future::pending::<()>().await;
        });
        // Losing a waiter cannot lose the join handle or manufacture completion.
        let _ = tokio::time::timeout(Duration::ZERO, owner.stop()).await;
        assert_eq!(owner.stop().await, Ok(()));
        assert!(released.load(Ordering::Acquire));
        assert_eq!(owner.stop().await, Ok(()));
    }

    #[tokio::test]
    async fn network_changes_are_coalesced_for_the_existing_platform_owner() {
        let (discovery, publisher) = ServiceDiscovery::channel(TEST_CAPACITY);
        let mut changes = publisher.subscribe_network_changes();
        discovery.network_changed();
        discovery.network_changed();
        changes.changed().await.expect("platform network change");
        assert_eq!(*changes.borrow_and_update(), 2);
        assert!(!changes.has_changed().expect("live owner"));
    }

    #[tokio::test]
    async fn replacement_apple_subscription_ignores_history_and_observes_new_network_changes() {
        let (discovery, publisher) = ServiceDiscovery::channel(TEST_CAPACITY);
        let old_session = publisher.subscribe_network_changes();
        discovery.network_changed();
        discovery.network_changed();
        assert!(old_session.has_changed().expect("live owner"));

        // The publisher's original watch receiver has never consumed these
        // revisions. A plain clone would replay them on every replacement,
        // continuously cancelling new Apple backends before they can settle.
        let mut replacement = publisher.subscribe_network_changes();
        assert_eq!(*replacement.borrow(), 2);
        assert!(!replacement
            .has_changed()
            .expect("history already represented"));
        discovery.network_changed();
        replacement.changed().await.expect("genuinely new change");
        assert_eq!(*replacement.borrow_and_update(), 3);

        let mut next_replacement = publisher.subscribe_network_changes();
        assert!(!next_replacement
            .has_changed()
            .expect("replacement begins current"));
        discovery.network_changed();
        next_replacement
            .changed()
            .await
            .expect("later network return");
        assert_eq!(*next_replacement.borrow_and_update(), 4);
    }

    fn snapshot_for_peer(service_name: &str) -> DiscoverySnapshot {
        let mut service_advertisement = ServiceAdvertisement::new(
            DiscoveryServiceName::from_instance(service_name, DiscoveryTransport::Tcp)
                .expect("test service name"),
        );
        let discovery_endpoint =
            DiscoveryEndpoint::tcp("192.168.1.8:42699".parse().expect("test endpoint parses"))
                .expect("test endpoint is valid");
        let _ = service_advertisement.insert(discovery_endpoint);
        let mut discovery_snapshot = DiscoverySnapshot::new(TEST_CAPACITY);
        let _ = discovery_snapshot.insert(service_advertisement);
        discovery_snapshot
    }

    #[tokio::test]
    async fn latest_snapshot_replaces_queued_work_and_non_central_clears_it() {
        let (mut service_discovery, service_discovery_publisher) =
            ServiceDiscovery::channel(TEST_CAPACITY);
        assert_eq!(
            service_discovery_publisher.replace_snapshot(snapshot_for_peer("inactive")),
            SnapshotPublication::NotCentral(DiscoveryParticipation::Inactive)
        );
        service_discovery.set_participation(DiscoveryParticipation::Central);
        assert_eq!(
            service_discovery_publisher.replace_snapshot(snapshot_for_peer("first")),
            SnapshotPublication::Published
        );
        assert_eq!(
            service_discovery_publisher.replace_snapshot(snapshot_for_peer("latest")),
            SnapshotPublication::Published
        );
        let latest_snapshot = service_discovery
            .next_snapshot()
            .await
            .expect("snapshot changed");
        assert_eq!(latest_snapshot, snapshot_for_peer("latest"));

        service_discovery.set_participation(DiscoveryParticipation::Satellite);
        service_discovery_publisher.clear_snapshot();
        assert!(service_discovery.current_snapshot().is_empty());
        assert_eq!(
            service_discovery.current_snapshot().capacity(),
            TEST_CAPACITY
        );
        assert_eq!(
            service_discovery_publisher.replace_snapshot(snapshot_for_peer("satellite")),
            SnapshotPublication::NotCentral(DiscoveryParticipation::Satellite)
        );
    }

    #[test]
    fn publisher_rejects_a_snapshot_with_another_platform_budget() {
        let (service_discovery, service_discovery_publisher) =
            ServiceDiscovery::channel(TEST_CAPACITY);
        service_discovery.set_participation(DiscoveryParticipation::Central);
        let mismatched_snapshot = DiscoverySnapshot::new(OTHER_CAPACITY);
        assert_eq!(
            service_discovery_publisher.replace_snapshot(mismatched_snapshot),
            SnapshotPublication::CapacityMismatch {
                expected: TEST_CAPACITY,
                actual: OTHER_CAPACITY,
            }
        );
        assert_eq!(service_discovery_publisher.capacity(), TEST_CAPACITY);
    }

    #[tokio::test]
    async fn dropping_the_discovery_owner_terminates_provider_lifecycle() {
        let (service_discovery, mut service_discovery_publisher) =
            ServiceDiscovery::channel(TEST_CAPACITY);
        service_discovery.set_participation(DiscoveryParticipation::Central);
        assert_eq!(
            service_discovery_publisher
                .wait_for_participation_change()
                .await,
            Ok(DiscoveryParticipation::Central)
        );
        drop(service_discovery);
        assert_eq!(
            service_discovery_publisher
                .wait_for_participation_change()
                .await,
            Ok(DiscoveryParticipation::Inactive)
        );
        assert_eq!(
            service_discovery_publisher
                .wait_for_participation_change()
                .await,
            Err(DiscoveryLifecycleError::Closed)
        );
    }
}
