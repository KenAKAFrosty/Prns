use std::num::NonZeroU8;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use prns_core::interfaces::wifi_auto::DiscoverySnapshot;
use tokio::sync::watch;

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
        (
            Self {
                snapshot_receiver,
                participation_sender,
                work_signal: Arc::clone(&work_signal),
            },
            ServiceDiscoveryPublisher {
                snapshot_sender,
                participation_receiver,
                work_signal,
                advertisement_capacity,
            },
        )
    }

    pub(crate) fn set_participation(&self, new_participation: DiscoveryParticipation) {
        let previous_participation = self.participation_sender.send_replace(new_participation);
        if previous_participation != new_participation {
            self.work_signal.wake();
        }
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
}

impl ServiceDiscoveryPublisher {
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
