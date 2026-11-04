//! MLS operations over one recoverable crypto/application transaction.
//!
//! This crate uses `alloc`; the shared `prns-groups-core` core does not. Internal MLS
//! allocation and skipped-key retention are not bounded by the adapter. Platform
//! heap, stack, entropy, durable backend and physical erasure still require
//! qualification before firmware use.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;
mod adapter;
mod limits;
mod output;
mod policy;
mod provider;

pub use adapter::{
    application_key, Adapter, Admission, ApplicationKey, ApplicationTransaction, Command,
    Committed, CompositionError, Error, MAX_APPLICATION_KEY_BYTES,
};
pub use limits::{Limit, Limits};
pub use output::{Event, Output, OutputContext};
pub use policy::{LocalIdentity, MemberBinding, Policy, PolicyError};
pub use prns_groups_core::{GroupId, OperationId};
pub use prns_groups_store::{CommitReceipt, RecoveryOutcome};
pub use provider::ProviderError;
