//! Bounded native retention of authenticated public availability, before event
//! routing. There is no event consumer, persistent store, deadline invention, or
//! automatic pairing. The protocol's existing deadline is evaluated by its clock.
use crate::remote_control_service::{
    RemoteControlPairingDiscoveryInspection, RemoteControlPairingDiscoverySnapshot,
};
use crate::{lock, HostRuntime, HostStartError, HostSubmitError};
use personal_rns::remote_control::{
    RemoteControlPairingAvailabilityObservation, RemoteControlPairingAvailabilitySnapshot,
};
use personal_rns::units::InstantMillis;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const CAPACITY: usize = 64;
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(crate) struct Discovery {
    generation: u64,
    evicted: u64,
    observations: Vec<RemoteControlPairingAvailabilitySnapshot>,
}
impl Discovery {
    pub(crate) fn new() -> Result<Self, HostStartError> {
        let generation = NEXT_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| {
                HostStartError::Runtime("pairing discovery generation exhausted".into())
            })?;
        Ok(Self {
            generation,
            evicted: 0,
            observations: Vec::new(),
        })
    }
    pub(crate) fn observe(
        &mut self,
        value: &RemoteControlPairingAvailabilityObservation<'_>,
    ) -> Result<(), personal_rns::remote_control::RemoteControlPairingPublicAppDataError> {
        self.insert(value.try_into()?);
        Ok(())
    }
    fn insert(&mut self, value: RemoteControlPairingAvailabilitySnapshot) {
        // This event's native ingress timestamp is a conservative pruning bound.
        // Query additionally prunes against the current, identical runtime clock.
        self.prune(value.observed_at);
        if let Some(previous) = self
            .observations
            .iter_mut()
            .find(|item| item.endpoint == value.endpoint)
        {
            if value.observed_at > previous.observed_at {
                *previous = value;
            }
            return;
        }
        if self.observations.len() >= CAPACITY {
            // Fixed work and memory: prefer the most recent observations and
            // report every capacity omission, including an older new endpoint.
            self.evicted = self.evicted.saturating_add(1);
            match self
                .observations
                .iter()
                .enumerate()
                .min_by_key(|(_, item)| item.observed_at)
            {
                Some((oldest, previous)) if value.observed_at >= previous.observed_at => {
                    self.observations.remove(oldest);
                }
                _ => return,
            }
        }
        self.observations.push(value);
    }
    fn prune(&mut self, now: InstantMillis) {
        self.observations.retain(|item| item.expires_at > now);
    }
    pub(crate) fn snapshot(&mut self, now: InstantMillis) -> RemoteControlPairingDiscoverySnapshot {
        self.prune(now);
        RemoteControlPairingDiscoverySnapshot {
            generation: self.generation,
            observed_at: now,
            capacity: CAPACITY as u32,
            evicted: self.evicted,
            observations: self.observations.clone(),
        }
    }
}

impl RemoteControlPairingDiscoveryInspection for HostRuntime {
    async fn remote_control_pairing_discovery(
        &self,
    ) -> Result<RemoteControlPairingDiscoverySnapshot, HostSubmitError> {
        if self.stopped.load(Ordering::Acquire)
            || *self.shutdown.borrow()
            || self.commands.is_closed()
        {
            return Err(HostSubmitError::Stopped);
        }
        Ok(lock(&self.pairing_discovery).snapshot(self.clock.now()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::{
        interfaces::InterfaceId,
        remote_control::{RemoteControlPairingEndpoint, RemoteControlPairingPublicAppDataBytes},
        units::HopCount,
        wire::DestinationHash,
    };
    fn item(id: u8, observed: u64, expires: u64) -> RemoteControlPairingAvailabilitySnapshot {
        RemoteControlPairingAvailabilitySnapshot {
            endpoint: RemoteControlPairingEndpoint::from_destination_hash(DestinationHash::new(
                [id; 16],
            )),
            observed_at: InstantMillis(observed),
            expires_at: InstantMillis(expires),
            hops: HopCount(1),
            source_interface: InterfaceId::new([7; 8]),
            public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(b"public".as_slice())
                .unwrap(),
        }
    }
    #[test]
    fn discovery_is_empty_lazy_and_generations_are_distinct() {
        let mut one = Discovery::new().unwrap();
        let mut two = Discovery::new().unwrap();
        assert_eq!(one.observations.capacity(), 0);
        let snapshot = one.snapshot(InstantMillis(1));
        assert!(snapshot.observations.is_empty());
        assert_eq!(snapshot.capacity, 64);
        assert_eq!(snapshot.evicted, 0);
        assert_ne!(
            snapshot.generation,
            two.snapshot(InstantMillis(1)).generation
        );
    }
    #[test]
    fn discovery_duplicates_and_older_observations_do_not_extend_deadlines() {
        let mut cache = Discovery::new().unwrap();
        cache.insert(item(1, 5, 15));
        cache.insert(item(1, 5, 25));
        cache.insert(item(1, 4, 30));
        assert_eq!(
            cache.snapshot(InstantMillis(6)).observations,
            vec![item(1, 5, 15)]
        );
        cache.insert(item(1, 7, 17));
        assert_eq!(
            cache.snapshot(InstantMillis(8)).observations,
            vec![item(1, 7, 17)]
        );
        assert!(cache.snapshot(InstantMillis(17)).observations.is_empty());
    }
    #[test]
    fn discovery_capacity_evicts_oldest_without_expanding_memory_or_returning_expired() {
        let mut cache = Discovery::new().unwrap();
        for id in 1..=64 {
            cache.insert(item(id, u64::from(id), 1_000));
        }
        cache.insert(item(65, 65, 1_000));
        let snapshot = cache.snapshot(InstantMillis(66));
        assert_eq!(snapshot.observations.len(), CAPACITY);
        assert_eq!(snapshot.evicted, 1);
        assert!(!snapshot
            .observations
            .iter()
            .any(|entry| entry.endpoint == item(1, 0, 0).endpoint));
        cache.insert(item(66, 0, 1_000));
        assert_eq!(
            cache.snapshot(InstantMillis(67)).observations.len(),
            CAPACITY
        );
        assert_eq!(cache.snapshot(InstantMillis(1_000)).evicted, 2);
        assert!(cache.snapshot(InstantMillis(1_000)).observations.is_empty());
    }
    #[test]
    fn discovery_expiration_reclaims_capacity_before_eviction_and_snapshots_are_owned() {
        let mut cache = Discovery::new().unwrap();
        for id in 1..=64 {
            cache.insert(item(id, 1, 5));
        }
        let held = cache.snapshot(InstantMillis(2));
        cache.insert(item(65, 5, 10));
        let latest = cache.snapshot(InstantMillis(6));
        assert_eq!(latest.observations, vec![item(65, 5, 10)]);
        assert_eq!(latest.evicted, 0);
        assert_eq!(held.observations.len(), 64);
        assert_eq!(held.observed_at, InstantMillis(2));
    }
}
