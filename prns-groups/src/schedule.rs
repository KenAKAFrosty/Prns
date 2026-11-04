//! Bounded, disposable scheduling evidence. No durable obligation lives here.
use crate::{runner::SyncClient, NetworkConfig};
use prns_groups_core::sync::{PeerSchedule, SyncWake};
use prns_groups_reticulum::{request_group, Network, RequestJob};
use prns_groups_service::{ControlReference, Endpoint, GroupId};
use prns_groups_store::StoreIncarnation;
use prns_host::IdentityHash;
use std::{sync::Mutex, time::Duration};
use tokio::{sync::Notify, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecoveryHint {
    pub group: GroupId,
    pub control: ControlReference,
    pub incarnation: StoreIncarnation,
    pub peer: Endpoint,
    pub identity: IdentityHash,
}

impl RecoveryHint {
    pub fn trace(self, disposition: RecoveryDisposition) {
        crate::diagnostics::recovery(&self.group.0, self.identity.as_bytes(), disposition);
    }

    pub fn current(self, config: &NetworkConfig) -> bool {
        self.group == config.group
            && self.control == config.control
            && self.incarnation == config.incarnation
            && config.readers.contains(&self.peer)
            && config
                .peers
                .iter()
                .any(|peer| peer.endpoint == self.peer && peer.identity == self.identity)
    }
}

/// Each category describes one local scheduling decision, not payload acceptance
/// or a delivery acknowledgement. Only authenticated, fully framed service
/// responses can reach the queue; authentication failures are not raw hints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryDisposition {
    ServiceFramingRejected,
    ServiceRequesterUnknown,
    ServiceScopeStale,
    ServiceScopeUnavailable,
    Offered,
    Coalesced,
    QueueFull,
    QueuePoisoned,
    ScopeUnavailable,
    ScopeStale,
    CandidateMissing,
    PeerActive,
    NotFailed,
    Credited,
    ConsumedNoPending,
    ConsumedProbeWait,
    ConsumedFreshWork,
    ConsumedAccelerated,
    ConsumedNoShortening,
}

impl RecoveryDisposition {
    pub fn name(self) -> &'static str {
        match self {
            Self::ServiceFramingRejected => "service_framing_rejected",
            Self::ServiceRequesterUnknown => "service_requester_unknown",
            Self::ServiceScopeStale => "service_scope_stale",
            Self::ServiceScopeUnavailable => "service_scope_unavailable",
            Self::Offered => "offered",
            Self::Coalesced => "coalesced",
            Self::QueueFull => "queue_full",
            Self::QueuePoisoned => "queue_poisoned",
            Self::ScopeUnavailable => "scope_unavailable",
            Self::ScopeStale => "scope_stale",
            Self::CandidateMissing => "candidate_missing",
            Self::PeerActive => "peer_active",
            Self::NotFailed => "not_failed",
            Self::Credited => "credited",
            Self::ConsumedNoPending => "consumed_no_pending",
            Self::ConsumedProbeWait => "consumed_probe_wait",
            Self::ConsumedFreshWork => "consumed_fresh_work",
            Self::ConsumedAccelerated => "consumed_accelerated",
            Self::ConsumedNoShortening => "consumed_no_shortening",
        }
    }

    pub fn consumed(pending: bool, enabled: bool, fresh: bool, accelerated: bool) -> Self {
        if !enabled {
            Self::ConsumedProbeWait
        } else if !pending {
            Self::ConsumedNoPending
        } else if fresh {
            // Fresh durable work takes precedence over recovery evidence. The
            // accompanying schedule event says whether it shortened the wait.
            Self::ConsumedFreshWork
        } else if accelerated {
            Self::ConsumedAccelerated
        } else {
            Self::ConsumedNoShortening
        }
    }
}

/// One coalesced hint per authorization scope, at most 32 scopes. Saturation or
/// cancellation loses only acceleration: ordinary durable recovery continues.
#[derive(Default)]
pub(crate) struct Recoveries {
    hints: Mutex<Vec<RecoveryHint>>,
    pub changed: Notify,
}
impl Recoveries {
    pub fn offer(&self, hint: RecoveryHint) -> RecoveryDisposition {
        let Ok(mut hints) = self.hints.lock() else {
            hint.trace(RecoveryDisposition::QueuePoisoned);
            return RecoveryDisposition::QueuePoisoned;
        };
        let disposition = if hints.contains(&hint) {
            RecoveryDisposition::Coalesced
        } else if hints.len() == 32 {
            RecoveryDisposition::QueueFull
        } else {
            hints.push(hint);
            RecoveryDisposition::Offered
        };
        // Publish the offer before notifying the consumer, so a complete trace
        // cannot show consumption preceding the queue's disposition.
        hint.trace(disposition);
        if disposition == RecoveryDisposition::Offered {
            self.changed.notify_one();
        }
        disposition
    }
    /// Leave other groups' bounded evidence pending until their fair catalog
    /// turn. Reading one working set does not make other scopes stale.
    pub fn take_group(&self, group: GroupId) -> Vec<RecoveryHint> {
        let Ok(mut hints) = self.hints.lock() else {
            return Vec::new();
        };
        let mut selected = Vec::new();
        hints.retain(|hint| {
            if hint.group == group {
                selected.push(*hint);
                false
            } else {
                true
            }
        });
        selected
    }
    pub fn take(&self) -> Vec<RecoveryHint> {
        self.hints
            .lock()
            .map(|mut hints| std::mem::take(&mut *hints))
            .unwrap_or_default()
    }
}

/// Called only after the service responded successfully. `Ok` from serving can
/// also mean an error response, so independently require identified reader
/// membership and complete framing. A fresh configuration read fences a control
/// or store change during the response. This is reachability evidence, not an
/// acknowledgement that application content was accepted.
pub(crate) async fn authenticated_recovery(
    network: &Network,
    store: &SyncClient,
    job: &RequestJob,
) -> Option<RecoveryHint> {
    let rejected = |disposition| {
        crate::diagnostics::recovery(
            &store.config.group.0,
            job.requester().as_bytes(),
            disposition,
        );
    };
    if request_group(job.data()).ok() != Some(store.config.group) {
        rejected(RecoveryDisposition::ServiceFramingRejected);
        return None;
    }
    let Some(peer) = network.requester(job) else {
        rejected(RecoveryDisposition::ServiceRequesterUnknown);
        return None;
    };
    let hint = RecoveryHint {
        group: store.config.group,
        control: store.config.control,
        incarnation: store.config.incarnation,
        peer,
        identity: job.requester(),
    };
    if !hint.current(&store.config) {
        hint.trace(RecoveryDisposition::ServiceScopeStale);
        return None;
    }
    let Some(config) = tokio::time::timeout(Duration::from_secs(10), store.client.network_config())
        .await
        .ok()
        .and_then(Result::ok)
    else {
        hint.trace(RecoveryDisposition::ServiceScopeUnavailable);
        return None;
    };
    if !hint.current(&config) {
        hint.trace(RecoveryDisposition::ServiceScopeStale);
        return None;
    }
    Some(hint)
}

pub(crate) fn expedite(
    state: &PeerSchedule,
    due: &mut Instant,
    floor: Instant,
    evidence: SyncWake,
    now: Instant,
) -> bool {
    let remaining = millis(due.saturating_duration_since(now));
    let minimum = millis(floor.saturating_duration_since(now));
    let delay = state.accelerated_delay_millis(remaining, minimum, evidence);
    // Use the exact floor, not now + a rounded duration: repeated evidence
    // must not shave rounding residue into several apparent acceleration credits.
    let proposed = if delay < remaining {
        (*due).min(now.max(floor))
    } else {
        *due
    };
    let changed = proposed < *due;
    *due = proposed;
    changed
}

pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos().div_ceil(1_000_000)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use prns_groups_core::{AuthorizationEra, ControlId, EndpointId, EndpointIncarnation};

    fn hint() -> RecoveryHint {
        RecoveryHint {
            group: GroupId([1; 32]),
            control: ControlReference {
                head: ControlId([2; 32]),
                era: AuthorizationEra(ControlId([2; 32])),
            },
            incarnation: StoreIncarnation::new([3; 24]).unwrap(),
            peer: Endpoint {
                id: EndpointId([4; 32]),
                incarnation: EndpointIncarnation::new(1).unwrap(),
            },
            identity: IdentityHash::new([5; 16]),
        }
    }

    #[test]
    fn recovery_requires_exact_current_membership_control_store_and_identity() {
        let hint = hint();
        let config = NetworkConfig {
            control: hint.control,
            group: hint.group,
            local: hint.peer,
            readers: vec![hint.peer],
            incarnation: hint.incarnation,
            local_reticulum_identity: IdentityHash::new([6; 16]),
            peers: vec![prns_groups_reticulum::Peer {
                endpoint: hint.peer,
                identity: hint.identity,
                destination: prns_host::DestinationHash::new([7; 16]),
            }],
        };
        assert!(hint.current(&config));
        let mut changed = config.clone();
        changed.control.head = ControlId([9; 32]);
        assert!(!hint.current(&changed));
        changed = config.clone();
        changed.incarnation = StoreIncarnation::new([8; 24]).unwrap();
        assert!(!hint.current(&changed));
        changed = config.clone();
        changed.readers.clear();
        assert!(!hint.current(&changed));
        changed = config.clone();
        changed.peers[0].identity = IdentityHash::new([9; 16]);
        assert!(!hint.current(&changed));
        changed = config;
        changed.group = GroupId([9; 32]);
        assert!(!hint.current(&changed));
    }

    #[test]
    fn selecting_one_group_preserves_other_groups_recovery_hints() {
        let queue = Recoveries::default();
        let first = hint();
        let second = RecoveryHint {
            group: GroupId([9; 32]),
            ..first
        };
        queue.offer(first);
        queue.offer(second);
        assert_eq!(queue.take_group(first.group), vec![first]);
        assert!(queue.take_group(first.group).is_empty());
        assert_eq!(queue.take_group(second.group), vec![second]);
    }

    #[test]
    fn duplicate_recovery_notifications_coalesce_and_capacity_is_bounded() {
        let queue = Recoveries::default();
        assert_eq!(queue.offer(hint()), RecoveryDisposition::Offered);
        for _ in 0..100 {
            assert_eq!(queue.offer(hint()), RecoveryDisposition::Coalesced);
        }
        assert_eq!(queue.take(), vec![hint()]);
        for byte in 0..100 {
            let disposition = queue.offer(RecoveryHint {
                group: GroupId([byte; 32]),
                ..hint()
            });
            assert_eq!(
                disposition,
                if byte < 32 {
                    RecoveryDisposition::Offered
                } else {
                    RecoveryDisposition::QueueFull
                }
            );
        }
        assert_eq!(queue.take().len(), 32);
        assert!(queue.take().is_empty());
    }

    #[test]
    fn consumed_hints_do_not_claim_deadline_shortening_without_a_real_shortening() {
        use RecoveryDisposition as D;
        assert_eq!(D::consumed(false, true, false, false), D::ConsumedNoPending);
        assert_eq!(
            D::consumed(false, false, false, false),
            D::ConsumedProbeWait
        );
        assert_eq!(D::consumed(true, true, true, true), D::ConsumedFreshWork);
        assert_eq!(D::consumed(true, true, true, false), D::ConsumedFreshWork);
        assert_eq!(
            D::consumed(true, true, false, false),
            D::ConsumedNoShortening
        );
        assert_eq!(D::consumed(true, true, false, true), D::ConsumedAccelerated);
    }

    #[test]
    fn repeated_evidence_has_only_one_shortening_until_an_attempt_advances_floor() {
        let now = Instant::now();
        let mut state = PeerSchedule::default();
        let tuning = prns_groups_core::sync::SyncPolicy::default().tuning(Default::default());
        for _ in 0..10 {
            state.delay_millis(tuning, prns_groups_core::sync::SyncActivity::Failed, 0);
        }
        let mut due = now + Duration::from_secs(300);
        let floor = now + Duration::from_secs(15);
        assert!(expedite(
            &state,
            &mut due,
            floor,
            SyncWake::PeerRecovered,
            now + Duration::from_micros(100)
        ));
        for millis in 1..15_000 {
            assert!(!expedite(
                &state,
                &mut due,
                floor,
                SyncWake::PeerRecovered,
                now + Duration::from_millis(millis)
            ));
        }
        assert_eq!(due, floor);
        assert_eq!(state.failures(), 10);
    }
}
