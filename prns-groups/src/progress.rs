//! Bounded, disposable observations shared by native consumers. The durable
//! record/change/receipt APIs remain the authority for delivery and recovery.

use crate::{Error, NetworkConfig};
use prns_groups_core::sync::{SyncIssue, SyncPhase};
use prns_groups_service::{ControlReference, Endpoint, GroupId};
use prns_groups_store::StoreIncarnation;
use prns_host::IdentityHash;
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

const MAX_PEERS: usize = 256;
const MAX_GROUPS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct GroupSyncProgress {
    /// The runner future currently owns the observation source. This does not
    /// mean a peer is connected, the OS will schedule it, or a record arrived.
    pub running: bool,
    /// All peers in the last observed configuration fit the disposable budget.
    /// False is unknown or partial, never proof of a delivery failure.
    pub observed: bool,
    /// The last observed membership. Compare this with current conversation
    /// detail before presenting peer observations as belonging to that view.
    pub control: Option<ControlReference>,
    pub peers: Vec<PeerSyncProgress>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct PeerSyncProgress {
    pub endpoint: Endpoint,
    pub identity: IdentityHash,
    pub phase: SyncPhase,
    /// Presence of durable authored work at the last bounded store query.
    /// None means unknown, not an empty outbox. It is not a current record count.
    pub pending: Option<bool>,
    /// Remaining local scheduling delay, not an estimated delivery time.
    pub retry_after_millis: Option<u64>,
    pub failures: u8,
    pub updated_ago_millis: u64,
    pub last_issue: Option<SyncIssue>,
}

struct Entry {
    group: GroupId,
    control: ControlReference,
    incarnation: StoreIncarnation,
    peer: PeerSyncProgress,
    observed: Instant,
    due: Option<Instant>,
}

struct GroupObservation {
    group: GroupId,
    control: ControlReference,
    complete: bool,
}

#[derive(Default)]
struct State {
    running: bool,
    generation: u64,
    groups: Vec<GroupObservation>,
    peers: Vec<Entry>,
}

#[derive(Default)]
pub(crate) struct ProgressTracker(Mutex<State>);

/// Read-only native observation source. It carries no submission, network or
/// durable-storage authority and is not exposed as a foreign mutable object.
#[derive(Clone)]
pub struct SyncProgressSource(Arc<ProgressTracker>);
impl SyncProgressSource {
    pub(crate) fn new(tracker: Arc<ProgressTracker>) -> Self {
        Self(tracker)
    }
    pub fn snapshot(&self, group: GroupId) -> GroupSyncProgress {
        self.0.snapshot(group)
    }
}

pub(crate) struct ProgressLease(Arc<ProgressTracker>, u64);

pub(crate) struct PeerObserver {
    tracker: Arc<ProgressTracker>,
    generation: u64,
    group: GroupId,
    control: ControlReference,
    incarnation: StoreIncarnation,
    endpoint: Endpoint,
}
impl PeerObserver {
    pub fn new(tracker: Arc<ProgressTracker>, config: &NetworkConfig, endpoint: Endpoint) -> Self {
        Self {
            generation: tracker.generation(),
            tracker,
            group: config.group,
            control: config.control,
            incarnation: config.incarnation,
            endpoint,
        }
    }
    pub fn update(
        &self,
        phase: SyncPhase,
        pending: Option<bool>,
        due: Option<Instant>,
        failures: u8,
        issue: Option<SyncIssue>,
    ) {
        self.tracker.update(
            self.generation,
            self.group,
            self.endpoint,
            self.control,
            self.incarnation,
            phase,
            pending,
            due,
            failures,
            issue,
        );
    }
}
impl Drop for ProgressLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0 .0.lock() {
            if state.generation == self.1 {
                state.running = false;
            }
        }
    }
}

impl ProgressTracker {
    pub fn start(self: &Arc<Self>) -> Result<ProgressLease, Error> {
        let mut state = self.0.lock().map_err(|_| Error::Unavailable)?;
        // One scheduling owner per Client tree. Prevent two runners from
        // issuing conflicting status and doubling the same peer's attempts.
        if state.running {
            return Err(Error::Configuration);
        }
        state.generation = state.generation.checked_add(1).ok_or(Error::Unavailable)?;
        state.running = true;
        state.groups.clear();
        state.peers.clear();
        Ok(ProgressLease(Arc::clone(self), state.generation))
    }

    pub fn generation(&self) -> u64 {
        self.0.lock().map(|state| state.generation).unwrap_or(0)
    }

    pub fn configure(&self, configs: &[NetworkConfig]) {
        let Ok(mut state) = self.0.lock() else { return };
        if !state.running {
            return;
        }
        // Configurations arrive one at a time from a bounded catalog traversal.
        // An empty selection says nothing about the other observed groups.
        for config in configs.iter().take(MAX_GROUPS) {
            state.groups.retain(|entry| entry.group != config.group);
            state.peers.retain(|entry| {
                entry.group != config.group
                    || (entry.control == config.control
                        && entry.incarnation == config.incarnation
                        && config.peers.iter().any(|peer| {
                            peer.endpoint != config.local
                                && entry.peer.endpoint == peer.endpoint
                                && entry.peer.identity == peer.identity
                        }))
            });
            let count = config
                .peers
                .iter()
                .filter(|peer| peer.endpoint != config.local)
                .count();
            let wanted = count.min(MAX_PEERS);
            while state.groups.len() >= MAX_GROUPS
                || state
                    .peers
                    .iter()
                    .filter(|entry| entry.group != config.group)
                    .count()
                    + wanted
                    > MAX_PEERS
            {
                let evicted = state.groups.remove(0).group;
                state.peers.retain(|entry| entry.group != evicted);
            }
            for peer in config
                .peers
                .iter()
                .filter(|peer| peer.endpoint != config.local)
                .take(MAX_PEERS)
            {
                if state.peers.iter().any(|entry| {
                    entry.group == config.group && entry.peer.endpoint == peer.endpoint
                }) {
                    continue;
                }
                if state.peers.len() == MAX_PEERS {
                    break;
                }
                state.peers.push(Entry {
                    group: config.group,
                    control: config.control,
                    incarnation: config.incarnation,
                    peer: PeerSyncProgress {
                        endpoint: peer.endpoint,
                        identity: peer.identity,
                        phase: SyncPhase::Unknown,
                        pending: None,
                        retry_after_millis: None,
                        failures: 0,
                        updated_ago_millis: 0,
                        last_issue: None,
                    },
                    observed: Instant::now(),
                    due: None,
                });
            }
            state.groups.push(GroupObservation {
                group: config.group,
                control: config.control,
                complete: count <= MAX_PEERS,
            });
        }
    }

    pub fn forget(&self, group: GroupId) {
        let Ok(mut state) = self.0.lock() else { return };
        state.groups.retain(|entry| entry.group != group);
        state.peers.retain(|entry| entry.group != group);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        generation: u64,
        group: GroupId,
        endpoint: Endpoint,
        control: ControlReference,
        incarnation: StoreIncarnation,
        phase: SyncPhase,
        pending: Option<bool>,
        due: Option<Instant>,
        failures: u8,
        issue: Option<SyncIssue>,
    ) {
        let Ok(mut state) = self.0.lock() else { return };
        if !state.running || state.generation != generation {
            return;
        }
        if let Some(entry) = state.peers.iter_mut().find(|entry| {
            entry.group == group
                && entry.peer.endpoint == endpoint
                && entry.control == control
                && entry.incarnation == incarnation
        }) {
            entry.peer.phase = phase;
            entry.peer.pending = pending;
            entry.peer.failures = failures;
            entry.peer.last_issue = issue;
            entry.due = due;
            entry.observed = Instant::now();
        }
    }

    pub fn unavailable(&self, group: GroupId, issue: SyncIssue) {
        let Ok(mut state) = self.0.lock() else { return };
        for entry in state.peers.iter_mut().filter(|entry| entry.group == group) {
            entry.peer.phase = issue_phase(issue);
            entry.peer.pending = None;
            entry.peer.last_issue = Some(issue);
            entry.due = None;
            entry.observed = Instant::now();
        }
    }

    pub fn snapshot(&self, group: GroupId) -> GroupSyncProgress {
        let Ok(state) = self.0.lock() else {
            return GroupSyncProgress {
                running: false,
                observed: false,
                control: None,
                peers: Vec::new(),
            };
        };
        let now = Instant::now();
        let configuration = state.groups.iter().find(|entry| entry.group == group);
        GroupSyncProgress {
            running: state.running,
            observed: state.running && configuration.is_some_and(|entry| entry.complete),
            control: configuration.map(|entry| entry.control),
            peers: state
                .peers
                .iter()
                .filter(|entry| entry.group == group)
                .map(|entry| {
                    let mut peer = entry.peer.clone();
                    peer.updated_ago_millis =
                        crate::schedule::millis(now.saturating_duration_since(entry.observed));
                    peer.retry_after_millis = if state.running {
                        entry
                            .due
                            .map(|due| crate::schedule::millis(due.saturating_duration_since(now)))
                    } else {
                        None
                    };
                    if !state.running {
                        peer.phase = SyncPhase::Unknown;
                        peer.pending = None;
                        peer.last_issue = None;
                    }
                    peer
                })
                .collect(),
        }
    }
}

pub(crate) fn issue(error: &Error) -> SyncIssue {
    if error.is_storage_full() {
        return SyncIssue::StorageFull;
    }
    match error {
        Error::Busy => SyncIssue::StorageBusy,
        Error::Clock => SyncIssue::TimeUncertain,
        Error::NotProvisioned => SyncIssue::MembershipChanged,
        Error::Configuration => SyncIssue::Configuration,
        Error::Transport(prns_groups_reticulum::Error::Service(error)) => service_issue(error),
        Error::Transport(prns_groups_reticulum::Error::Remote(
            prns_groups_reticulum::RemoteError::Unauthorized
            | prns_groups_reticulum::RemoteError::UnknownGroup,
        )) => SyncIssue::MembershipChanged,
        Error::Transport(_) => SyncIssue::Transport,
        Error::Storage(_) | Error::Unavailable | Error::Corrupt => SyncIssue::StorageUnavailable,
        Error::Service(error) => service_issue(error),
        _ => SyncIssue::Configuration,
    }
}

fn service_issue(
    error: &prns_groups_service::Error<prns_groups_store_redb::RedbError>,
) -> SyncIssue {
    use prns_groups_service::{Error as ServiceError, Fault};
    match error {
        ServiceError::Fault(Fault::ConfigurationChanged) => SyncIssue::MembershipChanged,
        ServiceError::Fault(Fault::TimeUncertain) => SyncIssue::TimeUncertain,
        ServiceError::Fault(Fault::Capacity) => SyncIssue::StorageFull,
        ServiceError::Fault(Fault::ApplicationStorageFault | Fault::Corrupt)
        | ServiceError::Store(_) => SyncIssue::StorageUnavailable,
        _ => SyncIssue::Rejected,
    }
}

pub(crate) fn issue_phase(issue: SyncIssue) -> SyncPhase {
    match issue {
        SyncIssue::StorageBusy | SyncIssue::StorageFull | SyncIssue::StorageUnavailable => {
            SyncPhase::StorageBlocked
        }
        SyncIssue::MembershipChanged => SyncPhase::MembershipCatchUp,
        _ => SyncPhase::RetryScheduled,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use prns_groups_core::{AuthorizationEra, ControlId, EndpointId, EndpointIncarnation};

    fn config() -> NetworkConfig {
        let peer = Endpoint {
            id: EndpointId([2; 32]),
            incarnation: EndpointIncarnation::new(1).unwrap(),
        };
        NetworkConfig {
            group: GroupId([1; 32]),
            control: ControlReference {
                head: ControlId([3; 32]),
                era: AuthorizationEra(ControlId([3; 32])),
            },
            incarnation: StoreIncarnation::new([4; 24]).unwrap(),
            local: Endpoint {
                id: EndpointId([5; 32]),
                ..peer
            },
            local_reticulum_identity: IdentityHash::new([6; 16]),
            readers: vec![peer],
            peers: vec![prns_groups_reticulum::Peer {
                endpoint: peer,
                identity: IdentityHash::new([7; 16]),
                destination: prns_host::DestinationHash::new([8; 16]),
            }],
        }
    }

    #[tokio::test(start_paused = true)]
    async fn observations_expire_on_owner_drop_and_restart_and_never_claim_receipts() {
        let tracker = Arc::new(ProgressTracker::default());
        let config = config();
        assert!(!tracker.snapshot(config.group).observed);
        let lease = tracker.start().unwrap();
        assert!(tracker.start().is_err());
        tracker.configure(std::slice::from_ref(&config));
        let observer = PeerObserver::new(Arc::clone(&tracker), &config, config.peers[0].endpoint);
        observer.update(
            SyncPhase::RetryScheduled,
            Some(true),
            Some(Instant::now() + std::time::Duration::from_secs(30)),
            3,
            Some(SyncIssue::Transport),
        );
        tokio::time::advance(std::time::Duration::from_secs(12)).await;
        let snapshot = tracker.snapshot(config.group);
        assert!(snapshot.running && snapshot.observed);
        assert_eq!(snapshot.peers[0].pending, Some(true));
        assert_eq!(snapshot.peers[0].retry_after_millis, Some(18_000));
        assert_eq!(snapshot.peers[0].updated_ago_millis, 12_000);
        drop(lease);
        let stopped = tracker.snapshot(config.group);
        assert!(!stopped.running && !stopped.observed);
        assert_eq!(stopped.peers[0].phase, SyncPhase::Unknown);
        assert_eq!(stopped.peers[0].pending, None);
        assert_eq!(stopped.peers[0].last_issue, None);
        assert_eq!(stopped.peers[0].retry_after_millis, None);
        let _next = tracker.start().unwrap();
        assert!(tracker.snapshot(config.group).peers.is_empty());
        assert!(!tracker.snapshot(config.group).observed);
        tracker.configure(std::slice::from_ref(&config));
        // A late observation from the old owner cannot cross generations, even
        // when group/control/store/endpoint are all unchanged.
        observer.update(SyncPhase::Connecting, Some(true), None, 0, None);
        assert_eq!(
            tracker.snapshot(config.group).peers[0].phase,
            SyncPhase::Unknown
        );
    }

    #[test]
    fn old_control_and_store_observations_cannot_overwrite_current_scope() {
        let tracker = Arc::new(ProgressTracker::default());
        let _lease = tracker.start().unwrap();
        let mut config = config();
        tracker.configure(std::slice::from_ref(&config));
        let old = PeerObserver::new(Arc::clone(&tracker), &config, config.peers[0].endpoint);
        config.control.head = ControlId([9; 32]);
        tracker.configure(std::slice::from_ref(&config));
        old.update(SyncPhase::Connecting, Some(true), None, 0, None);
        assert_eq!(
            tracker.snapshot(config.group).peers[0].phase,
            SyncPhase::Unknown
        );
        let old_store = PeerObserver::new(Arc::clone(&tracker), &config, config.peers[0].endpoint);
        config.incarnation = StoreIncarnation::new([10; 24]).unwrap();
        tracker.configure(std::slice::from_ref(&config));
        old_store.update(SyncPhase::Connecting, Some(true), None, 0, None);
        assert_eq!(
            tracker.snapshot(config.group).peers[0].phase,
            SyncPhase::Unknown
        );
        tracker.forget(config.group);
        assert!(tracker.snapshot(config.group).peers.is_empty());
    }

    #[test]
    fn observation_capacity_is_bounded_and_storage_failure_is_not_idle() {
        let tracker = Arc::new(ProgressTracker::default());
        let _lease = tracker.start().unwrap();
        let base = config();
        let configs: Vec<_> = (0..MAX_GROUPS * 3)
            .map(|i| NetworkConfig {
                group: GroupId([i as u8; 32]),
                ..base.clone()
            })
            .collect();
        tracker.configure(&configs);
        assert_eq!(tracker.0.lock().unwrap().groups.len(), MAX_GROUPS);
        assert_eq!(tracker.0.lock().unwrap().peers.len(), MAX_GROUPS);
        tracker.unavailable(configs[0].group, SyncIssue::StorageFull);
        let progress = tracker.snapshot(configs[0].group);
        assert_eq!(progress.peers[0].phase, SyncPhase::StorageBlocked);
        assert_eq!(progress.peers[0].pending, None);
        assert_eq!(progress.peers[0].last_issue, Some(SyncIssue::StorageFull));
        let crowded: Vec<_> = configs
            .into_iter()
            .take(MAX_GROUPS)
            .map(|mut config| {
                config.peers = (0..20)
                    .map(|i| prns_groups_reticulum::Peer {
                        endpoint: Endpoint {
                            id: EndpointId([i + 10; 32]),
                            ..base.peers[0].endpoint
                        },
                        identity: IdentityHash::new([i + 10; 16]),
                        destination: prns_host::DestinationHash::new([i + 10; 16]),
                    })
                    .collect();
                config
            })
            .collect();
        tracker.configure(&crowded);
        assert!(tracker.0.lock().unwrap().peers.len() <= MAX_PEERS);
        let retained = MAX_PEERS / 20;
        for (i, config) in crowded.iter().enumerate() {
            assert_eq!(
                tracker.snapshot(config.group).observed,
                i >= crowded.len() - retained
            );
        }
    }
    #[tokio::test(start_paused = true)]
    async fn rotating_groups_preserve_each_others_observations_and_scope_failures() {
        let tracker = Arc::new(ProgressTracker::default());
        let _lease = tracker.start().unwrap();
        let first = config();
        let second = NetworkConfig {
            group: GroupId([20; 32]),
            ..first.clone()
        };
        tracker.configure(std::slice::from_ref(&first));
        let observer = PeerObserver::new(Arc::clone(&tracker), &first, first.peers[0].endpoint);
        observer.update(
            SyncPhase::RetryScheduled,
            Some(true),
            Some(Instant::now() + std::time::Duration::from_secs(30)),
            2,
            Some(SyncIssue::Transport),
        );
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        tracker.configure(std::slice::from_ref(&second));
        tracker.configure(&[]);
        let remembered = tracker.snapshot(first.group);
        assert!(remembered.observed);
        assert_eq!(remembered.control, Some(first.control));
        assert_eq!(remembered.peers[0].retry_after_millis, Some(25_000));
        assert_eq!(remembered.peers[0].updated_ago_millis, 5_000);
        tracker.unavailable(second.group, SyncIssue::StorageUnavailable);
        assert_eq!(tracker.snapshot(first.group), remembered);
        assert_eq!(
            tracker.snapshot(second.group).peers[0].last_issue,
            Some(SyncIssue::StorageUnavailable)
        );
        tracker.forget(second.group);
        assert_eq!(tracker.snapshot(first.group), remembered);
        assert!(!tracker.snapshot(second.group).observed);
    }

    #[test]
    fn recent_group_eviction_and_partial_rosters_are_unknown_not_delivery_failure() {
        let tracker = Arc::new(ProgressTracker::default());
        let _lease = tracker.start().unwrap();
        let base = config();
        for n in 0..=MAX_GROUPS {
            let selected = NetworkConfig {
                group: GroupId([n as u8; 32]),
                ..base.clone()
            };
            tracker.configure(std::slice::from_ref(&selected));
        }
        let evicted = tracker.snapshot(GroupId([0; 32]));
        assert!(evicted.running);
        assert!(!evicted.observed);
        assert!(evicted.control.is_none() && evicted.peers.is_empty());
        assert_eq!(tracker.0.lock().unwrap().groups.len(), MAX_GROUPS);
        let mut crowded = base;
        crowded.peers = (0..MAX_PEERS + 1)
            .map(|n| {
                let mut bytes = [0; 32];
                bytes[..8].copy_from_slice(&(n as u64).to_be_bytes());
                prns_groups_reticulum::Peer {
                    endpoint: Endpoint {
                        id: EndpointId(bytes),
                        ..crowded.peers[0].endpoint
                    },
                    ..crowded.peers[0]
                }
            })
            .collect();
        tracker.configure(std::slice::from_ref(&crowded));
        let partial = tracker.snapshot(crowded.group);
        assert!(partial.running && !partial.observed);
        assert_eq!(partial.control, Some(crowded.control));
        assert_eq!(partial.peers.len(), MAX_PEERS);
        assert!(partial
            .peers
            .iter()
            .all(|peer| peer.phase == SyncPhase::Unknown
                && peer.pending.is_none()
                && peer.last_issue.is_none()));
        assert_eq!(tracker.0.lock().unwrap().peers.len(), MAX_PEERS);
    }
}
