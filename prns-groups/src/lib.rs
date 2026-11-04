//! Ownership and scheduling for generic group records and enrollment.
//! Local operations require no network host or ambient async runtime. Optional
//! networking borrows a supplied host and executor; text is a separate view.
#![forbid(unsafe_code)]

#[cfg(feature = "uniffi-bindings")]
uniffi::setup_scaffolding!();
#[cfg(feature = "uniffi-bindings")]
mod bindings;

#[cfg(feature = "network")]
mod announces;
mod api;
pub use api::*;
mod attachment;
mod client;
pub use attachment::Attachment;
mod dependencies;
pub use dependencies::{Dependencies, DependencyScan, MAX_DEPENDENCY_NAMESPACES_PER_STEP};
mod observation;
pub use observation::{GroupObservation, MAX_OBSERVATIONS};
pub mod admission;
#[cfg(feature = "archive")]
pub mod archive;
#[cfg(not(feature = "archive"))]
mod archive;
pub use archive::selection_in as archive_selection_in;
mod clock;
mod commit;
#[cfg(feature = "network")]
mod delivery;
#[cfg(feature = "network")]
mod diagnostics;
pub mod enrollment;
pub mod fixture;
#[cfg(feature = "network")]
mod hub;
mod manager;
pub mod owned;
mod page;
pub use page::PageLimits;
pub mod profile;
pub use profile::{default_group_profile, resolve_group_profile, GroupProfile, ProfileSupport};
#[cfg(feature = "network")]
mod progress;
mod records;
pub mod retained;
#[cfg(feature = "network")]
mod runner;
pub use prns_groups_core::sync::{SyncIssue, SyncPhase, SyncPolicy};
#[cfg(feature = "network")]
pub use progress::{GroupSyncProgress, PeerSyncProgress, SyncProgressSource};
pub use records::*;
#[cfg(feature = "network")]
mod schedule;
mod session;
pub use manager::GroupManager;
#[cfg(all(test, feature = "text"))]
mod test_support;
mod types;

#[cfg(feature = "network")]
pub use announces::{
    announce_control, AnnounceControl, AnnounceOutcome, AnnounceRequests,
    MANUAL_ANNOUNCE_COOLDOWN_MILLIS,
};
pub use client::{Client, ClientLimits, InputBudget, Job, Submit};
pub use clock::{Clock, SystemClock};
#[cfg(feature = "network")]
pub use diagnostics::{enable_metadata_diagnostics, metadata_schedule_snapshot};
#[cfg(feature = "network")]
pub use hub::{
    run_multi, run_multi_scoped_with_announcements, run_multi_with_announcements,
    run_multi_with_policy,
};
pub use prns_groups_core::retry::{RequestToken, RetryError};
pub use prns_host::IdentityHash;
#[cfg(feature = "network")]
pub use runner::{run, RunnerControl, RunnerOptions};
pub use session::StoreSession;
pub use types::*;

/// Mint an application retry token using platform entropy and wall-clock time.
/// Persist/reuse the returned ID with unchanged input after an ambiguous result.
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn create_request_id() -> Result<RequestId, ApiError> {
    let prns_groups_core::ClockReading::Trusted(now) = SystemClock.now() else {
        return Err(ApiError::Clock);
    };
    let mut entropy = [0; 20];
    getrandom::getrandom(&mut entropy).map_err(|_| ApiError::Unavailable)?;
    RequestToken::new(now.lower().0, entropy)
        .map(RequestId::from)
        .map_err(|reason| ApiError::Retry { reason })
}
