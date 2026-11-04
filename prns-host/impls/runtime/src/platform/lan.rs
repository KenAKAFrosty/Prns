//! Automatic LAN construction is replaceable independently of product settings.

use crate::HostAutoLanFactory;

#[cfg(all(target_os = "ios", feature = "apple-platform"))]
static APPLE_MULTICAST_ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Select the signed application's multicast capability before creating a host.
///
/// The application must declare the matching multicast entitlement and use a
/// provisioning profile that grants it. This is configuration, not an entitlement
/// query. The first selection (or host construction) fixes it for this process;
/// repeated startup cannot change the capability of an existing LAN supervisor.
pub fn configure_apple_multicast(enabled: bool) {
    #[cfg(all(target_os = "ios", feature = "apple-platform"))]
    {
        select_apple_multicast(&APPLE_MULTICAST_ENABLED, enabled);
    }
    #[cfg(not(all(target_os = "ios", feature = "apple-platform")))]
    let _ = enabled;
}

#[cfg(any(all(target_os = "ios", feature = "apple-platform"), test))]
fn select_apple_multicast(selection: &std::sync::OnceLock<bool>, enabled: bool) -> bool {
    *selection.get_or_init(|| enabled)
}

#[cfg(all(target_os = "ios", feature = "apple-platform"))]
struct AppleLanStatus {
    generation: u64,
    status: personal_rns::wifi_auto::AutoWifiStatus,
    provider: personal_rns::wifi_auto::ServiceDiscoveryTask,
}

#[cfg(all(target_os = "ios", feature = "apple-platform"))]
fn apple_status() -> &'static std::sync::Mutex<Option<AppleLanStatus>> {
    static STATUS: std::sync::OnceLock<std::sync::Mutex<Option<AppleLanStatus>>> =
        std::sync::OnceLock::new();
    STATUS.get_or_init(|| std::sync::Mutex::new(None))
}

/// Coalesced OS path notification; only the current native LAN generation is touched.
pub fn mobile_auto_lan_network_changed() {
    #[cfg(all(target_os = "ios", feature = "apple-platform"))]
    if let Ok(status) = apple_status().lock() {
        if let Some(current) = status.as_ref() {
            current.status.network_changed();
        }
    }
    #[cfg(all(target_os = "android", feature = "android-platform"))]
    super::android::lan::wake();
}

pub struct MobileAutoLanStatus {
    pub method: &'static str,
    pub stock_multicast_available: bool,
    pub detail: Option<String>,
    pub observed_error: Option<String>,
    pub discovery_ready: Option<bool>,
}

pub fn mobile_auto_lan_status() -> MobileAutoLanStatus {
    #[cfg(target_os = "ios")]
    {
        #[cfg(feature = "apple-platform")]
        let multicast_enabled = APPLE_MULTICAST_ENABLED.get().copied().unwrap_or(false);
        #[cfg(not(feature = "apple-platform"))]
        let multicast_enabled = false;
        #[cfg(feature = "apple-platform")]
        let observed = apple_status().lock().ok().and_then(|current| {
            current.as_ref().and_then(|current| {
                current
                    .status
                    .activation_error()
                    .or_else(|| current.provider.detail())
            })
        });
        #[cfg(not(feature = "apple-platform"))]
        let observed: Option<String> = None;
        #[cfg(feature = "apple-platform")]
        let ready = apple_status().lock().ok().and_then(|current| {
            current.as_ref().map(|current| {
                apple_discovery_ready(
                    multicast_enabled,
                    current.status.multicast_discovery_ready(),
                    current.status.rendezvous_ready(),
                    current.provider.is_active(),
                )
            })
        });
        #[cfg(not(feature = "apple-platform"))]
        let ready: Option<bool> = None;
        return MobileAutoLanStatus {
            method: if multicast_enabled { "MulticastAndBonjour" } else { "Bonjour" },
            stock_multicast_available: multicast_enabled,
            detail: (!multicast_enabled).then(|| "Finds compatible Bonjour peers. Stock Reticulum multicast discovery is not enabled for this app build.".into()),
            observed_error: observed,
            discovery_ready: ready,
        };
    }
    #[cfg(all(target_os = "android", feature = "android-platform"))]
    return MobileAutoLanStatus {
        method: "MulticastAndBonjour",
        stock_multicast_available: true,
        detail: None,
        observed_error: super::android::lan::platform_detail(),
        discovery_ready: super::android::lan::platform_ready(),
    };
    #[cfg(not(any(
        target_os = "ios",
        all(target_os = "android", feature = "android-platform")
    )))]
    MobileAutoLanStatus {
        method: "Native",
        stock_multicast_available: true,
        detail: None,
        observed_error: None,
        discovery_ready: None,
    }
}

#[cfg(any(all(target_os = "ios", feature = "apple-platform"), test))]
fn apple_discovery_ready(
    multicast_enabled: bool,
    multicast_ready: bool,
    rendezvous_ready: bool,
    bonjour_active: bool,
) -> bool {
    (multicast_enabled && multicast_ready) || (rendezvous_ready && bonjour_active)
}

/// iOS multicast requires an explicit capability selection by the signed app.
pub fn mobile_auto_lan_factory() -> Option<HostAutoLanFactory> {
    #[cfg(all(target_os = "ios", feature = "apple-platform"))]
    {
        let multicast_enabled = select_apple_multicast(&APPLE_MULTICAST_ENABLED, false);
        return Some(std::sync::Arc::new(move |handle, config, routing| {
            validate_mobile_settings(config, routing)?;
            let (discovery, owner) = personal_rns::wifi_auto::apple_service_discovery_managed();
            let interface = personal_rns::wifi_auto::AutoWifi::new();
            let interface = if multicast_enabled {
                interface.with_host_discovery(discovery)
            } else {
                interface.with_platform_discovery(discovery)
            };
            static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut status) = apple_status().lock() {
                *status = Some(AppleLanStatus {
                    generation,
                    status: interface.status(),
                    provider: owner.clone(),
                });
            }
            Ok(crate::HostPreparedAttachment::ManagedSupervisor {
                attachment: handle.supervise(interface),
                kind: prns_host::InterfaceKind::AutoLan,
                cleanup: std::sync::Arc::new(move || {
                    let owner = owner.clone();
                    Box::pin(async move {
                        owner.stop().await?;
                        if let Ok(mut status) = apple_status().lock() {
                            if status
                                .as_ref()
                                .is_some_and(|current| current.generation == generation)
                            {
                                *status = None;
                            }
                        }
                        Ok(())
                    })
                }),
            })
        }));
    }
    #[cfg(all(target_os = "android", feature = "android-platform"))]
    return Some(std::sync::Arc::new(|handle, config, routing| {
        validate_mobile_settings(config, routing)?;
        let (interface, session) = super::android::lan::prepare()?;
        Ok(crate::HostPreparedAttachment::ManagedSupervisor {
            attachment: handle.supervise(interface),
            kind: prns_host::InterfaceKind::AutoLan,
            cleanup: std::sync::Arc::new(move || {
                let session = session.clone();
                Box::pin(async move { session.stop().await })
            }),
        })
    }));
    #[cfg(all(target_os = "ios", not(feature = "apple-platform")))]
    return Some(std::sync::Arc::new(|_, _, _| {
        Err(prns_host::CommandFailure::UnsupportedByBackend)
    }));
    #[cfg(not(any(
        target_os = "ios",
        all(target_os = "android", feature = "android-platform")
    )))]
    None
}

#[cfg(any(
    target_os = "ios",
    all(target_os = "android", feature = "android-platform"),
    test
))]
fn validate_mobile_settings(
    config: &prns_host::InterfaceConfig,
    routing: Option<prns_host::InterfaceRoutingPolicy>,
) -> Result<(), prns_host::CommandFailure> {
    use prns_host::{DiscoveryScope, InterfaceConfig, MulticastAddressType};
    if routing.is_some() {
        return Err(prns_host::CommandFailure::UnsupportedByBackend);
    }
    let supported = matches!(config, InterfaceConfig::AutoLan {
        group_id, discovery_scope, discovery_port, data_port, devices, ignored_devices, multicast_address_type,
    } if group_id.as_deref().is_none_or(|group| group == "reticulum")
        && discovery_scope.is_none_or(|scope| scope == DiscoveryScope::Link)
        && discovery_port.is_none_or(|port| port == 29_716)
        && data_port.is_none_or(|port| port == 42_671)
        && devices.is_empty() && ignored_devices.is_empty()
        && multicast_address_type.is_none_or(|kind| kind == MulticastAddressType::Temporary));
    if supported {
        Ok(())
    } else {
        Err(prns_host::CommandFailure::InvalidConfiguration { detail: "Mobile automatic LAN currently supports the stock reticulum group, ports and device selection only".into() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_multicast_selection_cannot_change_after_configuration_or_host_creation() {
        let enabled = std::sync::OnceLock::new();
        assert!(select_apple_multicast(&enabled, true));
        assert!(select_apple_multicast(&enabled, true));
        assert!(select_apple_multicast(&enabled, false));

        let disabled = std::sync::OnceLock::new();
        assert!(!select_apple_multicast(&disabled, false));
        assert!(!select_apple_multicast(&disabled, true));
    }

    #[test]
    fn apple_discovery_lanes_remain_independent() {
        // An entitled build can discover stock peers while Bonjour is unavailable.
        assert!(apple_discovery_ready(true, true, true, false));
        // A failed multicast socket does not take working Bonjour discovery down.
        assert!(apple_discovery_ready(true, false, true, true));
        // An unentitled build cannot claim readiness from the multicast lane.
        assert!(!apple_discovery_ready(false, true, true, false));
        assert!(apple_discovery_ready(false, false, true, true));
        assert!(!apple_discovery_ready(true, false, true, false));
        assert!(!apple_discovery_ready(true, false, false, true));
    }

    #[test]
    fn unsupported_groups_cannot_silently_disable_the_platform_discovery_path() {
        let mut config = prns_host::InterfaceConfig::AutoLan {
            group_id: None,
            discovery_scope: None,
            discovery_port: None,
            data_port: None,
            devices: vec![],
            ignored_devices: vec![],
            multicast_address_type: None,
        };
        assert!(validate_mobile_settings(&config, None).is_ok());
        if let prns_host::InterfaceConfig::AutoLan { group_id, .. } = &mut config {
            *group_id = Some("private".into());
        }
        assert!(validate_mobile_settings(&config, None).is_err());
    }
}
