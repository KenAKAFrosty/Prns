use prns_config::AutoInterfacePlan;

use crate::wifi_auto::{AutoWifi, AutoWifiDevicePolicy, AutoWifiSettings};

use super::{AttachmentResult, InterfaceConstruction};

pub(super) fn stand_up(
    interface_construction: InterfaceConstruction<'_>,
    auto_interface_plan: &AutoInterfacePlan,
) -> AttachmentResult {
    let auto_wifi_settings =
        auto_wifi_settings(&interface_construction.interface.name, auto_interface_plan)?;
    #[cfg(feature = "wifi-auto-mdns")]
    let native_service_discovery = auto_wifi_settings
        .stock_service_discovery_enabled()
        .then(|| crate::wifi_auto::native_service_discovery(auto_wifi_settings.devices().clone()));
    let auto_wifi = AutoWifi::with_policy_and_settings(
        interface_construction.interface.policy,
        auto_wifi_settings,
    );
    #[cfg(feature = "wifi-auto-mdns")]
    let auto_wifi = match native_service_discovery {
        Some(native_service_discovery) => auto_wifi.with_host_discovery(native_service_discovery),
        None => auto_wifi,
    };
    let attached_auto_wifi = interface_construction.attach(auto_wifi);
    Ok(attached_auto_wifi.id())
}

fn auto_wifi_settings(
    interface_name: &str,
    auto_interface_plan: &AutoInterfacePlan,
) -> Result<AutoWifiSettings, crate::wifi_auto::AutoWifiSettingsError> {
    let discovery_group_id = auto_interface_plan.group_id().as_bytes();
    let mut instance_tag = (discovery_group_id.len() as u64).to_be_bytes().to_vec();
    instance_tag.extend_from_slice(discovery_group_id);
    instance_tag.extend_from_slice(interface_name.as_bytes());
    AutoWifiSettings::new(
        discovery_group_id.to_vec(),
        auto_interface_plan.discovery_scope(),
        auto_interface_plan.multicast_address_type(),
        auto_interface_plan.discovery_port().get(),
        auto_interface_plan.data_port().get(),
        AutoWifiDevicePolicy::new(
            auto_interface_plan.devices().allowed().to_vec(),
            auto_interface_plan.devices().ignored().to_vec(),
        ),
    )
    .map(|auto_wifi_settings| auto_wifi_settings.with_instance_tag(instance_tag))
}

#[cfg(test)]
mod tests {
    use prns_config::PlannedMedium;

    use super::auto_wifi_settings;

    #[test]
    fn planned_settings_cross_the_runtime_boundary_without_defaulting() {
        let node_plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Mesh]]\ntype = AutoInterface\nenabled = Yes\ngroup_id = field-mesh\n\
             discovery_scope = organisation\nmulticast_address_type = permanent\ndiscovery_port = 31000\n\
             data_port = 32000\ndevices = en0, wlan0\nignored_devices = wlan0\n",
        )
        .expect("valid AutoInterface configuration")
        .value;
        let PlannedMedium::AutoWifi(auto_interface_plan) = &node_plan.interfaces[0].medium else {
            panic!("AutoInterface medium expected")
        };

        let runtime_settings =
            auto_wifi_settings(&node_plan.interfaces[0].name, auto_interface_plan)
                .expect("typed plan maps to runtime settings");

        assert_eq!(runtime_settings.group_id(), b"field-mesh");
        assert_eq!(
            runtime_settings.discovery_scope(),
            prns_core::interfaces::wifi_auto::DiscoveryScope::Organisation
        );
        assert_eq!(
            runtime_settings.multicast_address_type(),
            prns_core::interfaces::wifi_auto::MulticastAddressType::Permanent
        );
        assert_eq!(runtime_settings.discovery_port(), 31_000);
        assert_eq!(runtime_settings.data_port(), 32_000);
        assert_eq!(runtime_settings.devices().allowed(), &["en0", "wlan0"]);
        assert_eq!(runtime_settings.devices().ignored(), &["wlan0"]);
    }
}
