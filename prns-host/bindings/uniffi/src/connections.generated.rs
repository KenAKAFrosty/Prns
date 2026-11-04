// Generated from canonical native connection declarations. Do not edit.
#![allow(clippy::needless_question_mark)]
use crate::connections::{ConnectionBindingClient, ConnectionProfileClient};
pub const CONNECTION_SEMANTIC_FINGERPRINT: &str =
    "f711f3861576437ef4b51ebf653a76d75c899c19f7bcd8acff654c218922d672";
fn invalid(reason: prns_host_runtime::connections::InputError) -> ConnectionError {
    prns_host_runtime::connections::Error::Invalid(reason).into()
}
fn fixed16(value: Vec<u8>) -> Result<[u8; 16], ConnectionError> {
    value
        .try_into()
        .map_err(|_| invalid(prns_host_runtime::connections::InputError::InvalidId))
}
#[derive(Debug, uniffi::Record)]
pub struct ConnectionOptions {
    pub limits: ConnectionLimits,
    pub queue_capacity: u32,
    pub cache_bytes: u64,
}
impl TryFrom<ConnectionOptions> for prns_host_runtime::connections::Options {
    type Error = ConnectionError;
    fn try_from(value: ConnectionOptions) -> Result<Self, Self::Error> {
        Ok(Self {
            limits: value.limits.try_into()?,
            queue_capacity: core::num::NonZeroUsize::new(
                usize::try_from(value.queue_capacity).map_err(|_| {
                    invalid(prns_host_runtime::connections::InputError::InvalidLimits)
                })?,
            )
            .ok_or_else(|| invalid(prns_host_runtime::connections::InputError::InvalidLimits))?,
            cache_bytes: usize::try_from(value.cache_bytes)
                .map_err(|_| invalid(prns_host_runtime::connections::InputError::InvalidLimits))?,
        })
    }
}
#[derive(Debug, uniffi::Record)]
pub struct ConnectionLimits {
    pub profiles: u32,
    pub encoded_bytes: u32,
}
impl TryFrom<ConnectionLimits> for prns_host_runtime::connections::Limits {
    type Error = ConnectionError;
    fn try_from(value: ConnectionLimits) -> Result<Self, Self::Error> {
        Ok(Self {
            profiles: value.profiles,
            encoded_bytes: value.encoded_bytes,
        })
    }
}
impl From<prns_host_runtime::connections::Limits> for ConnectionLimits {
    fn from(value: prns_host_runtime::connections::Limits) -> Self {
        Self {
            profiles: value.profiles,
            encoded_bytes: value.encoded_bytes,
        }
    }
}
#[derive(Debug, uniffi::Record)]
pub struct ConnectionProfile {
    pub id: Vec<u8>,
    pub label: String,
    pub enabled: bool,
    pub config: crate::transport::InterfaceConfig,
    pub routing: Option<crate::transport::InterfaceRoutingPolicy>,
}
impl TryFrom<ConnectionProfile> for prns_host_runtime::connections::Profile {
    type Error = ConnectionError;
    fn try_from(value: ConnectionProfile) -> Result<Self, Self::Error> {
        Ok(Self {
            id: prns_host_runtime::connections::ProfileId::new(fixed16(value.id)?)
                .map_err(ConnectionError::from)?,
            label: value.label,
            enabled: value.enabled,
            config: value
                .config
                .try_into()
                .map_err(|_| invalid(prns_host_runtime::connections::InputError::InvalidConfig))?,
            routing: value
                .routing
                .map(|item| {
                    Ok::<_, ConnectionError>(item.try_into().map_err(|_| {
                        invalid(prns_host_runtime::connections::InputError::InvalidConfig)
                    })?)
                })
                .transpose()?,
        })
    }
}
impl From<prns_host_runtime::connections::Profile> for ConnectionProfile {
    fn from(value: prns_host_runtime::connections::Profile) -> Self {
        Self {
            id: value.id.bytes().to_vec(),
            label: value.label,
            enabled: value.enabled,
            config: value.config.into(),
            routing: value.routing.map(|item| item.into()),
        }
    }
}
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ConnectionError {
    #[error("connection profiles: Invalid value={value:?}")]
    Invalid { value: ConnectionInputError },
    #[error("connection profiles: Schema value={value:?}")]
    Schema { value: ConnectionSchemaError },
    #[error("connection profiles: Conflict value={value:?}")]
    Conflict { value: ConnectionSnapshot },
    #[error("connection profiles: Storage value={value:?}")]
    Storage { value: String },
    #[error("connection profiles: File value={value:?}")]
    File { value: String },
    #[error("connection profiles: Spawn value={value:?}")]
    Spawn { value: String },
    #[error("connection profiles: EntropyUnavailable")]
    EntropyUnavailable,
    #[error("connection profiles: Busy")]
    Busy,
    #[error("connection profiles: Closed")]
    Closed,
    #[error("connection profiles: Reentrant")]
    Reentrant,
    #[error("connection profiles: WorkerFailed")]
    WorkerFailed,
    #[error("connection profiles: AlreadyBound")]
    AlreadyBound,
    #[error("connection profiles: BindingChanged")]
    BindingChanged,
    #[error("connection profiles: NotEnabled")]
    NotEnabled,
    #[error("connection profiles: Host value={value:?}")]
    Host {
        value: crate::transport::BindingError,
    },
    #[error("connection profiles: Attachment value={value:?}")]
    Attachment { value: ConnectionAttachmentError },
}
impl From<prns_host_runtime::connections::Error> for ConnectionError {
    fn from(value: prns_host_runtime::connections::Error) -> Self {
        match value {
            prns_host_runtime::connections::Error::Invalid(value) => ConnectionError::Invalid {
                value: value.into(),
            },
            prns_host_runtime::connections::Error::Schema(value) => ConnectionError::Schema {
                value: value.into(),
            },
            prns_host_runtime::connections::Error::Conflict(value) => ConnectionError::Conflict {
                value: value.into(),
            },
            prns_host_runtime::connections::Error::Storage(value) => ConnectionError::Storage {
                value: value.to_string(),
            },
            prns_host_runtime::connections::Error::File(value) => ConnectionError::File {
                value: value.to_string(),
            },
            prns_host_runtime::connections::Error::Spawn(value) => ConnectionError::Spawn {
                value: value.to_string(),
            },
            prns_host_runtime::connections::Error::EntropyUnavailable => {
                ConnectionError::EntropyUnavailable
            }
            prns_host_runtime::connections::Error::Busy => ConnectionError::Busy,
            prns_host_runtime::connections::Error::Closed => ConnectionError::Closed,
            prns_host_runtime::connections::Error::Reentrant => ConnectionError::Reentrant,
            prns_host_runtime::connections::Error::WorkerFailed => ConnectionError::WorkerFailed,
            prns_host_runtime::connections::Error::AlreadyBound => ConnectionError::AlreadyBound,
            prns_host_runtime::connections::Error::BindingChanged => {
                ConnectionError::BindingChanged
            }
            prns_host_runtime::connections::Error::NotEnabled => ConnectionError::NotEnabled,
            prns_host_runtime::connections::Error::Host(value) => ConnectionError::Host {
                value: crate::facade::binding_error(value),
            },
            prns_host_runtime::connections::Error::Attachment(value) => {
                ConnectionError::Attachment {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(Debug, uniffi::Enum)]
pub enum ConnectionInputError {
    InvalidLimits,
    InvalidId,
    InvalidLabel,
    InvalidTarget,
    InvalidConfig,
    UnsupportedConfig,
    DuplicateId,
    DuplicateEndpoint,
    MultipleLanProfiles,
    TooManyProfiles,
    EncodedCapacity,
}
impl From<prns_host_runtime::connections::InputError> for ConnectionInputError {
    fn from(value: prns_host_runtime::connections::InputError) -> Self {
        match value {
            prns_host_runtime::connections::InputError::InvalidLimits => {
                ConnectionInputError::InvalidLimits
            }
            prns_host_runtime::connections::InputError::InvalidId => {
                ConnectionInputError::InvalidId
            }
            prns_host_runtime::connections::InputError::InvalidLabel => {
                ConnectionInputError::InvalidLabel
            }
            prns_host_runtime::connections::InputError::InvalidTarget => {
                ConnectionInputError::InvalidTarget
            }
            prns_host_runtime::connections::InputError::InvalidConfig => {
                ConnectionInputError::InvalidConfig
            }
            prns_host_runtime::connections::InputError::UnsupportedConfig => {
                ConnectionInputError::UnsupportedConfig
            }
            prns_host_runtime::connections::InputError::DuplicateId => {
                ConnectionInputError::DuplicateId
            }
            prns_host_runtime::connections::InputError::DuplicateEndpoint => {
                ConnectionInputError::DuplicateEndpoint
            }
            prns_host_runtime::connections::InputError::MultipleLanProfiles => {
                ConnectionInputError::MultipleLanProfiles
            }
            prns_host_runtime::connections::InputError::TooManyProfiles => {
                ConnectionInputError::TooManyProfiles
            }
            prns_host_runtime::connections::InputError::EncodedCapacity => {
                ConnectionInputError::EncodedCapacity
            }
        }
    }
}
#[derive(Debug, uniffi::Enum)]
pub enum ConnectionSchemaError {
    Missing,
    Corrupt,
    UnsupportedVersion { value: u32 },
    CapacityMismatch,
    RevisionExhausted,
}
impl From<prns_host_runtime::connections::SchemaError> for ConnectionSchemaError {
    fn from(value: prns_host_runtime::connections::SchemaError) -> Self {
        match value {
            prns_host_runtime::connections::SchemaError::Missing => ConnectionSchemaError::Missing,
            prns_host_runtime::connections::SchemaError::Corrupt => ConnectionSchemaError::Corrupt,
            prns_host_runtime::connections::SchemaError::UnsupportedVersion(value) => {
                ConnectionSchemaError::UnsupportedVersion { value }
            }
            prns_host_runtime::connections::SchemaError::CapacityMismatch => {
                ConnectionSchemaError::CapacityMismatch
            }
            prns_host_runtime::connections::SchemaError::RevisionExhausted => {
                ConnectionSchemaError::RevisionExhausted
            }
        }
    }
}
#[derive(Debug, uniffi::Record)]
pub struct ConnectionSnapshot {
    pub guard: ConnectionGuard,
    pub limits: ConnectionLimits,
    pub profiles: Vec<ConnectionProfile>,
}
impl From<prns_host_runtime::connections::Snapshot> for ConnectionSnapshot {
    fn from(value: prns_host_runtime::connections::Snapshot) -> Self {
        Self {
            guard: value.guard.into(),
            limits: value.limits.into(),
            profiles: value.profiles.into_iter().map(|item| item.into()).collect(),
        }
    }
}
#[derive(Debug, uniffi::Record)]
pub struct ConnectionGuard {
    pub incarnation: Vec<u8>,
    pub revision: u64,
}
impl From<prns_host_runtime::connections::Guard> for ConnectionGuard {
    fn from(value: prns_host_runtime::connections::Guard) -> Self {
        Self {
            incarnation: value.incarnation.to_vec(),
            revision: value.revision,
        }
    }
}
impl TryFrom<ConnectionGuard> for prns_host_runtime::connections::Guard {
    type Error = ConnectionError;
    fn try_from(value: ConnectionGuard) -> Result<Self, Self::Error> {
        Ok(Self {
            incarnation: fixed16(value.incarnation)?,
            revision: value.revision,
        })
    }
}
#[derive(Debug, uniffi::Enum)]
pub enum ConnectionAttachmentError {
    Closed,
    Conflict,
    NotOwned,
    Session {
        value: crate::transport::BindingError,
    },
    Command {
        value: crate::transport::CommandFailure,
    },
    RetirementPending {
        interface: Vec<u8>,
        failure: crate::transport::CommandFailure,
    },
}
impl From<prns_host_runtime::attachment_scope::AttachmentError> for ConnectionAttachmentError {
    fn from(value: prns_host_runtime::attachment_scope::AttachmentError) -> Self {
        match value {
            prns_host_runtime::attachment_scope::AttachmentError::Closed => {
                ConnectionAttachmentError::Closed
            }
            prns_host_runtime::attachment_scope::AttachmentError::Conflict => {
                ConnectionAttachmentError::Conflict
            }
            prns_host_runtime::attachment_scope::AttachmentError::NotOwned => {
                ConnectionAttachmentError::NotOwned
            }
            prns_host_runtime::attachment_scope::AttachmentError::Session(value) => {
                ConnectionAttachmentError::Session {
                    value: crate::facade::binding_error(value),
                }
            }
            prns_host_runtime::attachment_scope::AttachmentError::Command(value) => {
                ConnectionAttachmentError::Command {
                    value: value.into(),
                }
            }
            prns_host_runtime::attachment_scope::AttachmentError::RetirementPending {
                interface,
                failure,
            } => ConnectionAttachmentError::RetirementPending {
                interface: interface.as_bytes().to_vec(),
                failure: failure.into(),
            },
        }
    }
}
#[derive(Debug, uniffi::Record)]
pub struct ConnectionBindingStatus {
    pub generation: u64,
    pub phase: ConnectionBindingPhase,
    pub cleanup_failure: Option<ConnectionError>,
}
impl From<prns_host_runtime::connections::BindingStatus> for ConnectionBindingStatus {
    fn from(value: prns_host_runtime::connections::BindingStatus) -> Self {
        Self {
            generation: value.generation,
            phase: value.phase.into(),
            cleanup_failure: value.cleanup_failure.map(|item| item.into()),
        }
    }
}
#[derive(Debug, uniffi::Enum)]
pub enum ConnectionBindingPhase {
    Active,
    Fenced,
    Closing,
    Closed,
}
impl From<prns_host_runtime::connections::BindingPhase> for ConnectionBindingPhase {
    fn from(value: prns_host_runtime::connections::BindingPhase) -> Self {
        match value {
            prns_host_runtime::connections::BindingPhase::Active => ConnectionBindingPhase::Active,
            prns_host_runtime::connections::BindingPhase::Fenced => ConnectionBindingPhase::Fenced,
            prns_host_runtime::connections::BindingPhase::Closing => {
                ConnectionBindingPhase::Closing
            }
            prns_host_runtime::connections::BindingPhase::Closed => ConnectionBindingPhase::Closed,
        }
    }
}
#[derive(Debug, uniffi::Record)]
pub struct ConnectionBindingSnapshot {
    pub generation: u64,
    pub saved: ConnectionGuard,
    pub applied_revision: Option<u64>,
    pub phase: ConnectionBindingPhase,
    pub attachments: Vec<ConnectionLiveProfile>,
}
impl From<prns_host_runtime::connections::BindingSnapshot> for ConnectionBindingSnapshot {
    fn from(value: prns_host_runtime::connections::BindingSnapshot) -> Self {
        Self {
            generation: value.generation,
            saved: value.saved.into(),
            applied_revision: value.applied_revision,
            phase: value.phase.into(),
            attachments: value
                .attachments
                .into_iter()
                .map(|item| item.into())
                .collect(),
        }
    }
}
#[derive(Debug, uniffi::Record)]
pub struct ConnectionLiveProfile {
    pub profile: ConnectionProfile,
    pub interface: Option<Vec<u8>>,
    pub state: ConnectionAttachmentState,
    pub failure: Option<ConnectionError>,
}
impl From<prns_host_runtime::connections::LiveProfile> for ConnectionLiveProfile {
    fn from(value: prns_host_runtime::connections::LiveProfile) -> Self {
        Self {
            profile: value.profile.into(),
            interface: value.interface.map(|item| item.as_bytes().to_vec()),
            state: value.state.into(),
            failure: value.failure.map(|item| item.into()),
        }
    }
}
#[derive(Debug, uniffi::Enum)]
pub enum ConnectionAttachmentState {
    Attaching,
    Attached,
    Retiring,
    Failed,
}
impl From<prns_host_runtime::connections::AttachmentState> for ConnectionAttachmentState {
    fn from(value: prns_host_runtime::connections::AttachmentState) -> Self {
        match value {
            prns_host_runtime::connections::AttachmentState::Attaching => {
                ConnectionAttachmentState::Attaching
            }
            prns_host_runtime::connections::AttachmentState::Attached => {
                ConnectionAttachmentState::Attached
            }
            prns_host_runtime::connections::AttachmentState::Retiring => {
                ConnectionAttachmentState::Retiring
            }
            prns_host_runtime::connections::AttachmentState::Failed => {
                ConnectionAttachmentState::Failed
            }
        }
    }
}
#[uniffi::export]
impl ConnectionProfileClient {
    pub async fn snapshot(&self) -> Result<ConnectionSnapshot, ConnectionError> {
        let value = self
            .client
            .snapshot()
            .await
            .map_err(ConnectionError::from)?;
        Ok(value.into())
    }
    pub async fn replace(
        &self,
        expected: ConnectionGuard,
        profiles: Vec<ConnectionProfile>,
    ) -> Result<ConnectionSnapshot, ConnectionError> {
        let value = self
            .client
            .replace(
                expected.try_into()?,
                profiles
                    .into_iter()
                    .map(|item| Ok(item.try_into()?))
                    .collect::<Result<Vec<_>, ConnectionError>>()?,
            )
            .await
            .map_err(ConnectionError::from)?;
        Ok(value.into())
    }
}
#[uniffi::export]
impl ConnectionBindingClient {
    pub fn status(&self) -> ConnectionBindingStatus {
        let value = self.client.status();
        value.into()
    }
    pub async fn snapshot(&self) -> Result<ConnectionBindingSnapshot, ConnectionError> {
        let value = self
            .client
            .snapshot()
            .await
            .map_err(ConnectionError::from)?;
        Ok(value.into())
    }
    pub async fn retry(
        &self,
        expected: ConnectionGuard,
        profile: Vec<u8>,
    ) -> Result<(), ConnectionError> {
        self.client
            .retry(
                expected.try_into()?,
                prns_host_runtime::connections::ProfileId::new(fixed16(profile)?)
                    .map_err(ConnectionError::from)?,
            )
            .await
            .map_err(ConnectionError::from)?;
        Ok(())
    }
}
#[uniffi::export]
pub fn connection_tcp_client_config(
    host: String,
    port: u16,
    bitrate: crate::transport::Bitrate,
) -> Result<crate::transport::InterfaceConfig, ConnectionError> {
    let value = prns_host_runtime::connections::tcp_client_config(
        &host,
        port,
        bitrate
            .try_into()
            .map_err(|_| invalid(prns_host_runtime::connections::InputError::InvalidConfig))?,
    )
    .map_err(ConnectionError::from)?;
    Ok(value.into())
}
#[uniffi::export]
pub fn connection_tcp_client_config_from_target(
    target: String,
    bitrate: crate::transport::Bitrate,
) -> Result<crate::transport::InterfaceConfig, ConnectionError> {
    let value = prns_host_runtime::connections::tcp_client_config_from_target(
        &target,
        bitrate
            .try_into()
            .map_err(|_| invalid(prns_host_runtime::connections::InputError::InvalidConfig))?,
    )
    .map_err(ConnectionError::from)?;
    Ok(value.into())
}
