//! The application selects one native image and the canonical domain APIs.
//! Product bootstrap and workflows belong here; domain methods do not.
#![forbid(unsafe_code)]

pub mod policy;
mod remote_control;
pub use policy::application_options;

uniffi::setup_scaffolding!();
pub mod bootstrap;
prns_service_runtime_uniffi::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::runtime::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::host::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::identities::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::identity_core::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::identity::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::identity_runtime::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::group_core::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::group_service::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::group::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::group_store_redb::uniffi_reexport_scaffolding!();
prns_service_runtime_uniffi::identities::contact_card::uniffi_reexport_scaffolding!();

/// Product API semantics. UniFFI separately checks each generated ABI checksum.
/// Bump together with the expected version in the app binding image recipe.
#[uniffi::export]
pub fn app_api_version() -> u32 {
    #[cfg(target_os = "android")]
    prns_expo_android::ensure_linked();
    3
}
