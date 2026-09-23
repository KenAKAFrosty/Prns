//! Default image for standalone Expo/RN applications.
//! Native compositions depend on prns-host-uniffi directly and replace this image.
pub use prns_host_uniffi::*;
uniffi::setup_scaffolding!();

// Referencing the metadata-bearing SDK exports prevents dead stripping of the namespace.
#[uniffi::export]
pub fn host_binding_contract() -> prns_host_uniffi::HostBindingContract {
    #[cfg(target_os = "android")]
    prns_expo_android::ensure_linked();
    prns_host_uniffi::binding_contract()
}
