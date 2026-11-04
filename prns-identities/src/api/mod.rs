//! The canonical native identity contract, shared by Rust and foreign clients.
//! Only capacity-bearing collections differ from the allocation-free core.
//! Conversions enforce the selected owner's actual bounds; they never truncate.
pub(crate) mod client;
mod error;
pub use client::{Client, IdentityCatalogClient, IdentityClient, IdentityJob, ImportKey, Submit};
pub(crate) use error::owner_error;
mod model;
pub use crate::creation::{CreateOutcome, CreateRequestId, CreateRequestScope};
pub use model::*;
pub use prns_identities_core::{
    ApiError, CommandDigest, CreationFailure, DesiredState, Fault, IdentityGuard,
    IdentityReference, KeyReference, KeySource, LocalIdentityId, Metadata, MetadataGuard,
    OperationResult, OperationState, OperationToken, PendingTransition, PublicIdentityHash,
    RetirementBlock, RuntimeFailure, RuntimeStatus, ServiceDependency, ServiceFailure, ServiceId,
    StorageFailure, SubmissionFailure, TransitionPhase, TransitionTarget,
};

#[cfg(test)]
mod tests;
