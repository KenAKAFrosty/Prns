//! Bounded authenticated observations and pre-admission recipient lookup.

use std::collections::BTreeMap;
use std::time::Duration;

use prns_lxmf_wire::{normalize_lxmf_display_name, parse_lxmf_announce};
use tokio::sync::{watch, Mutex, Semaphore};
use tokio::time::Instant;

use super::{DirectNetwork, DirectSendFailure, LxmfPeer, PeerJob, MAX_DISPLAY_NAME_BYTES};

pub const DISCOVERED_PEER_CAPACITY: usize = 256;
pub const DISCOVERED_PEER_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const RECIPIENT_RESOLUTION_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PENDING_RECIPIENT_LOOKUPS: usize = 8;

struct Observation {
    peer: LxmfPeer,
    received: Instant,
    discovered: bool,
}

impl Observation {
    fn project(&self, now: Instant) -> LxmfPeer {
        let mut peer = self.peer.clone();
        peer.age_millis = u64::try_from(now.saturating_duration_since(self.received).as_millis())
            .unwrap_or(u64::MAX);
        peer
    }
}

/// One bounded metadata owner, with separate visibility for cleared discovery history.
#[derive(Default)]
pub(crate) struct PeerCache {
    observations: BTreeMap<[u8; 16], Observation>,
    cleared_at: Option<Instant>,
}

impl PeerCache {
    fn expire(&mut self, now: Instant) {
        self.observations.retain(|_, observation| {
            now.saturating_duration_since(observation.received) < DISCOVERED_PEER_MAX_AGE
        });
    }

    pub(crate) fn observe(&mut self, job: PeerJob) -> bool {
        let Ok(announce) = parse_lxmf_announce(&job.app_data, MAX_DISPLAY_NAME_BYTES) else {
            return false;
        };
        let now = Instant::now();
        self.expire(now);
        if now.saturating_duration_since(job.observed_at) >= DISCOVERED_PEER_MAX_AGE {
            return false;
        }
        if self
            .observations
            .get(&job.destination)
            .is_some_and(|current| {
                current.received > job.observed_at
                    || current.peer.announced_identity != job.announced_identity
            })
        {
            return false;
        }
        if !self.observations.contains_key(&job.destination)
            && self.observations.len() == DISCOVERED_PEER_CAPACITY
        {
            if let Some(oldest) = self
                .observations
                .iter()
                .min_by_key(|(destination, entry)| (entry.received, **destination))
                .map(|(destination, _)| *destination)
            {
                self.observations.remove(&oldest);
            }
        }
        let mut normalized = [0_u8; MAX_DISPLAY_NAME_BYTES];
        let display_name = announce.display_name.and_then(|bytes| {
            normalize_lxmf_display_name(bytes, &mut normalized)
                .ok()
                .map(String::from)
        });
        self.observations.insert(
            job.destination,
            Observation {
                peer: LxmfPeer {
                    destination: job.destination,
                    announced_identity: job.announced_identity,
                    display_name,
                    required_stamp_cost: announce.required_stamp_cost,
                    observed_at_millis: job.observed_at_millis,
                    age_millis: 0,
                    source_interface: job.source_interface,
                    hops: job.hops,
                    is_path_response: job.is_path_response,
                },
                received: job.observed_at,
                discovered: self
                    .cleared_at
                    .is_none_or(|cleared| job.observed_at > cleared),
            },
        );
        true
    }

    pub(crate) fn discovered(&mut self) -> Vec<LxmfPeer> {
        let now = Instant::now();
        self.expire(now);
        self.observations
            .values()
            .filter(|entry| entry.discovered)
            .map(|entry| entry.project(now))
            .collect()
    }

    pub(crate) fn recipient(&mut self, destination: &[u8; 16]) -> Option<LxmfPeer> {
        let now = Instant::now();
        self.expire(now);
        self.observations
            .get(destination)
            .map(|entry| entry.project(now))
    }

    pub(crate) fn clear_discovered(&mut self) {
        self.cleared_at = Some(Instant::now());
        for entry in self.observations.values_mut() {
            entry.discovered = false;
        }
    }
}

pub(crate) enum ResolvePeerFailure {
    TimedOut,
    Stopped,
    Unavailable,
    Busy,
}

pub(crate) struct PeerResolver {
    slots: Semaphore,
}

impl Default for PeerResolver {
    fn default() -> Self {
        Self {
            slots: Semaphore::new(MAX_PENDING_RECIPIENT_LOOKUPS),
        }
    }
}

impl PeerResolver {
    /// No message is composed, persisted, or sent here. Dropping this future can
    /// leave only a harmless path request, never an abandoned message submission.
    pub(crate) async fn resolve(
        &self,
        destination: [u8; 16],
        peers: &Mutex<PeerCache>,
        network: &dyn DirectNetwork,
        mut refresh: watch::Receiver<u64>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<LxmfPeer, ResolvePeerFailure> {
        if *shutdown.borrow() {
            return Err(ResolvePeerFailure::Stopped);
        }
        let retained = peers.lock().await.recipient(&destination);
        if let Some(peer) = retained {
            return Ok(peer);
        }
        let _slot = self
            .slots
            .try_acquire()
            .map_err(|_| ResolvePeerFailure::Busy)?;
        let lookup = async {
            // The public RequestPath command emits even if routing already has a
            // cached path. A route/public key alone says nothing about LXMF stamps.
            network
                .request_path(destination)
                .await
                .map_err(|error| match error {
                    DirectSendFailure::LocalNodeStopped => ResolvePeerFailure::Stopped,
                    DirectSendFailure::NoRoute | DirectSendFailure::DeliveryTimedOut => {
                        ResolvePeerFailure::TimedOut
                    }
                    DirectSendFailure::LinkFailed => ResolvePeerFailure::Unavailable,
                })?;
            loop {
                let observed = peers.lock().await.recipient(&destination);
                if let Some(peer) = observed {
                    return Ok(peer);
                }
                // Path settlement and accepted-announce callback processing use
                // different lanes. Await the service observer, not just the route.
                refresh
                    .changed()
                    .await
                    .map_err(|_| ResolvePeerFailure::Stopped)?;
            }
        };
        tokio::select! {
            biased;
            _ = shutdown.changed() => Err(ResolvePeerFailure::Stopped),
            result = tokio::time::timeout(RECIPIENT_RESOLUTION_TIMEOUT, lookup) => {
                result.unwrap_or(Err(ResolvePeerFailure::TimedOut))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u16) -> PeerJob {
        let mut destination = [0_u8; 16];
        destination[..2].copy_from_slice(&id.to_be_bytes());
        PeerJob {
            destination,
            announced_identity: [5; 16],
            app_data: vec![0x93, 0xc4, 0x04, b'p', b'e', b'e', b'r', 0xc0, 0x90],
            observed_at_millis: u64::MAX,
            observed_at: Instant::now(),
            source_interface: [7; 8],
            hops: 3,
            is_path_response: false,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn discovery_is_bounded_deduplicated_and_evicts_oldest_observation() {
        let mut peers = PeerCache::default();
        for id in 0..256 {
            assert!(peers.observe(job(id)));
            tokio::time::advance(Duration::from_millis(1)).await;
        }
        assert_eq!(peers.discovered().len(), DISCOVERED_PEER_CAPACITY);
        assert!(peers.observe(job(0)));
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(peers.observe(job(256)));
        assert_eq!(peers.discovered().len(), DISCOVERED_PEER_CAPACITY);
        assert!(peers.recipient(&job(0).destination).is_some());
        assert!(peers.recipient(&job(1).destination).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn discovery_expires_by_monotonic_age_and_repeat_resets_age() {
        let mut peers = PeerCache::default();
        assert!(peers.observe(job(1)));
        tokio::time::advance(DISCOVERED_PEER_MAX_AGE - Duration::from_millis(1)).await;
        let first = peers.discovered().pop().unwrap();
        assert_eq!(first.age_millis, 86_399_999);
        assert_eq!(
            first.observed_at_millis,
            u64::MAX,
            "unrelated engine epoch is preserved"
        );
        assert_eq!((first.source_interface, first.hops), ([7; 8], 3));
        assert!(peers.observe(job(1)));
        assert_eq!(peers.discovered()[0].age_millis, 0);
        tokio::time::advance(DISCOVERED_PEER_MAX_AGE).await;
        assert!(peers.discovered().is_empty());
        assert!(peers.recipient(&job(1).destination).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn clearing_discovery_retains_resolution_and_does_not_resurface_queued_history() {
        let mut peers = PeerCache::default();
        assert!(peers.observe(job(1)));
        let queued_before_clear = job(2);
        peers.clear_discovered();
        assert!(peers.discovered().is_empty());
        assert!(peers.recipient(&job(1).destination).is_some());
        assert!(peers.observe(queued_before_clear));
        assert!(peers.discovered().is_empty());
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(peers.observe(job(2)));
        assert_eq!(peers.discovered().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn stale_queued_and_identity_conflicting_metadata_never_replaces_current_peer() {
        let mut peers = PeerCache::default();
        let stale = job(1);
        tokio::time::advance(Duration::from_millis(1)).await;
        let mut current = job(1);
        current.is_path_response = true;
        assert!(peers.observe(current));
        assert!(!peers.observe(stale));
        let mut conflicting = job(1);
        conflicting.announced_identity = [9; 16];
        assert!(!peers.observe(conflicting));
        assert!(peers.discovered()[0].is_path_response);
        assert_eq!(peers.discovered()[0].announced_identity, [5; 16]);
    }
}
