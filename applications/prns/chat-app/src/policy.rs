//! Explicit product selection over canonical native construction options.
//! These are logical limits, not guarantees about physical storage or radio speed.
use prns_service_runtime_uniffi::{
    group::{archive, enrollment::CatalogLimits, retained, ClientLimits, SyncPolicy},
    identity_runtime::{Budget, Resources},
    runtime::groups::standalone::{CarrierOptions, Options, StartupRecovery},
};

pub(crate) const REMOTE_CONTROL_OPERATIONS: u32 = 16;
const ENROLLMENT_FRAME_BYTES: u32 = 512 * 1024;

/// Select immutable group interoperability bounds before retaining a Create
/// input. The member count is supplied by the user; 256 is a UI starting value,
/// not a global limit. Encoded-size derivation stays in the owning group crate.
#[uniffi::export]
pub fn application_group_profile(
    max_members: u32,
) -> Result<
    prns_service_runtime_uniffi::group::GroupProfile,
    prns_service_runtime_uniffi::group_core::ApiError,
> {
    use prns_service_runtime_uniffi::{group::resolve_group_profile, group_core::ApiError};
    let profile = resolve_group_profile(max_members, 2054, 65_536, 262_144)?;
    if profile.max_enrollment_frame_bytes > ENROLLMENT_FRAME_BYTES {
        return Err(ApiError::InvalidInput);
    }
    Ok(profile)
}

/// Installation defaults are product policy. They are supplied only to strict
/// creation; reopening never reinserts a removed profile or changes saved intent.
pub fn initial_connection_profiles(
) -> Result<Vec<prns_host_runtime::connections::Profile>, prns_host_runtime::connections::Error> {
    use prns_host::{Bitrate, InterfaceConfig};
    use prns_host_runtime::connections::{tcp_client_config, Profile, ProfileId};
    Ok(vec![
        Profile {
            id: ProfileId::generate()?,
            label: "Local network".into(),
            enabled: false,
            config: InterfaceConfig::AutoLan {
                group_id: None,
                discovery_scope: None,
                discovery_port: None,
                data_port: None,
                devices: vec![],
                ignored_devices: vec![],
                multicast_address_type: None,
            },
            routing: None,
        },
        Profile {
            id: ProfileId::generate()?,
            label: "PRNS public node".into(),
            enabled: true,
            config: tcp_client_config("prns.fly.dev", 4242, Bitrate::Auto)?,
            routing: None,
        },
    ])
}

pub fn connection_options() -> prns_host_runtime::connections::Options {
    use prns_host_runtime::connections::{Limits, Options};
    Options {
        limits: Limits {
            profiles: 17,
            encoded_bytes: 32 * 1024,
        },
        queue_capacity: std::num::NonZeroUsize::new(16).unwrap_or(std::num::NonZeroUsize::MIN),
        cache_bytes: 1024 * 1024,
    }
}

/// New installations keep local history until explicit deletion. Full budgets
/// produce visible errors; increasing a delivery deadline is not archival.
/// Existing identities retain their stored group/archive selections on reopen.
/// Startup recovers saved enablement and accepted lifecycle work; readiness
/// acknowledges admission, while identity snapshots report actual activation.
#[uniffi::export]
pub fn application_options() -> Options {
    let queue_capacity = 64;
    let carrier = CarrierOptions {
        operations: 16,
        outgoing_links: 8,
        sync_frame_bytes: 128 * 1024,
        sync_page_items: 64,
        sync_peers: 8,
        sync_inbound_jobs: 8,
        sync_timeout_millis: 30_000,
        enrollment_frame_bytes: ENROLLMENT_FRAME_BYTES,
        enrollment_inbound_jobs: 8,
        enrollment_timeout_millis: 30_000,
    };
    // The concrete native owner supports four independent catalog identities.
    // The adapter derives each activation's actual charge from these same
    // carrier bounds; this budget permits all four plus the installation lane.
    let queues_per_identity =
        carrier.operations + carrier.sync_inbound_jobs + carrier.enrollment_inbound_jobs + 1;
    let management = prns_core::remote_control::RemoteControlStorageRequirements::AVAILABLE;
    let infrastructure = Resources {
        held_keys: u32::try_from(management.held_identities()).unwrap_or(u32::MAX),
        destinations: u32::try_from(management.upstream_app_destinations()).unwrap_or(u32::MAX),
        request_handlers: u32::try_from(management.request_handlers()).unwrap_or(u32::MAX),
        // Bounded management exchanges plus one outgoing and one incoming pairing.
        links: REMOTE_CONTROL_OPERATIONS + 2,
        queue_slots: queue_capacity + REMOTE_CONTROL_OPERATIONS,
    };
    Options {
        startup_recovery: StartupRecovery::RecoverCurrent,
        queue_capacity,
        identity_budget: Budget {
            capacity: Resources {
                held_keys: 4 + infrastructure.held_keys,
                destinations: 8 + infrastructure.destinations,
                request_handlers: 8 + infrastructure.request_handlers,
                links: 4 * carrier.outgoing_links + infrastructure.links,
                queue_slots: 4 * queues_per_identity + infrastructure.queue_slots,
            },
            infrastructure_reserve: infrastructure,
        },
        cache_bytes_per_identity: 8 * 1024 * 1024,
        total_cache_bytes: 32 * 1024 * 1024,
        group_client: ClientLimits {
            max_job_bytes: 1024 * 1024,
            max_pending_job_bytes: 8 * 1024 * 1024,
        },
        // A bounded working set of validated namespaces, independent of how
        // many conversations are stored. Evicted entries are revalidated.
        validation_cache_entries: 1024,
        retention: retained::Limits {
            max_operations: 256,
            max_bytes: 8 * 1024 * 1024,
            ..retained::Limits::default()
        },
        catalog: CatalogLimits {
            max_groups: None,
            max_catalog_bytes: 256 * 1024 * 1024,
            max_enrollment_frame_bytes: carrier.enrollment_frame_bytes,
            max_row_bytes: 1024 * 1024,
            max_transaction_rows: 512,
            max_transaction_bytes: 8 * 1024 * 1024,
            max_maintenance_rows: 32,
            max_incoming_invitations: 4,
            max_incoming_per_peer: 2,
            namespace: prns_service_runtime_uniffi::group_store_redb::RedbLimits {
                max_changes: 512,
                max_key_bytes: 256,
                max_value_bytes: 2 * 1024 * 1024,
                max_staged_bytes: 8 * 1024 * 1024,
                max_live_objects: 65_536,
                max_live_bytes: 256 * 1024 * 1024,
                max_receipts: 16_384,
            },
        },
        archive: Some(archive::Limits {
            max_records: 100_000,
            max_bytes: 256 * 1024 * 1024,
        }),
        carrier,
        sync_policy: SyncPolicy::default(),
        announce_interval_millis: Some(3_600_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_group_profile_fits_selected_local_admission_policies(
    ) -> Result<(), prns_service_runtime_uniffi::group_core::ApiError> {
        let profile = application_group_profile(256)?;
        let options = application_options();
        assert_eq!(options.catalog.max_groups, None);
        assert!(profile.validate().is_ok());
        assert!(options.catalog.validate().is_ok());
        assert!(profile.max_enrollment_frame_bytes <= options.carrier.enrollment_frame_bytes);
        assert!(profile.max_enrollment_frame_bytes <= options.catalog.max_enrollment_frame_bytes);
        assert!(application_group_profile(0).is_err());
        assert!(application_group_profile(u32::MAX).is_err());
        Ok(())
    }
}
