//! Reusable opt-in RemoteControl service configuration and owned public observations.
//! Private identities use the host's existing zeroizing identity source. Paths and
//! enabled capabilities belong to the caller; this module has no application policy.

use crate::{resolve_identity, NativeStartError};
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::interfaces::InterfaceId;
use personal_rns::remote_control::*;
use personal_rns::runtime::{Message, RemoteControlPairingConfirmation};
use personal_rns::units::InstantMillis;
use prns_host::IdentityConfig;

pub struct NativeRemoteControlConfig {
    pub controller_identity: IdentityConfig,
    pub target_identity: IdentityConfig,
    pub initial_controller_grants: Vec<RemoteControlControllerGrant>,
    pub self_announcement: RemoteControlSelfAnnouncement,
    pub capabilities: RemoteControlCapabilities,
}

pub(crate) struct ResolvedRemoteControlConfig {
    identities: RemoteControlNodeIdentitySecrets,
    grants: Vec<RemoteControlControllerGrant>,
    self_announcement: RemoteControlSelfAnnouncement,
    capabilities: RemoteControlCapabilities,
}

impl NativeRemoteControlConfig {
    pub(crate) fn resolve(self) -> Result<ResolvedRemoteControlConfig, NativeStartError> {
        // Validate grants before resolving caller-selected identity sources.
        if !self.initial_controller_grants.is_empty() {
            RemoteControlControllerGrants::try_from(self.initial_controller_grants.as_slice())
                .map_err(|error| {
                    NativeStartError::Runtime(format!(
                        "invalid initial remote-control grants: {error:?}"
                    ))
                })?;
        }
        let controller = resolve_identity(self.controller_identity)?;
        let target = resolve_identity(self.target_identity)?;
        let identities = RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(*controller)),
            RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(*target)),
        )
        .map_err(|error| NativeStartError::Runtime(error.to_string()))?;
        Ok(ResolvedRemoteControlConfig {
            identities,
            grants: self.initial_controller_grants,
            self_announcement: self.self_announcement,
            capabilities: self.capabilities,
        })
    }
}

impl ResolvedRemoteControlConfig {
    pub(crate) fn into_parts(
        self,
    ) -> (
        RemoteControlNodeIdentitySecrets,
        Vec<RemoteControlControllerGrant>,
        RemoteControlSelfAnnouncement,
        RemoteControlCapabilities,
    ) {
        (
            self.identities,
            self.grants,
            self.self_announcement,
            self.capabilities,
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NativeRemoteControlConfirmation {
    pub attempt_id: RemoteControlPairingAttemptId,
    pub context: RemoteControlPairingContext,
    pub controller: RemoteControlControllerIdentity,
    pub target: RemoteControlTargetIdentity,
    pub permissions: RemoteControlPairingPermissions,
    pub confirmation_code: String,
}

impl From<&RemoteControlPairingConfirmation> for NativeRemoteControlConfirmation {
    fn from(value: &RemoteControlPairingConfirmation) -> Self {
        Self {
            attempt_id: value.attempt_id(),
            context: value.context(),
            controller: *value.controller(),
            target: RemoteControlTargetIdentity::new(*value.target().public_keys()),
            permissions: value.permissions().clone(),
            confirmation_code: value.confirmation_code().to_string(),
        }
    }
}

/// Public facts share the existing application event queue and its sole consumer.
/// The engine keeps the bounded live attempt; decisions carry its attempt id and
/// are rejected by the engine after expiry, completion, rejection or link closure.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq, Eq)]
pub enum NativeRemoteControlEvent {
    PairingAvailable {
        endpoint: RemoteControlPairingEndpoint,
        observed_at: InstantMillis,
        expires_at: InstantMillis,
        hops: u8,
        source_interface: InterfaceId,
        public_app_data: Vec<u8>,
    },
    TargetConfirmationRequired {
        confirmation: NativeRemoteControlConfirmation,
        window: RemoteControlTargetPairingAttemptWindow,
    },
    TargetControllerCommitted {
        attempt_id: RemoteControlPairingAttemptId,
    },
    TargetAuthorizationRequired {
        attempt_id: RemoteControlPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
    TargetAuthorizationPersisted {
        attempt_id: RemoteControlPairingAttemptId,
    },
    TargetExpiredDuringAuthorization {
        attempt_id: RemoteControlPairingAttemptId,
    },
    ControllerConfirmationRequired {
        confirmation: NativeRemoteControlConfirmation,
        window: RemoteControlControllerPairingAttemptWindow,
    },
    ControllerPersistenceRequired {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
        target: RemoteControlTargetIdentity,
        authority: RemoteControlControllerAuthority,
        permitted_requests: RemoteControlRequestSet,
    },
    ControllerAuthorizationPersisted {
        attempt_id: RemoteControlPairingAttemptId,
    },
    ControllerAuthorizationPersistenceFailed {
        attempt_id: RemoteControlPairingAttemptId,
    },
    ControllerExpired {
        aborted: RemoteControlControllerPairingAborted,
    },
    ControllerLinkClosed {
        aborted: RemoteControlControllerPairingAborted,
    },
    TargetExpired {
        aborted: RemoteControlTargetPairingAborted,
    },
    TargetLinkClosed {
        aborted: RemoteControlTargetPairingAborted,
    },
    TargetCompletionRetentionExpired {
        attempt_id: RemoteControlPairingAttemptId,
    },
    TargetCompletionLinkClosed {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

impl NativeRemoteControlEvent {
    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::PairingAvailable {
                public_app_data, ..
            } => public_app_data.len(),
            Self::TargetConfirmationRequired { confirmation, .. }
            | Self::ControllerConfirmationRequired { confirmation, .. } => {
                confirmation.confirmation_code.len()
            }
            _ => 0,
        }
    }

    /// Borrows for projection so NativeCallback embedding retains the original
    /// move-only confirmation. Queue-mode callers move only this public projection.
    pub fn from_message(message: &Message<'_>) -> Option<Self> {
        Some(match message {
            Message::RemoteControlPairingAvailable(value) => Self::PairingAvailable {
                endpoint: value.endpoint(),
                observed_at: value.observed_at(),
                expires_at: value.expires_at(),
                hops: value.hops().0,
                source_interface: value.source_interface(),
                public_app_data: value.public_app_data().as_bytes().to_vec(),
            },
            Message::RemoteControlTargetPairingConfirmationRequired(value) => {
                Self::TargetConfirmationRequired {
                    confirmation: value.confirmation().into(),
                    window: value.window(),
                }
            }
            Message::RemoteControlControllerPairingConfirmationRequired(value) => {
                Self::ControllerConfirmationRequired {
                    confirmation: value.confirmation().into(),
                    window: value.window(),
                }
            }
            Message::RemoteControlControllerPairingPersistenceRequired(value) => {
                Self::ControllerPersistenceRequired {
                    attempt_id: value.attempt_id(),
                    context: value.context(),
                    target: RemoteControlTargetIdentity::new(
                        *value.access().target().public_keys(),
                    ),
                    authority: value.access().authority(),
                    permitted_requests: *value.access().permitted_requests(),
                }
            }
            Message::RemoteControlTargetPairingControllerCommitted { attempt_id } => {
                Self::TargetControllerCommitted {
                    attempt_id: *attempt_id,
                }
            }
            Message::RemoteControlTargetPairingAuthorizationRequired { attempt_id, grant } => {
                Self::TargetAuthorizationRequired {
                    attempt_id: *attempt_id,
                    grant: *grant,
                }
            }
            Message::RemoteControlTargetPairingAuthorizationPersisted { attempt_id } => {
                Self::TargetAuthorizationPersisted {
                    attempt_id: *attempt_id,
                }
            }
            Message::RemoteControlTargetPairingExpiredDuringAuthorization { attempt_id } => {
                Self::TargetExpiredDuringAuthorization {
                    attempt_id: *attempt_id,
                }
            }
            Message::RemoteControlControllerPairingAuthorizationPersisted { attempt_id } => {
                Self::ControllerAuthorizationPersisted {
                    attempt_id: *attempt_id,
                }
            }
            Message::RemoteControlControllerPairingAuthorizationPersistenceFailed {
                attempt_id,
            } => Self::ControllerAuthorizationPersistenceFailed {
                attempt_id: *attempt_id,
            },
            Message::RemoteControlControllerPairingExpired { aborted } => {
                Self::ControllerExpired { aborted: *aborted }
            }
            Message::RemoteControlControllerPairingLinkClosed { aborted } => {
                Self::ControllerLinkClosed { aborted: *aborted }
            }
            Message::RemoteControlTargetPairingExpired { aborted } => {
                Self::TargetExpired { aborted: *aborted }
            }
            Message::RemoteControlTargetPairingLinkClosed { aborted } => {
                Self::TargetLinkClosed { aborted: *aborted }
            }
            Message::RemoteControlTargetPairingCompletionRetentionExpired { attempt_id } => {
                Self::TargetCompletionRetentionExpired {
                    attempt_id: *attempt_id,
                }
            }
            Message::RemoteControlTargetPairingCompletionLinkClosed { attempt_id } => {
                Self::TargetCompletionLinkClosed {
                    attempt_id: *attempt_id,
                }
            }
            Message::Delivered(_)
            | Message::Request { .. }
            | Message::Response { .. }
            | Message::ResponseSegment { .. }
            | Message::Resource { .. }
            | Message::ResourceSegment { .. }
            | Message::ChannelMessage { .. } => return None,
        })
    }
}
