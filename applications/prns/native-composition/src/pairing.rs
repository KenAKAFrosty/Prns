use std::sync::atomic::{AtomicBool, Ordering};

use personal_rns::prelude::{Message, PrnsEvent, RemoteControlControllerPairingConfirmation};
use personal_rns::remote_control::{
    RemoteControlControllerPairingAborted, RemoteControlPairingAttemptId,
    RemoteControlPairingEndpoint, RemoteControlRequestSet,
};
use personal_rns::units::InstantMillis;
use prns_core::engine::{PersistenceFlushCause, PersistenceFlushTarget};
use tokio::sync::mpsc;

use crate::contract::{
    DevelopmentNodeFailure, DevelopmentNodeFailureStage, RemoteControlPairingCandidate,
    RemoteControlPairingState, RemoteControlRequestKind, U64String,
};
use crate::snapshot::SnapshotStore;

pub const EVENT_LANE_CAPACITY: usize = 16;
pub const PAIRING_CANDIDATE_CAPACITY: usize = 8;
const MAX_PAIRING_DISPLAY_NAME_CHARS: usize = 64;

#[derive(Clone)]
pub struct PairingCandidateControl {
    pub candidate_id: String,
    pub endpoint: RemoteControlPairingEndpoint,
    pub display_name: Option<String>,
    pub observed_at: InstantMillis,
    pub expires_at: InstantMillis,
}

#[derive(Default)]
pub struct PairingControls {
    pub candidates: Vec<PairingCandidateControl>,
    pub(crate) expired_candidate_ids: Vec<String>,
    pub confirmation: Option<RemoteControlControllerPairingConfirmation>,
    pub initiated_candidate_id: Option<String>,
    pub active_attempt_id: Option<String>,
    pub attempt_phase: Option<PairingAttemptPhase>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingAttemptPhase {
    AwaitingConfirmation,
    ConfirmationReady,
    DecisionSubmitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingCandidateResolution {
    Retained {
        endpoint: RemoteControlPairingEndpoint,
        expires_at: InstantMillis,
    },
    Missing,
    Expired,
}

pub enum OwnedNodeEvent {
    PersistenceRestored,
    PersistenceFlushed {
        cause: PersistenceFlushCause,
    },
    PairingAvailable {
        endpoint: RemoteControlPairingEndpoint,
        observed_at: InstantMillis,
        expires_at: InstantMillis,
        public_app_data: Vec<u8>,
    },
    ControllerConfirmation(RemoteControlControllerPairingConfirmation),
    ControllerAuthorizationPersisted(RemoteControlPairingAttemptId),
    ControllerAuthorizationPersistenceFailed(RemoteControlPairingAttemptId),
    ControllerExpired {
        attempt_id: Option<RemoteControlPairingAttemptId>,
    },
    ControllerLinkClosed {
        attempt_id: Option<RemoteControlPairingAttemptId>,
    },
    PersistenceFailed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppliedNodeEvent {
    None,
    PersistenceRestored,
    TargetInventoryChanged,
}

pub fn capture_event(event: PrnsEvent<'_>) -> Option<OwnedNodeEvent> {
    match event {
        PrnsEvent::Diagnostic(personal_rns::Diagnostic::PersistenceRestored { .. }) => {
            Some(OwnedNodeEvent::PersistenceRestored)
        }
        PrnsEvent::Diagnostic(personal_rns::Diagnostic::PersistenceFlushed { cause, .. }) => {
            Some(OwnedNodeEvent::PersistenceFlushed { cause })
        }
        PrnsEvent::Diagnostic(personal_rns::Diagnostic::PersistenceFlushFailed {
            cause,
            target,
        }) => Some(OwnedNodeEvent::PersistenceFailed { cause, target }),
        PrnsEvent::Message(Message::RemoteControlPairingAvailable(observation)) => {
            Some(OwnedNodeEvent::PairingAvailable {
                endpoint: observation.endpoint(),
                observed_at: observation.observed_at(),
                expires_at: observation.expires_at(),
                public_app_data: observation.public_app_data().as_bytes().to_vec(),
            })
        }
        PrnsEvent::Message(Message::RemoteControlControllerPairingConfirmationRequired(
            confirmation,
        )) => Some(OwnedNodeEvent::ControllerConfirmation(confirmation)),
        PrnsEvent::Message(Message::RemoteControlControllerPairingAuthorizationPersisted {
            attempt_id,
        }) => Some(OwnedNodeEvent::ControllerAuthorizationPersisted(attempt_id)),
        PrnsEvent::Message(
            Message::RemoteControlControllerPairingAuthorizationPersistenceFailed { attempt_id },
        ) => Some(OwnedNodeEvent::ControllerAuthorizationPersistenceFailed(
            attempt_id,
        )),
        PrnsEvent::Message(Message::RemoteControlControllerPairingExpired { aborted }) => {
            Some(OwnedNodeEvent::ControllerExpired {
                attempt_id: aborted_attempt_id(aborted),
            })
        }
        PrnsEvent::Message(Message::RemoteControlControllerPairingLinkClosed { aborted }) => {
            Some(OwnedNodeEvent::ControllerLinkClosed {
                attempt_id: aborted_attempt_id(aborted),
            })
        }
        PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => None,
    }
}

pub fn send_event(
    sender: &mpsc::Sender<OwnedNodeEvent>,
    overflowed: &AtomicBool,
    event: PrnsEvent<'_>,
) {
    let Some(owned) = capture_event(event) else {
        return;
    };
    if let Err(error) = sender.try_send(owned) {
        if matches!(error, mpsc::error::TrySendError::Full(_)) {
            overflowed.store(true, Ordering::Release);
        }
    }
}

pub fn apply_event(
    event: OwnedNodeEvent,
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    now: InstantMillis,
) -> AppliedNodeEvent {
    match event {
        OwnedNodeEvent::PersistenceRestored => return AppliedNodeEvent::PersistenceRestored,
        OwnedNodeEvent::PersistenceFlushed { .. } => {}
        OwnedNodeEvent::PairingAvailable {
            endpoint,
            observed_at,
            expires_at,
            public_app_data,
        } => {
            let candidate_id = bytes_hex(endpoint.destination_hash().as_bytes());
            controls.upsert_candidate(
                PairingCandidateControl {
                    candidate_id,
                    endpoint,
                    display_name: safe_display_name(&public_app_data),
                    observed_at,
                    expires_at,
                },
                now,
            );
            controls.publish_candidates(snapshots, now);
        }
        OwnedNodeEvent::ControllerConfirmation(confirmation) => {
            let observed_attempt = confirmation.confirmation().attempt_id();
            let observed_attempt_id = attempt_id_string(observed_attempt);
            if !controls.matches_confirmation_id(&observed_attempt_id) {
                return AppliedNodeEvent::None;
            }
            let confirmation_code = confirmation.confirmation().confirmation_code().to_string();
            let target_identity_fingerprint = confirmation
                .confirmation()
                .target()
                .identity_hash()
                .as_bytes()
                .to_vec();
            let permissions = request_kinds(
                confirmation
                    .confirmation()
                    .permissions()
                    .permitted_requests(),
            );
            snapshots.update(|snapshot| {
                snapshot.pairing = RemoteControlPairingState::ConfirmationRequired {
                    attempt_id: observed_attempt_id,
                    confirmation_code,
                    target_identity_fingerprint,
                    permissions,
                };
            });
            controls.retain_confirmation(confirmation);
        }
        OwnedNodeEvent::ControllerAuthorizationPersisted(attempt_id) => {
            let observed_attempt_id = attempt_id_string(attempt_id);
            if controls.active_attempt_id.as_deref() != Some(observed_attempt_id.as_str()) {
                return AppliedNodeEvent::None;
            }
            let selected_candidate_id = controls.clear_attempt();
            snapshots.update(|snapshot| {
                remove_selected_candidate(snapshot, selected_candidate_id.as_deref());
                snapshot.pairing = RemoteControlPairingState::Paired {
                    attempt_id: observed_attempt_id,
                };
                snapshot.active_operation = None;
            });
            return AppliedNodeEvent::TargetInventoryChanged;
        }
        OwnedNodeEvent::ControllerAuthorizationPersistenceFailed(attempt_id) => {
            let observed_attempt_id = attempt_id_string(attempt_id);
            if controls.active_attempt_id.as_deref() != Some(observed_attempt_id.as_str()) {
                return AppliedNodeEvent::None;
            }
            let selected_candidate_id = controls.clear_attempt();
            snapshots.update(|snapshot| {
                remove_selected_candidate(snapshot, selected_candidate_id.as_deref());
                snapshot.pairing = RemoteControlPairingState::Failed {
                    stage: crate::contract::RemoteControlPairingFailureStage::Persistence,
                    detail: "The paired target authorization could not be persisted.".to_owned(),
                };
                snapshot.active_operation = None;
            });
        }
        OwnedNodeEvent::ControllerExpired { attempt_id } => {
            if !controls.matches_terminal(attempt_id) {
                return AppliedNodeEvent::None;
            }
            let selected_candidate_id = controls.clear_attempt();
            snapshots.update(|snapshot| {
                remove_selected_candidate(snapshot, selected_candidate_id.as_deref());
                snapshot.pairing = RemoteControlPairingState::Expired {
                    detail: "The retained upstream pairing attempt expired.".to_owned(),
                };
                snapshot.active_operation = None;
            });
        }
        OwnedNodeEvent::ControllerLinkClosed { attempt_id } => {
            if !controls.matches_terminal(attempt_id) {
                return AppliedNodeEvent::None;
            }
            let selected_candidate_id = controls.clear_attempt();
            snapshots.update(|snapshot| {
                remove_selected_candidate(snapshot, selected_candidate_id.as_deref());
                snapshot.pairing = RemoteControlPairingState::Failed {
                    stage: crate::contract::RemoteControlPairingFailureStage::Link,
                    detail: "The upstream pairing Link closed before authorization persisted."
                        .to_owned(),
                };
                snapshot.active_operation = None;
            });
        }
        OwnedNodeEvent::PersistenceFailed { .. } => snapshots.fail(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::PersistenceRestore,
            detail: "Prns reported a persistence flush failure.".to_owned(),
        }),
    }
    AppliedNodeEvent::None
}

pub fn apply_persistence_event(
    event: &OwnedNodeEvent,
    persistence: &mut prns_host::PersistenceSnapshot,
) {
    match event {
        OwnedNodeEvent::PersistenceRestored => {
            persistence.restored = true;
            persistence.last_failure_detail = None;
        }
        OwnedNodeEvent::PersistenceFlushed { cause } => {
            persistence.last_flush_cause = Some(host_flush_cause(*cause));
            persistence.last_failure_detail = None;
        }
        OwnedNodeEvent::PersistenceFailed { cause, target } => {
            persistence.last_failure_detail = Some(format!("{cause:?}:{target:?}"));
        }
        OwnedNodeEvent::PairingAvailable { .. }
        | OwnedNodeEvent::ControllerConfirmation(_)
        | OwnedNodeEvent::ControllerAuthorizationPersisted(_)
        | OwnedNodeEvent::ControllerAuthorizationPersistenceFailed(_)
        | OwnedNodeEvent::ControllerExpired { .. }
        | OwnedNodeEvent::ControllerLinkClosed { .. } => {}
    }
}

const fn host_flush_cause(cause: PersistenceFlushCause) -> prns_host::PersistenceFlushCause {
    match cause {
        PersistenceFlushCause::Startup => prns_host::PersistenceFlushCause::Startup,
        PersistenceFlushCause::Interval => prns_host::PersistenceFlushCause::Interval,
        PersistenceFlushCause::RouteChange => prns_host::PersistenceFlushCause::RouteChange,
        PersistenceFlushCause::RatchetRotation => prns_host::PersistenceFlushCause::RatchetRotation,
        PersistenceFlushCause::Shutdown => prns_host::PersistenceFlushCause::Shutdown,
    }
}

impl PairingControls {
    fn upsert_candidate(&mut self, candidate: PairingCandidateControl, now: InstantMillis) -> bool {
        self.prune_expired(now);
        if candidate.expires_at <= now {
            self.remember_expired(candidate.candidate_id);
            return false;
        }
        self.expired_candidate_ids
            .retain(|candidate_id| candidate_id != &candidate.candidate_id);

        if let Some(retained) = self
            .candidates
            .iter_mut()
            .find(|retained| retained.candidate_id == candidate.candidate_id)
        {
            let is_fresher = candidate.observed_at > retained.observed_at
                || (candidate.observed_at == retained.observed_at
                    && candidate.expires_at > retained.expires_at);
            if is_fresher {
                *retained = candidate;
                return true;
            }
            return false;
        }

        let candidate_id = candidate.candidate_id.clone();
        self.candidates.push(candidate);
        self.candidates.sort_unstable_by(candidate_retention_order);
        self.candidates.truncate(PAIRING_CANDIDATE_CAPACITY);
        self.candidates
            .iter()
            .any(|candidate| candidate.candidate_id == candidate_id)
    }

    pub fn resolve_candidate(
        &mut self,
        candidate_id: &str,
        now: InstantMillis,
    ) -> PairingCandidateResolution {
        let Some(position) = self
            .candidates
            .iter()
            .position(|candidate| candidate.candidate_id == candidate_id)
        else {
            return if self
                .expired_candidate_ids
                .iter()
                .any(|expired| expired == candidate_id)
            {
                PairingCandidateResolution::Expired
            } else {
                PairingCandidateResolution::Missing
            };
        };
        if self.candidates[position].expires_at <= now {
            let expired = self.candidates.remove(position);
            self.remember_expired(expired.candidate_id);
            return PairingCandidateResolution::Expired;
        }
        let candidate = &self.candidates[position];
        PairingCandidateResolution::Retained {
            endpoint: candidate.endpoint,
            expires_at: candidate.expires_at,
        }
    }

    fn publish_candidates(&self, snapshots: &SnapshotStore, now: InstantMillis) {
        let candidates = self.projected_candidates(now);
        let current = snapshots.read();
        if current.pairing_candidates == candidates {
            return;
        }
        snapshots.update(|snapshot| {
            snapshot.pairing_candidates = candidates;
        });
    }

    fn projected_candidates(&self, now: InstantMillis) -> Vec<RemoteControlPairingCandidate> {
        let mut retained = self.candidates.iter().collect::<Vec<_>>();
        retained.sort_unstable_by(|left, right| {
            right
                .observed_at
                .0
                .cmp(&left.observed_at.0)
                .then_with(|| right.expires_at.0.cmp(&left.expires_at.0))
                .then_with(|| left.candidate_id.cmp(&right.candidate_id))
        });
        retained
            .into_iter()
            .map(|candidate| RemoteControlPairingCandidate {
                candidate_id: candidate.candidate_id.clone(),
                display_name: candidate.display_name.clone(),
                observed_at_millis: U64String::from(candidate.observed_at.0),
                expires_at_millis: U64String::from(candidate.expires_at.0),
                expires_in_millis: U64String::from(candidate.expires_at.0.saturating_sub(now.0)),
            })
            .collect()
    }

    fn prune_expired(&mut self, now: InstantMillis) {
        let mut retained = Vec::with_capacity(self.candidates.len());
        for candidate in std::mem::take(&mut self.candidates) {
            if candidate.expires_at <= now {
                self.remember_expired(candidate.candidate_id);
            } else {
                retained.push(candidate);
            }
        }
        self.candidates = retained;
    }

    fn remember_expired(&mut self, candidate_id: String) {
        self.expired_candidate_ids
            .retain(|retained| retained != &candidate_id);
        self.expired_candidate_ids.push(candidate_id);
        if self.expired_candidate_ids.len() > PAIRING_CANDIDATE_CAPACITY {
            self.expired_candidate_ids.remove(0);
        }
    }

    pub fn retain_initiation(&mut self, candidate_id: String) {
        self.initiated_candidate_id = Some(candidate_id);
        self.confirmation = None;
        self.active_attempt_id = None;
        self.attempt_phase = None;
    }

    pub fn retain_attempt(&mut self, attempt_id: String) {
        self.active_attempt_id = Some(attempt_id);
        self.attempt_phase = Some(PairingAttemptPhase::AwaitingConfirmation);
    }

    pub fn pairing_in_progress(&self) -> bool {
        self.initiated_candidate_id.is_some() || self.active_attempt_id.is_some()
    }

    pub fn take_confirmation_for_decision(
        &mut self,
        attempt_id: &str,
    ) -> Option<RemoteControlControllerPairingConfirmation> {
        if self.attempt_phase != Some(PairingAttemptPhase::ConfirmationReady) {
            return None;
        }
        let confirmation = self.confirmation.as_ref()?;
        if attempt_id_string(confirmation.confirmation().attempt_id()) != attempt_id {
            return None;
        }
        self.attempt_phase = Some(PairingAttemptPhase::DecisionSubmitted);
        self.confirmation.take()
    }

    pub fn restore_confirmation(
        &mut self,
        confirmation: RemoteControlControllerPairingConfirmation,
    ) {
        self.confirmation = Some(confirmation);
        self.attempt_phase = Some(PairingAttemptPhase::ConfirmationReady);
    }

    fn retain_confirmation(&mut self, confirmation: RemoteControlControllerPairingConfirmation) {
        self.confirmation = Some(confirmation);
        self.attempt_phase = Some(PairingAttemptPhase::ConfirmationReady);
    }

    pub fn cancel_initiation(&mut self) {
        self.confirmation = None;
        self.initiated_candidate_id = None;
        self.active_attempt_id = None;
        self.attempt_phase = None;
    }

    pub fn clear_attempt(&mut self) -> Option<String> {
        let selected_candidate_id = self.initiated_candidate_id.take();
        if let Some(candidate_id) = selected_candidate_id.as_deref() {
            self.candidates
                .retain(|candidate| candidate.candidate_id != candidate_id);
        }
        self.confirmation = None;
        self.active_attempt_id = None;
        self.attempt_phase = None;
        selected_candidate_id
    }

    fn matches_terminal(&self, attempt_id: Option<RemoteControlPairingAttemptId>) -> bool {
        let observed = attempt_id.map(attempt_id_string);
        self.matches_terminal_id(observed.as_deref())
    }

    fn matches_confirmation_id(&self, observed_attempt_id: &str) -> bool {
        self.active_attempt_id.as_deref() == Some(observed_attempt_id)
            && self.attempt_phase == Some(PairingAttemptPhase::AwaitingConfirmation)
            && self.confirmation.is_none()
    }

    fn matches_terminal_id(&self, attempt_id: Option<&str>) -> bool {
        match (self.active_attempt_id.as_deref(), attempt_id) {
            (Some(active), Some(observed)) => active == observed,
            (Some(_), None) => false,
            (None, None) => self.initiated_candidate_id.is_some(),
            (None, Some(_)) => false,
        }
    }
}

pub fn expire_candidates(
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    now: InstantMillis,
) {
    let retained_count = controls.candidates.len();
    if retained_count == 0 {
        return;
    }
    controls.prune_expired(now);
    controls.publish_candidates(snapshots, now);
}

pub fn publish_candidate_resolution(
    controls: &PairingControls,
    snapshots: &SnapshotStore,
    now: InstantMillis,
    resolution: PairingCandidateResolution,
) {
    controls.publish_candidates(snapshots, now);
    if resolution == PairingCandidateResolution::Expired {
        snapshots.update(|snapshot| {
            if snapshot.active_operation.is_none() {
                snapshot.pairing = RemoteControlPairingState::Expired {
                    detail: "The selected pairing session expired.".to_owned(),
                };
                snapshot.active_operation = None;
            }
        });
    }
}

fn candidate_retention_order(
    left: &PairingCandidateControl,
    right: &PairingCandidateControl,
) -> std::cmp::Ordering {
    right
        .expires_at
        .0
        .cmp(&left.expires_at.0)
        .then_with(|| right.observed_at.0.cmp(&left.observed_at.0))
        .then_with(|| left.candidate_id.cmp(&right.candidate_id))
}

fn safe_display_name(public_app_data: &[u8]) -> Option<String> {
    let decoded = std::str::from_utf8(public_app_data).ok()?.trim();
    if decoded.is_empty()
        || decoded.chars().any(char::is_control)
        || decoded.chars().count() > MAX_PAIRING_DISPLAY_NAME_CHARS
    {
        return None;
    }
    Some(decoded.to_owned())
}

pub(crate) fn remove_selected_candidate(
    snapshot: &mut crate::contract::DevelopmentNodeSnapshot,
    candidate_id: Option<&str>,
) {
    if let Some(candidate_id) = candidate_id {
        snapshot
            .pairing_candidates
            .retain(|candidate| candidate.candidate_id != candidate_id);
    }
}

fn aborted_attempt_id(
    aborted: RemoteControlControllerPairingAborted,
) -> Option<RemoteControlPairingAttemptId> {
    match aborted {
        RemoteControlControllerPairingAborted::AwaitingOffer { .. } => None,
        RemoteControlControllerPairingAborted::AwaitingApproval { attempt_id, .. }
        | RemoteControlControllerPairingAborted::AwaitingCompletion { attempt_id, .. } => {
            Some(attempt_id)
        }
    }
}

pub fn apply_overflow_failure(controls: &mut PairingControls, snapshots: &SnapshotStore) {
    controls.candidates.clear();
    controls.expired_candidate_ids.clear();
    controls.cancel_initiation();
    snapshots.update(|snapshot| {
        snapshot.pairing_candidates.clear();
        snapshot.pairing = RemoteControlPairingState::Failed {
            stage: crate::contract::RemoteControlPairingFailureStage::Node,
            detail: "The bounded native event lane overflowed; pairing state is no longer authoritative."
                .to_owned(),
        };
        snapshot.active_operation = None;
    });
}

#[must_use]
pub fn attempt_id_string(attempt_id: RemoteControlPairingAttemptId) -> String {
    bytes_hex(attempt_id.transcript().as_bytes())
}

#[must_use]
pub fn bytes_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[must_use]
pub fn request_kinds(requests: &RemoteControlRequestSet) -> Vec<RemoteControlRequestKind> {
    requests
        .iter()
        .map(|kind| match kind {
            personal_rns::remote_control::RemoteControlRequestKind::Describe => {
                RemoteControlRequestKind::Describe
            }
            personal_rns::remote_control::RemoteControlRequestKind::AnnounceSelf => {
                RemoteControlRequestKind::AnnounceSelf
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_and_endpoint_ids_are_lowercase_exact_hex() {
        assert_eq!(bytes_hex(&[0x00, 0xA2, 0xFF]), "00a2ff");
    }

    #[test]
    fn request_projection_is_closed_and_ordered() {
        assert_eq!(
            request_kinds(&RemoteControlRequestSet::all()),
            vec![
                RemoteControlRequestKind::Describe,
                RemoteControlRequestKind::AnnounceSelf
            ]
        );
    }

    #[test]
    fn persistence_projection_tracks_restore_flush_and_failure_before_filtering() {
        let mut persistence = prns_host::PersistenceSnapshot::persistent();
        apply_persistence_event(&OwnedNodeEvent::PersistenceRestored, &mut persistence);
        assert!(persistence.restored);

        apply_persistence_event(
            &OwnedNodeEvent::PersistenceFlushed {
                cause: PersistenceFlushCause::Startup,
            },
            &mut persistence,
        );
        assert_eq!(
            persistence.last_flush_cause,
            Some(prns_host::PersistenceFlushCause::Startup)
        );
        assert!(persistence.last_failure_detail.is_none());

        apply_persistence_event(
            &OwnedNodeEvent::PersistenceFailed {
                cause: PersistenceFlushCause::Shutdown,
                target: PersistenceFlushTarget::RoutingState,
            },
            &mut persistence,
        );
        assert_eq!(
            persistence.last_failure_detail.as_deref(),
            Some("Shutdown:RoutingState")
        );
    }

    #[test]
    fn terminal_events_only_match_the_retained_generation() {
        let mut controls = PairingControls {
            initiated_candidate_id: Some("candidate".to_owned()),
            ..PairingControls::default()
        };
        assert!(controls.matches_terminal_id(None));
        assert!(!controls.matches_terminal_id(Some("older")));

        controls.active_attempt_id = Some("current".to_owned());
        assert!(!controls.matches_terminal_id(None));
        assert!(!controls.matches_terminal_id(Some("older")));
        assert!(controls.matches_terminal_id(Some("current")));
    }

    #[test]
    fn event_lane_overflow_invalidates_the_attempt_and_ignores_late_terminal_events() {
        let attempt = pairing_attempt_id(0x31);
        let attempt_id = attempt_id_string(attempt);
        let mut controls = PairingControls::default();
        controls.upsert_candidate(candidate(0x41, 1, 100, b"Candidate"), InstantMillis(1));
        controls.retain_initiation("candidate".to_owned());
        controls.retain_attempt(attempt_id);
        let snapshots = SnapshotStore::new();

        apply_overflow_failure(&mut controls, &snapshots);

        assert!(controls.candidates.is_empty());
        assert!(controls.expired_candidate_ids.is_empty());
        assert!(!controls.pairing_in_progress());
        assert!(controls.confirmation.is_none());
        assert!(controls.attempt_phase.is_none());
        assert!(matches!(
            snapshots.read().pairing,
            RemoteControlPairingState::Failed {
                stage: crate::contract::RemoteControlPairingFailureStage::Node,
                ..
            }
        ));

        assert_eq!(
            apply_event(
                OwnedNodeEvent::ControllerExpired {
                    attempt_id: Some(attempt),
                },
                &mut controls,
                &snapshots,
                InstantMillis(2),
            ),
            AppliedNodeEvent::None
        );
        assert!(matches!(
            snapshots.read().pairing,
            RemoteControlPairingState::Failed {
                stage: crate::contract::RemoteControlPairingFailureStage::Node,
                ..
            }
        ));
    }

    #[test]
    fn confirmations_only_match_the_exact_active_attempt() {
        let mut controls = PairingControls::default();
        assert!(!controls.matches_confirmation_id("current"));

        controls.initiated_candidate_id = Some("candidate".to_owned());
        assert!(!controls.matches_confirmation_id("current"));

        controls.retain_attempt("current".to_owned());
        assert!(!controls.matches_confirmation_id("stale"));
        assert!(controls.matches_confirmation_id("current"));

        controls.attempt_phase = Some(PairingAttemptPhase::DecisionSubmitted);
        assert!(!controls.matches_confirmation_id("current"));
    }

    #[test]
    fn controller_persistence_failure_capture_only_changes_the_exact_active_attempt() {
        let active_attempt = pairing_attempt_id(0x31);
        let stale_attempt = pairing_attempt_id(0x41);
        let active_attempt_id = attempt_id_string(active_attempt);
        assert_ne!(attempt_id_string(stale_attempt), active_attempt_id);

        let mut controls = PairingControls {
            active_attempt_id: Some(active_attempt_id.clone()),
            ..PairingControls::default()
        };
        let snapshots = SnapshotStore::new();
        snapshots.set_runtime(crate::contract::DevelopmentNodeRuntime::Running);
        snapshots.update(|snapshot| {
            snapshot.pairing = RemoteControlPairingState::Persisting {
                attempt_id: active_attempt_id.clone(),
            };
            snapshot.active_operation = Some(crate::contract::DevelopmentNodeOperation {
                kind: crate::contract::DevelopmentNodeOperationKind::Pairing,
                started_at_millis: U64String::from(7),
            });
        });

        let stale = capture_event(PrnsEvent::Message(
            Message::RemoteControlControllerPairingAuthorizationPersistenceFailed {
                attempt_id: stale_attempt,
            },
        ))
        .expect("typed persistence failure is captured");
        assert_eq!(
            apply_event(stale, &mut controls, &snapshots, InstantMillis(1)),
            AppliedNodeEvent::None
        );
        assert_eq!(
            controls.active_attempt_id.as_deref(),
            Some(active_attempt_id.as_str())
        );
        let after_stale = snapshots.read();
        assert!(matches!(
            after_stale.pairing,
            RemoteControlPairingState::Persisting { ref attempt_id }
                if attempt_id == &active_attempt_id
        ));
        assert!(after_stale.active_operation.is_some());

        let active = capture_event(PrnsEvent::Message(
            Message::RemoteControlControllerPairingAuthorizationPersistenceFailed {
                attempt_id: active_attempt,
            },
        ))
        .expect("typed persistence failure is captured");
        assert_eq!(
            apply_event(active, &mut controls, &snapshots, InstantMillis(1)),
            AppliedNodeEvent::None
        );
        assert!(controls.active_attempt_id.is_none());
        let projected = snapshots.read();
        assert_eq!(
            projected.runtime,
            crate::contract::DevelopmentNodeRuntime::Running
        );
        assert!(matches!(
            projected.pairing,
            RemoteControlPairingState::Failed {
                stage: crate::contract::RemoteControlPairingFailureStage::Persistence,
                ..
            }
        ));
        assert!(projected.active_operation.is_none());
    }

    #[test]
    fn candidates_are_bounded_projected_and_expire_individually() {
        let mut controls = PairingControls::default();
        let snapshots = SnapshotStore::new();

        apply_candidate(&mut controls, &snapshots, 0x41, 1, 5, b" First ", 1);
        apply_candidate(&mut controls, &snapshots, 0x42, 2, 10, b"Second", 2);
        let projected = snapshots.read().pairing_candidates;
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].display_name.as_deref(), Some("Second"));
        assert_eq!(projected[0].expires_in_millis, U64String::from(8));
        assert_eq!(projected[1].display_name.as_deref(), Some("First"));

        expire_candidates(&mut controls, &snapshots, InstantMillis(5));
        assert_eq!(controls.candidates.len(), 1);
        let retained = snapshots.read();
        assert_eq!(retained.pairing_candidates.len(), 1);
        assert!(matches!(
            retained.pairing,
            RemoteControlPairingState::Searching
        ));
        assert_eq!(
            retained.pairing_candidates[0].expires_in_millis,
            U64String::from(5)
        );

        expire_candidates(&mut controls, &snapshots, InstantMillis(10));
        assert!(controls.candidates.is_empty());
        assert!(snapshots.read().pairing_candidates.is_empty());
        assert!(matches!(
            snapshots.read().pairing,
            RemoteControlPairingState::Searching
        ));
    }

    #[test]
    fn candidate_refresh_replaces_only_a_fresher_observation() {
        let mut controls = PairingControls::default();
        let snapshots = SnapshotStore::new();
        apply_candidate(&mut controls, &snapshots, 0x42, 10, 100, b"Original", 10);
        apply_candidate(&mut controls, &snapshots, 0x42, 9, 200, b"Stale", 10);
        let stale_ignored = snapshots.read().pairing_candidates;
        assert_eq!(stale_ignored[0].display_name.as_deref(), Some("Original"));
        assert_eq!(stale_ignored[0].observed_at_millis, U64String::from(10));
        assert_eq!(stale_ignored[0].expires_at_millis, U64String::from(100));

        apply_candidate(&mut controls, &snapshots, 0x42, 11, 120, b" Refreshed ", 11);
        let refreshed = snapshots.read().pairing_candidates;
        assert_eq!(refreshed.len(), 1);
        assert_eq!(refreshed[0].display_name.as_deref(), Some("Refreshed"));
        assert_eq!(refreshed[0].observed_at_millis, U64String::from(11));
        assert_eq!(refreshed[0].expires_at_millis, U64String::from(120));
    }

    #[test]
    fn candidate_display_name_accepts_only_trimmed_control_free_utf8() {
        assert_eq!(
            safe_display_name(b"  Trail node  ").as_deref(),
            Some("Trail node")
        );
        assert_eq!(safe_display_name(b" \t\n "), None);
        assert_eq!(safe_display_name(b"line\nbreak"), None);
        assert_eq!(safe_display_name(&[0xff]), None);
        assert_eq!(safe_display_name(&[b'a'; 65]), None);
    }

    #[test]
    fn worse_ninth_candidate_is_not_retained() {
        let mut controls = PairingControls::default();
        let snapshots = SnapshotStore::new();
        for fill in 1..=PAIRING_CANDIDATE_CAPACITY as u8 {
            apply_candidate(
                &mut controls,
                &snapshots,
                fill,
                u64::from(fill),
                100 + u64::from(fill),
                b"",
                1,
            );
        }
        let worse = candidate(0xf0, 90, 100, b"");
        let worse_id = worse.candidate_id.clone();
        controls.upsert_candidate(worse, InstantMillis(1));

        assert_eq!(controls.candidates.len(), PAIRING_CANDIDATE_CAPACITY);
        assert!(!controls
            .candidates
            .iter()
            .any(|candidate| candidate.candidate_id == worse_id));
    }

    #[test]
    fn better_ninth_candidate_evicts_the_deterministic_worst() {
        let mut controls = PairingControls::default();
        let mut worst_id = String::new();
        for fill in 1..=PAIRING_CANDIDATE_CAPACITY as u8 {
            let candidate = candidate(fill, u64::from(fill), 100 + u64::from(fill), b"");
            if fill == 1 {
                worst_id.clone_from(&candidate.candidate_id);
            }
            controls.upsert_candidate(candidate, InstantMillis(1));
        }
        let better = candidate(0xf0, 90, 200, b"");
        let better_id = better.candidate_id.clone();
        controls.upsert_candidate(better, InstantMillis(1));

        assert_eq!(controls.candidates.len(), PAIRING_CANDIDATE_CAPACITY);
        assert!(!controls
            .candidates
            .iter()
            .any(|candidate| candidate.candidate_id == worst_id));
        assert!(controls
            .candidates
            .iter()
            .any(|candidate| candidate.candidate_id == better_id));
    }

    #[test]
    fn exact_candidate_selection_is_not_overwritten_by_later_observations() {
        let mut controls = PairingControls::default();
        let snapshots = SnapshotStore::new();
        let first = candidate(0x41, 1, 100, b"First");
        let second = candidate(0x42, 2, 100, b"Second");
        let second_id = second.candidate_id.clone();
        let second_endpoint = second.endpoint;
        controls.upsert_candidate(first, InstantMillis(2));
        controls.upsert_candidate(second, InstantMillis(2));

        assert_eq!(
            controls.resolve_candidate(&second_id, InstantMillis(2)),
            PairingCandidateResolution::Retained {
                endpoint: second_endpoint,
                expires_at: InstantMillis(100),
            }
        );
        controls.retain_initiation(second_id.clone());
        apply_candidate(&mut controls, &snapshots, 0x43, 3, 200, b"Third", 3);
        controls.retain_attempt("active-attempt".to_owned());
        apply_candidate(&mut controls, &snapshots, 0x44, 4, 250, b"Fourth", 4);

        assert_eq!(
            controls.initiated_candidate_id.as_deref(),
            Some(second_id.as_str())
        );
        assert_eq!(
            controls.active_attempt_id.as_deref(),
            Some("active-attempt")
        );
    }

    #[test]
    fn finishing_one_candidate_retains_another_for_exact_selection() {
        let mut controls = PairingControls::default();
        let snapshots = SnapshotStore::new();
        let first = candidate(0x41, 1, 100, b"First");
        let first_id = first.candidate_id.clone();
        let second = candidate(0x42, 2, 120, b"Second");
        let second_id = second.candidate_id.clone();
        let second_endpoint = second.endpoint;
        controls.upsert_candidate(first, InstantMillis(2));
        controls.upsert_candidate(second, InstantMillis(2));
        controls.publish_candidates(&snapshots, InstantMillis(2));

        controls.retain_initiation(first_id.clone());
        controls.retain_attempt("attempt".to_owned());
        let removed = controls.clear_attempt();
        controls.publish_candidates(&snapshots, InstantMillis(3));

        assert_eq!(removed.as_deref(), Some(first_id.as_str()));
        assert_eq!(snapshots.read().pairing_candidates.len(), 1);
        assert_eq!(
            controls.resolve_candidate(&second_id, InstantMillis(3)),
            PairingCandidateResolution::Retained {
                endpoint: second_endpoint,
                expires_at: InstantMillis(120),
            }
        );
    }

    #[test]
    fn candidate_resolution_distinguishes_missing_and_expired() {
        let mut controls = PairingControls::default();
        assert_eq!(
            controls.resolve_candidate("missing", InstantMillis(1)),
            PairingCandidateResolution::Missing
        );
        let expired = candidate(0x42, 1, 5, b"");
        let expired_id = expired.candidate_id.clone();
        controls.upsert_candidate(expired, InstantMillis(1));
        assert_eq!(
            controls.resolve_candidate(&expired_id, InstantMillis(5)),
            PairingCandidateResolution::Expired
        );

        let snapshots = SnapshotStore::new();
        let pruned = candidate(0x43, 1, 5, b"");
        let pruned_id = pruned.candidate_id.clone();
        controls.upsert_candidate(pruned, InstantMillis(1));
        expire_candidates(&mut controls, &snapshots, InstantMillis(5));
        assert_eq!(
            controls.resolve_candidate(&pruned_id, InstantMillis(6)),
            PairingCandidateResolution::Expired
        );
    }

    #[test]
    fn fresh_candidate_preserves_terminal_pairing_states() {
        for terminal in [
            RemoteControlPairingState::Failed {
                stage: crate::contract::RemoteControlPairingFailureStage::Request,
                detail: "old failure".to_owned(),
            },
            RemoteControlPairingState::Paired {
                attempt_id: "completed".to_owned(),
            },
        ] {
            let mut controls = PairingControls::default();
            let snapshots = SnapshotStore::new();
            let expected = terminal.clone();
            snapshots.update(|snapshot| snapshot.pairing = terminal);

            apply_candidate(&mut controls, &snapshots, 0x42, 5, 100, b"Recovered", 5);

            let retained = snapshots.read();
            assert_eq!(retained.pairing, expected);
            assert_eq!(retained.pairing_candidates.len(), 1);
        }
    }

    #[test]
    fn candidate_updates_do_not_clear_a_terminal_state() {
        let mut controls = PairingControls::default();
        let snapshots = SnapshotStore::new();
        apply_candidate(&mut controls, &snapshots, 0x42, 10, 100, b"Current", 10);
        snapshots.update(|snapshot| {
            snapshot.pairing = RemoteControlPairingState::Paired {
                attempt_id: "completed".to_owned(),
            };
        });

        apply_candidate(&mut controls, &snapshots, 0x42, 10, 100, b"Current", 11);
        assert!(matches!(
            snapshots.read().pairing,
            RemoteControlPairingState::Paired { .. }
        ));
        apply_candidate(&mut controls, &snapshots, 0x42, 9, 200, b"Stale", 11);
        assert!(matches!(
            snapshots.read().pairing,
            RemoteControlPairingState::Paired { .. }
        ));

        apply_candidate(&mut controls, &snapshots, 0x42, 11, 120, b"Fresh", 11);
        assert!(matches!(
            snapshots.read().pairing,
            RemoteControlPairingState::Paired { .. }
        ));
    }

    fn apply_candidate(
        controls: &mut PairingControls,
        snapshots: &SnapshotStore,
        fill: u8,
        observed_at: u64,
        expires_at: u64,
        public_app_data: &[u8],
        now: u64,
    ) {
        let candidate = candidate(fill, observed_at, expires_at, public_app_data);
        apply_event(
            OwnedNodeEvent::PairingAvailable {
                endpoint: candidate.endpoint,
                observed_at: candidate.observed_at,
                expires_at: candidate.expires_at,
                public_app_data: public_app_data.to_vec(),
            },
            controls,
            snapshots,
            InstantMillis(now),
        );
    }

    fn candidate(
        fill: u8,
        observed_at: u64,
        expires_at: u64,
        public_app_data: &[u8],
    ) -> PairingCandidateControl {
        let endpoint = personal_rns::remote_control::RemoteControlPairingIdentity::new(
            personal_rns::identity::IdentityHash::new([fill; 16]),
        )
        .endpoint();
        PairingCandidateControl {
            candidate_id: bytes_hex(endpoint.destination_hash().as_bytes()),
            endpoint,
            display_name: safe_display_name(public_app_data),
            observed_at: InstantMillis(observed_at),
            expires_at: InstantMillis(expires_at),
        }
    }

    fn pairing_attempt_id(fill: u8) -> RemoteControlPairingAttemptId {
        use personal_rns::identity::in_memory::InMemoryNodeIdentity;
        use personal_rns::identity::vault::IdentitySecretKey;
        use personal_rns::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
        use personal_rns::remote_control::{
            RemoteControlControllerIdentity, RemoteControlPairingAttemptTimeout,
            RemoteControlPairingBegin, RemoteControlPairingContext, RemoteControlPairingIdentity,
            RemoteControlPairingInvitationCode, RemoteControlPairingPermissions,
            RemoteControlPairingPreparedOffer,
        };
        use personal_rns::routing::links::LinkId;
        use personal_rns::units::DurationMillis;

        let controller_signer = InMemoryNodeIdentity::from_secret_key_bytes(
            &IdentitySecretKey::new([fill; personal_rns::identity::IDENTITY_SECRET_KEY_LEN]),
        );
        let controller = RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: controller_signer.encryption_public_key(),
            signing: controller_signer.signing_public_key(),
        });
        let target_signer = InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
            [fill.wrapping_add(1); personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        ));
        let endpoint = RemoteControlPairingIdentity::new(IdentityHash::new([fill; 16])).endpoint();
        let context =
            RemoteControlPairingContext::new(endpoint, LinkId::new([fill.wrapping_add(2); 16]));
        let begin = RemoteControlPairingBegin::new(
            controller,
            endpoint,
            RemoteControlPairingInvitationCode::from_value(u32::from(fill)),
        );
        let prepared = RemoteControlPairingPreparedOffer::new(
            &target_signer,
            context,
            &begin,
            RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::all())
                .expect("nonempty permissions"),
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(5_000))
                .expect("valid attempt timeout"),
        );
        RemoteControlPairingAttemptId::from(prepared.transcript())
    }
}
