use super::*;
use prns_identities_core as core;

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityDependencyReport"))]
pub struct DependencyReport {
    /// Preserve exact slot placement for reserved-command replay. Trailing empty
    /// slots may be omitted; conversion pads only up to the actual owner bound.
    pub entries: Vec<Option<ServiceDependency>>,
    pub complete: bool,
    pub other_key_uses: u32,
}
impl DependencyReport {
    pub fn to_core<const S: usize>(&self) -> Result<core::DependencyReport<S>, ApiError> {
        Ok(core::DependencyReport::from_slots(
            &self.entries,
            self.complete,
            self.other_key_uses,
        )?)
    }
}
impl<const S: usize> From<core::DependencyReport<S>> for DependencyReport {
    fn from(report: core::DependencyReport<S>) -> Self {
        Self {
            entries: report.entries.into_iter().collect(),
            complete: report.complete,
            other_key_uses: report.other_key_uses,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "IdentityPreparedTransition")
)]
pub struct PreparedTransition {
    pub identity: IdentityGuard,
    pub target: TransitionTarget,
    pub review: DependencyReport,
}
impl PreparedTransition {
    pub fn to_core<const S: usize>(&self) -> Result<core::PreparedTransition<S>, ApiError> {
        Ok(core::PreparedTransition {
            identity: self.identity,
            target: self.target,
            review: self.review.to_core()?,
        })
    }
}
impl<const S: usize> From<core::PreparedTransition<S>> for PreparedTransition {
    fn from(value: core::PreparedTransition<S>) -> Self {
        Self {
            identity: value.identity,
            target: value.target,
            review: value.review.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityCreateRequest"))]
pub struct CreateRequest {
    pub source: KeySource,
    pub metadata: Metadata,
    pub services: Vec<ServiceId>,
}
impl CreateRequest {
    pub fn to_core<const S: usize>(&self) -> Result<core::CreateRequest<S>, ApiError> {
        let request = core::CreateRequest {
            source: self.source,
            metadata: self.metadata,
            services: core::ServiceSet::new(&self.services)?,
        };
        core::validate_create_request(&request)?;
        Ok(request)
    }
}
impl<const S: usize> From<core::CreateRequest<S>> for CreateRequest {
    fn from(value: core::CreateRequest<S>) -> Self {
        Self {
            source: value.source,
            metadata: value.metadata,
            services: value.services.iter().collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityCommand"))]
pub enum Command {
    Create(CreateRequest),
    Transition(PreparedTransition),
}
impl Command {
    pub fn to_core<const S: usize>(&self) -> Result<core::Command<S>, ApiError> {
        Ok(match self {
            Self::Create(value) => core::Command::Create(value.to_core()?),
            Self::Transition(value) => core::Command::Transition(value.to_core()?),
        })
    }
}
impl<const S: usize> From<core::Command<S>> for Command {
    fn from(value: core::Command<S>) -> Self {
        match value {
            core::Command::Create(value) => Self::Create(value.into()),
            core::Command::Transition(value) => Self::Transition(value.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityRecord"))]
pub struct IdentityRecord {
    pub id: LocalIdentityId,
    pub public_hash: PublicIdentityHash,
    pub key_reference: KeyReference,
    pub metadata: Metadata,
    pub metadata_revision: u64,
    pub lifecycle_revision: u64,
    pub desired: DesiredState,
    pub initial_services: Vec<ServiceId>,
    pub services: Vec<ServiceId>,
    pub pending: Option<PendingTransition>,
    pub recovery_cleanup: Option<ServiceFailure>,
}
impl IdentityRecord {
    pub fn reference(&self) -> IdentityReference {
        IdentityReference {
            id: self.id,
            public_hash: self.public_hash,
        }
    }
    pub fn guard(&self) -> IdentityGuard {
        IdentityGuard {
            id: self.id,
            public_hash: self.public_hash,
            lifecycle_revision: self.lifecycle_revision,
        }
    }
    pub fn metadata_guard(&self) -> MetadataGuard {
        MetadataGuard {
            id: self.id,
            public_hash: self.public_hash,
            metadata_revision: self.metadata_revision,
        }
    }
}
impl<const S: usize> From<core::IdentityRecord<S>> for IdentityRecord {
    fn from(value: core::IdentityRecord<S>) -> Self {
        Self {
            id: value.id,
            public_hash: value.public_hash,
            key_reference: value.key_reference,
            metadata: value.metadata,
            metadata_revision: value.metadata_revision,
            lifecycle_revision: value.lifecycle_revision,
            desired: value.desired,
            initial_services: value.initial_services.iter().collect(),
            services: value.services.iter().collect(),
            pending: value.pending,
            recovery_cleanup: value.recovery_cleanup,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityOperationRecord"))]
pub struct OperationRecord {
    pub token: OperationToken,
    pub command: Command,
    pub state: OperationState,
    pub unpublished_key: Option<KeyReference>,
}
impl<const S: usize> From<core::OperationRecord<S>> for OperationRecord {
    fn from(value: core::OperationRecord<S>) -> Self {
        Self {
            token: value.token,
            command: value.command.into(),
            state: value.state,
            unpublished_key: value.unpublished_key,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentitySnapshot"))]
pub struct Snapshot {
    pub identity: IdentityRecord,
    pub runtime: RuntimeStatus,
    pub last_error: Option<ApiError>,
}
