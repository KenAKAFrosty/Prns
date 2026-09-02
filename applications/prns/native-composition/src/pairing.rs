use std::sync::atomic::{AtomicBool, Ordering};

use personal_rns::prelude::{Message, PrnsEvent, RemoteControlControllerPairingConfirmation};
use personal_rns::remote_control::{
    RemoteControlControllerPairingAborted, RemoteControlPairingAttemptId,
    RemoteControlPairingEndpoint, RemoteControlRequestSet,
};
use personal_rns::units::InstantMillis;
use tokio::sync::mpsc;

use crate::contract::{
    DevelopmentNodeFailure, DevelopmentNodeFailureStage, RemoteControlPairingCandidate,
    RemoteControlPairingState, RemoteControlRequestKind, U64String,
};
use crate::snapshot::SnapshotStore;

pub const EVENT_LANE_CAPACITY: usize = 16;

pub struct PairingCandidateControl {
    pub candidate_id: String,
    pub endpoint: RemoteControlPairingEndpoint,
    pub expires_at: InstantMillis,
}

#[derive(Default)]
pub struct PairingControls {
    pub candidate: Option<PairingCandidateControl>,
    pub confirmation: Option<RemoteControlControllerPairingConfirmation>,
    pub initiated_candidate_id: Option<String>,
    pub active_attempt_id: Option<String>,
}

pub enum OwnedNodeEvent {
    PersistenceRestored,
    PairingAvailable {
        endpoint: RemoteControlPairingEndpoint,
        observed_at: InstantMillis,
        expires_at: InstantMillis,
        public_app_data: Vec<u8>,
    },
    ControllerConfirmation(RemoteControlControllerPairingConfirmation),
    ControllerAuthorizationPersisted(RemoteControlPairingAttemptId),
    ControllerExpired {
        attempt_id: Option<RemoteControlPairingAttemptId>,
    },
    ControllerLinkClosed {
        attempt_id: Option<RemoteControlPairingAttemptId>,
    },
    PersistenceFailed,
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
        PrnsEvent::Diagnostic(personal_rns::Diagnostic::PersistenceFlushFailed { .. }) => {
            Some(OwnedNodeEvent::PersistenceFailed)
        }
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
) -> AppliedNodeEvent {
    match event {
        OwnedNodeEvent::PersistenceRestored => return AppliedNodeEvent::PersistenceRestored,
        OwnedNodeEvent::PairingAvailable {
            endpoint,
            observed_at,
            expires_at,
            public_app_data,
        } => {
            if controls.initiated_candidate_id.is_some() || controls.active_attempt_id.is_some() {
                return AppliedNodeEvent::None;
            }
            let candidate_id = bytes_hex(endpoint.destination_hash().as_bytes());
            controls.candidate = Some(PairingCandidateControl {
                candidate_id: candidate_id.clone(),
                endpoint,
                expires_at,
            });
            snapshots.update(|snapshot| {
                snapshot.pairing = RemoteControlPairingState::CandidateObserved {
                    candidate: RemoteControlPairingCandidate {
                        candidate_id,
                        endpoint: endpoint.destination_hash().as_bytes().to_vec(),
                        observed_at_millis: U64String::from(observed_at.0),
                        expires_at_millis: U64String::from(expires_at.0),
                        public_app_data,
                    },
                };
            });
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
            controls.confirmation = Some(confirmation);
        }
        OwnedNodeEvent::ControllerAuthorizationPersisted(attempt_id) => {
            let observed_attempt_id = attempt_id_string(attempt_id);
            if controls.active_attempt_id.as_deref() != Some(observed_attempt_id.as_str()) {
                return AppliedNodeEvent::None;
            }
            controls.clear_attempt();
            snapshots.update(|snapshot| {
                snapshot.pairing = RemoteControlPairingState::Paired {
                    attempt_id: observed_attempt_id,
                };
                snapshot.active_operation = None;
            });
            return AppliedNodeEvent::TargetInventoryChanged;
        }
        OwnedNodeEvent::ControllerExpired { attempt_id } => {
            if !controls.matches_terminal(attempt_id) {
                return AppliedNodeEvent::None;
            }
            controls.clear_attempt();
            snapshots.update(|snapshot| {
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
            controls.clear_attempt();
            snapshots.update(|snapshot| {
                snapshot.pairing = RemoteControlPairingState::Failed {
                    stage: crate::contract::RemoteControlPairingFailureStage::Link,
                    detail: "The upstream pairing Link closed before authorization persisted."
                        .to_owned(),
                };
                snapshot.active_operation = None;
            });
        }
        OwnedNodeEvent::PersistenceFailed => snapshots.fail(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::PersistenceRestore,
            detail: "Prns reported a persistence flush failure.".to_owned(),
        }),
    }
    AppliedNodeEvent::None
}

impl PairingControls {
    pub fn retain_initiation(&mut self, candidate_id: String) {
        self.initiated_candidate_id = Some(candidate_id);
        self.confirmation = None;
        self.active_attempt_id = None;
    }

    pub fn retain_attempt(&mut self, attempt_id: String) {
        self.candidate = None;
        self.active_attempt_id = Some(attempt_id);
    }

    pub fn cancel_initiation(&mut self) {
        self.confirmation = None;
        self.initiated_candidate_id = None;
        self.active_attempt_id = None;
    }

    pub fn clear_attempt(&mut self) {
        self.candidate = None;
        self.confirmation = None;
        self.initiated_candidate_id = None;
        self.active_attempt_id = None;
    }

    fn matches_terminal(&self, attempt_id: Option<RemoteControlPairingAttemptId>) -> bool {
        let observed = attempt_id.map(attempt_id_string);
        self.matches_terminal_id(observed.as_deref())
    }

    fn matches_confirmation_id(&self, observed_attempt_id: &str) -> bool {
        self.active_attempt_id.as_deref() == Some(observed_attempt_id)
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

pub fn expire_candidate(
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    now: InstantMillis,
) {
    let Some(candidate) = controls.candidate.as_ref() else {
        return;
    };
    if candidate.expires_at > now {
        return;
    }
    let candidate_id = candidate.candidate_id.clone();
    controls.candidate = None;
    snapshots.update(|snapshot| {
        if matches!(
            &snapshot.pairing,
            RemoteControlPairingState::CandidateObserved { candidate }
                if candidate.candidate_id == candidate_id
        ) {
            snapshot.pairing = RemoteControlPairingState::Expired {
                detail: "The signed pairing availability observation expired.".to_owned(),
            };
            snapshot.active_operation = None;
        }
    });
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

pub fn apply_overflow_failure(snapshots: &SnapshotStore) {
    snapshots.update(|snapshot| {
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
    fn confirmations_only_match_the_exact_active_attempt() {
        let mut controls = PairingControls::default();
        assert!(!controls.matches_confirmation_id("current"));

        controls.initiated_candidate_id = Some("candidate".to_owned());
        assert!(!controls.matches_confirmation_id("current"));

        controls.retain_attempt("current".to_owned());
        assert!(!controls.matches_confirmation_id("stale"));
        assert!(controls.matches_confirmation_id("current"));
    }

    #[test]
    fn signed_candidate_expires_at_its_upstream_deadline() {
        let endpoint = personal_rns::remote_control::RemoteControlPairingIdentity::new(
            personal_rns::identity::IdentityHash::new([0x42; 16]),
        )
        .endpoint();
        let candidate_id = bytes_hex(endpoint.destination_hash().as_bytes());
        let mut controls = PairingControls {
            candidate: Some(PairingCandidateControl {
                candidate_id: candidate_id.clone(),
                endpoint,
                expires_at: InstantMillis(5),
            }),
            ..PairingControls::default()
        };
        let snapshots = SnapshotStore::new();
        snapshots.update(|snapshot| {
            snapshot.pairing = RemoteControlPairingState::CandidateObserved {
                candidate: RemoteControlPairingCandidate {
                    candidate_id,
                    endpoint: endpoint.destination_hash().as_bytes().to_vec(),
                    observed_at_millis: U64String::from(1),
                    expires_at_millis: U64String::from(5),
                    public_app_data: Vec::new(),
                },
            };
        });

        expire_candidate(&mut controls, &snapshots, InstantMillis(4));
        assert!(controls.candidate.is_some());
        expire_candidate(&mut controls, &snapshots, InstantMillis(5));
        assert!(controls.candidate.is_none());
        assert!(matches!(
            snapshots.read().pairing,
            RemoteControlPairingState::Expired { .. }
        ));
    }
}
