//! One image; every namespace belongs to its authoritative declaration crate.
#![forbid(unsafe_code)]
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
#[cfg(feature = "contact-card")]
prns_service_runtime_uniffi::identities::contact_card::uniffi_reexport_scaffolding!();
