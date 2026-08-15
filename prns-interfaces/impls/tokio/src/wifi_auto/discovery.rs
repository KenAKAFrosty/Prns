use std::num::NonZeroU8;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use prns_core::interfaces::wifi_auto::DiscoverySnapshot;
use tokio::sync::watch;

/// Whether this process may consume platform service-discovery resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryParticipation {
    Inactive,
    Satellite,
    Core,
}

/// Result of replacing the latest visible service-discovery state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPublication {
    Published,
    NotCore(DiscoveryParticipation),
    CapacityMismatch {
        expected: NonZeroU8,
        actual: NonZeroU8,
    },
}

struct WorkSignal {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl WorkSignal {
    fn new() -> Self {
        Self {
            generation: Mutex::new(0),
            changed: Condvar::new(),
        }
    }

    fn generation(&self) -> u64 {
        self.generation.lock().map(|value| *value).unwrap_or(0)
    }

    fn wake(&self) {
        if let Ok(mut generation) = self.generation.lock() {
            *generation = generation.wrapping_add(1);
            self.changed.notify_all();
        }
    }

    fn wait(&self, observed: u64, timeout: Option<Duration>) -> u64 {
        let Ok(mut generation) = self.generation.lock() else {
            return observed.wrapping_add(1);
        };
        match timeout {
            Some(timeout) if *generation == observed => {
                let Ok((next, _)) = self.changed.wait_timeout(generation, timeout) else {
                    return observed.wrapping_add(1);
                };
                generation = next;
            }
            None => {
                while *generation == observed {
                    let Ok(next) = self.changed.wait(generation) else {
                        return observed.wrapping_add(1);
                    };
                    generation = next;
                }
            }
            Some(_) => {}
        }
        *generation
    }
}

/// Runtime-owned side of a bounded, latest-state service-discovery channel.
pub struct ServiceDiscovery {
    snapshots: watch::Receiver<DiscoverySnapshot>,
    participation: watch::Sender<DiscoveryParticipation>,
    work: Arc<WorkSignal>,
}

impl ServiceDiscovery {
    /// Creates a channel with a platform-selected capacity of 1–255 advertisements.
    #[must_use]
    pub fn channel(capacity: NonZeroU8) -> (Self, ServiceDiscoveryPublisher) {
        let (snapshots, snapshot_receiver) = watch::channel(DiscoverySnapshot::new(capacity));
        let (participation, participation_receiver) =
            watch::channel(DiscoveryParticipation::Inactive);
        let work = Arc::new(WorkSignal::new());
        (
            Self {
                snapshots: snapshot_receiver,
                participation,
                work: Arc::clone(&work),
            },
            ServiceDiscoveryPublisher {
                snapshots,
                participation: participation_receiver,
                work,
                capacity,
            },
        )
    }

    pub(crate) fn set_participation(&self, participation: DiscoveryParticipation) {
        let previous = self.participation.send_replace(participation);
        if previous != participation {
            self.work.wake();
        }
    }

    pub(crate) async fn next_snapshot(&mut self) -> Option<DiscoverySnapshot> {
        self.snapshots.changed().await.ok()?;
        Some(self.snapshots.borrow_and_update().clone())
    }

    #[cfg(test)]
    pub(crate) fn current_snapshot(&self) -> DiscoverySnapshot {
        self.snapshots.borrow().clone()
    }
}

impl Drop for ServiceDiscovery {
    fn drop(&mut self) {
        let previous = self
            .participation
            .send_replace(DiscoveryParticipation::Inactive);
        if previous != DiscoveryParticipation::Inactive {
            self.work.wake();
        }
    }
}

/// Platform-owned side of a [`ServiceDiscovery`] channel.
#[derive(Clone)]
pub struct ServiceDiscoveryPublisher {
    snapshots: watch::Sender<DiscoverySnapshot>,
    participation: watch::Receiver<DiscoveryParticipation>,
    work: Arc<WorkSignal>,
    capacity: NonZeroU8,
}

impl ServiceDiscoveryPublisher {
    /// Returns the platform budget selected when the channel was created.
    #[must_use]
    pub const fn capacity(&self) -> NonZeroU8 {
        self.capacity
    }

    #[must_use]
    pub fn participation(&self) -> DiscoveryParticipation {
        *self.participation.borrow()
    }

    pub async fn wait_for_participation_change(&mut self) -> Option<DiscoveryParticipation> {
        self.participation.changed().await.ok()?;
        Some(*self.participation.borrow_and_update())
    }

    /// Waits for a specific lifecycle state without polling or missing an
    /// already-published transition.
    pub async fn wait_for_participation(
        &mut self,
        expected: DiscoveryParticipation,
    ) -> Option<DiscoveryParticipation> {
        let observed = self
            .participation
            .wait_for(|current| *current == expected)
            .await
            .ok()?;
        Some(*observed)
    }

    /// Replaces the latest complete snapshot without queueing intermediate state.
    #[must_use]
    pub fn replace_snapshot(&self, snapshot: DiscoverySnapshot) -> SnapshotPublication {
        let participation = self.participation();
        if participation != DiscoveryParticipation::Core {
            return SnapshotPublication::NotCore(participation);
        }
        if snapshot.capacity() != self.capacity {
            return SnapshotPublication::CapacityMismatch {
                expected: self.capacity,
                actual: snapshot.capacity(),
            };
        }
        self.snapshots.send_replace(snapshot);
        SnapshotPublication::Published
    }

    /// Removes all currently visible advertisements while preserving the budget.
    pub fn clear_snapshot(&self) {
        self.snapshots
            .send_replace(DiscoverySnapshot::new(self.capacity));
    }

    #[must_use]
    pub fn work_generation(&self) -> u64 {
        self.work.generation()
    }

    #[must_use]
    pub fn wait_for_work(&self, observed: u64, timeout_millis: u64) -> u64 {
        let timeout = (timeout_millis != 0).then(|| Duration::from_millis(timeout_millis));
        self.work.wait(observed, timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::wifi_auto::{
        DiscoveryEndpoint, DiscoveryServiceName, ServiceAdvertisement,
    };

    const TEST_CAPACITY: NonZeroU8 = NonZeroU8::new(4).unwrap();
    const OTHER_CAPACITY: NonZeroU8 = NonZeroU8::new(5).unwrap();

    fn one_peer(name: &str) -> DiscoverySnapshot {
        let mut advertisement =
            ServiceAdvertisement::new(DiscoveryServiceName::new(name).expect("test service name"));
        let endpoint =
            DiscoveryEndpoint::new("192.168.1.8:42699".parse().expect("test endpoint parses"))
                .expect("test endpoint is valid");
        let _ = advertisement.insert(endpoint);
        let mut snapshot = DiscoverySnapshot::new(TEST_CAPACITY);
        let _ = snapshot.insert(advertisement);
        snapshot
    }

    #[tokio::test]
    async fn latest_snapshot_replaces_queued_work_and_non_core_clears_it() {
        let (mut discovery, publisher) = ServiceDiscovery::channel(TEST_CAPACITY);
        assert_eq!(
            publisher.replace_snapshot(one_peer("inactive")),
            SnapshotPublication::NotCore(DiscoveryParticipation::Inactive)
        );
        discovery.set_participation(DiscoveryParticipation::Core);
        assert_eq!(
            publisher.replace_snapshot(one_peer("first")),
            SnapshotPublication::Published
        );
        assert_eq!(
            publisher.replace_snapshot(one_peer("latest")),
            SnapshotPublication::Published
        );
        let observed = discovery.next_snapshot().await.expect("snapshot changed");
        assert_eq!(observed, one_peer("latest"));

        discovery.set_participation(DiscoveryParticipation::Satellite);
        publisher.clear_snapshot();
        assert!(discovery.current_snapshot().is_empty());
        assert_eq!(discovery.current_snapshot().capacity(), TEST_CAPACITY);
        assert_eq!(
            publisher.replace_snapshot(one_peer("satellite")),
            SnapshotPublication::NotCore(DiscoveryParticipation::Satellite)
        );
    }

    #[test]
    fn publisher_rejects_a_snapshot_with_another_platform_budget() {
        let (discovery, publisher) = ServiceDiscovery::channel(TEST_CAPACITY);
        discovery.set_participation(DiscoveryParticipation::Core);
        let snapshot = DiscoverySnapshot::new(OTHER_CAPACITY);
        assert_eq!(
            publisher.replace_snapshot(snapshot),
            SnapshotPublication::CapacityMismatch {
                expected: TEST_CAPACITY,
                actual: OTHER_CAPACITY,
            }
        );
        assert_eq!(publisher.capacity(), TEST_CAPACITY);
    }

    #[tokio::test]
    async fn dropping_the_discovery_owner_terminates_provider_lifecycle() {
        let (discovery, mut publisher) = ServiceDiscovery::channel(TEST_CAPACITY);
        discovery.set_participation(DiscoveryParticipation::Core);
        assert_eq!(
            publisher.wait_for_participation_change().await,
            Some(DiscoveryParticipation::Core)
        );
        drop(discovery);
        assert_eq!(
            publisher.wait_for_participation_change().await,
            Some(DiscoveryParticipation::Inactive)
        );
        assert_eq!(publisher.wait_for_participation_change().await, None);
    }
}
