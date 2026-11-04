//! Platform lifecycle signals for the existing native mobile LAN adapter.
//!
//! These functions configure or invalidate the owning adapter. They neither
//! create a host nor attach an interface, and never grant application authority.

/// Select the application's signed multicast capability before its first mobile
/// LAN factory is constructed. The first selection is retained for the process;
/// this is configuration, not an entitlement check. Other platforms ignore it.
#[uniffi::export]
pub fn configure_apple_multicast(enabled: bool) {
    prns_host_runtime::platform::configure_apple_multicast(enabled);
}

/// Notify the current mobile LAN generation that the platform network changed.
/// Notifications are coalesced; they do not construct or replace an interface.
/// Platforms without the native mobile LAN adapter ignore this signal.
#[uniffi::export]
pub fn mobile_auto_lan_network_changed() {
    prns_host_runtime::platform::mobile_auto_lan_network_changed();
}

/// Validate and canonicalize a complete local discovery selection before a consumer persists it.
#[uniffi::export]
pub fn validate_bluetooth_discovery_groups(
    groups: Vec<String>,
) -> Result<Vec<String>, crate::transport::BindingError> {
    let groups = prns_host::bluetooth_discovery_groups(&groups).map_err(|_| {
        crate::transport::BindingError::InvalidInput {
            field: "discoveryGroups".into(),
            detail: "Choose one to four distinct discovery names, each 1–32 UTF-8 bytes".into(),
        }
    })?;
    Ok(groups
        .iter()
        .map(|group| group.as_str().to_owned())
        .collect())
}
