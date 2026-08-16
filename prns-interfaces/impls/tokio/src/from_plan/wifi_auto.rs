use prns_config::AutoInterfacePlan;

use crate::wifi_auto::{AutoWifi, AutoWifiDevicePolicy, AutoWifiSettings};

use super::{AttachmentResult, InterfaceConstruction};

pub(super) fn stand_up(
    construction: InterfaceConstruction<'_>,
    planned: &AutoInterfacePlan,
    mdns_find: bool,
) -> AttachmentResult {
    let mut settings = settings(&construction.interface.name, planned)?;
    if mdns_find {
        settings = settings.with_mdns_find();
    }
    let wifi = AutoWifi::with_policy_and_settings(construction.interface.policy, settings);
    let attached = construction.attach(wifi);
    Ok(attached.id())
}

fn settings(
    interface_name: &str,
    planned: &AutoInterfacePlan,
) -> Result<AutoWifiSettings, crate::wifi_auto::AutoWifiSettingsError> {
    let group_id = planned.group_id().as_bytes();
    let mut instance_tag = (group_id.len() as u64).to_be_bytes().to_vec();
    instance_tag.extend_from_slice(group_id);
    instance_tag.extend_from_slice(interface_name.as_bytes());
    AutoWifiSettings::new(
        group_id.to_vec(),
        planned.discovery_scope(),
        planned.multicast_address_type(),
        planned.discovery_port().get(),
        planned.data_port().get(),
        AutoWifiDevicePolicy::new(
            planned.devices().allowed().to_vec(),
            planned.devices().ignored().to_vec(),
        ),
    )
    .map(|settings| settings.with_instance_tag(instance_tag))
}

#[cfg(test)]
mod tests {
    use prns_config::PlannedMedium;

    use super::settings;

    #[test]
    fn planned_settings_cross_the_runtime_boundary_without_defaulting() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Mesh]]\ntype = AutoInterface\nenabled = Yes\ngroup_id = field-mesh\n\
             discovery_scope = organisation\nmulticast_address_type = permanent\ndiscovery_port = 31000\n\
             data_port = 32000\ndevices = en0, wlan0\nignored_devices = wlan0\nmdns_find = Yes\n",
        )
        .expect("valid AutoInterface configuration")
        .value;
        let PlannedMedium::AutoWifi(planned) = &plan.interfaces[0].medium else {
            panic!("AutoInterface medium expected")
        };

        let settings = settings(&plan.interfaces[0].name, planned)
            .expect("typed plan maps to runtime settings");

        assert_eq!(settings.group_id(), b"field-mesh");
        assert_eq!(
            settings.discovery_scope(),
            prns_core::interfaces::wifi_auto::DiscoveryScope::Organisation
        );
        assert_eq!(
            settings.multicast_address_type(),
            prns_core::interfaces::wifi_auto::MulticastAddressType::Permanent
        );
        assert_eq!(settings.discovery_port(), 31_000);
        assert_eq!(settings.data_port(), 32_000);
        assert_eq!(settings.devices().allowed(), &["en0", "wlan0"]);
        assert_eq!(settings.devices().ignored(), &["wlan0"]);
        assert!(planned.mdns_find());
    }
}
