use super::Error;
use crate::groups as assembly;
use assembly::network;
use prns_groups as groups;
use prns_identities::owned;
use std::{num::NonZeroUsize, time::Duration};

/// Whether construction schedules the catalog's already accepted work. This
/// never begins inert reservations or changes the persisted desired state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "ServiceStartupRecovery"))]
pub enum StartupRecovery {
    /// Leave existing identities dormant until an explicit client recovery.
    #[default]
    Dormant,
    /// Recover saved Enabled identities and accepted pending lifecycle work.
    /// Readiness acknowledges scheduling, not completed activation/connectivity.
    RecoverCurrent,
}

/// One selected native owner profile. Numeric representations are portable;
/// validation converts to native widths before any host or filesystem effects.
/// These are construction choices, not a second set of domain operation models.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "ServiceRuntimeOptions"))]
pub struct Options {
    pub startup_recovery: StartupRecovery,
    pub queue_capacity: u32,
    pub identity_budget: prns_identities_runtime::Budget,
    pub cache_bytes_per_identity: u64,
    pub total_cache_bytes: u64,
    pub group_client: groups::ClientLimits,
    /// Resident namespace validation entries per attached identity. This does
    /// not limit persisted group count; undersizing increases cold scan work.
    pub validation_cache_entries: u32,
    pub retention: groups::retained::Limits,
    pub catalog: groups::enrollment::CatalogLimits,
    #[cfg(feature = "archive")]
    pub archive: Option<groups::archive::Limits>,
    pub carrier: CarrierOptions,
    pub sync_policy: groups::SyncPolicy,
    pub announce_interval_millis: Option<u64>,
}

/// Representation adapter for the existing transport's usize/Duration settings.
/// The transport remains responsible for frame, queue, and deadline validation.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "ServiceCarrierOptions"))]
pub struct CarrierOptions {
    pub operations: u32,
    pub outgoing_links: u32,
    pub sync_frame_bytes: u32,
    pub sync_page_items: u32,
    pub sync_peers: u32,
    pub sync_inbound_jobs: u32,
    pub sync_timeout_millis: u64,
    pub enrollment_frame_bytes: u32,
    pub enrollment_inbound_jobs: u32,
    pub enrollment_timeout_millis: u64,
}
fn native_size(value: u64) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Error::InvalidOptions)
}
impl Options {
    pub(super) fn checked(self) -> Result<network::OwnerOptions, Error> {
        let local = crate::local::Options {
            identities: owned::Options {
                queue_capacity: NonZeroUsize::new(native_size(self.queue_capacity.into())?)
                    .ok_or(Error::InvalidOptions)?,
                budget: self.identity_budget,
            },
            stores: crate::stores::Options {
                per_identity: prns_storage::database::Options {
                    cache_bytes: native_size(self.cache_bytes_per_identity)?,
                },
                total_cache_bytes: native_size(self.total_cache_bytes)?,
            },
        };
        let entries = NonZeroUsize::new(native_size(self.validation_cache_entries.into())?)
            .ok_or(Error::InvalidOptions)?;
        let storage =
            assembly::Options::new(local, self.retention, self.catalog, self.group_client)
                .with_validation_cache_entries(entries);
        #[cfg(feature = "archive")]
        let storage = match self.archive {
            Some(limits) => storage.with_archive(limits),
            None => storage,
        };
        let carrier = self.carrier;
        let options = network::OwnerOptions {
            storage,
            service: network::RunOptions {
                carrier: network::Options {
                    operations: native_size(carrier.operations.into())?,
                    outgoing_links: native_size(carrier.outgoing_links.into())?,
                    sync: prns_groups_reticulum::Limits {
                        max_frame_bytes: native_size(carrier.sync_frame_bytes.into())?,
                        max_page_items: native_size(carrier.sync_page_items.into())?,
                        max_peers: native_size(carrier.sync_peers.into())?,
                        inbound_jobs: native_size(carrier.sync_inbound_jobs.into())?,
                        exchange_timeout: Duration::from_millis(carrier.sync_timeout_millis),
                    },
                    enrollment: prns_groups_reticulum::enrollment::Limits {
                        max_frame_bytes: native_size(carrier.enrollment_frame_bytes.into())?,
                        inbound_jobs: native_size(carrier.enrollment_inbound_jobs.into())?,
                        exchange_timeout: Duration::from_millis(carrier.enrollment_timeout_millis),
                    },
                },
                policy: self.sync_policy,
                announce_interval_millis: self.announce_interval_millis,
            },
        };
        options
            .validate::<4, 32, 4, 16>()
            .map_err(|_| Error::InvalidOptions)
    }
}
