//! Explicit LAN participation for an appliance whose radios are managed by its OS.
use clap::Args;
use personal_rns::interfaces::wifi_auto as contract;
use personal_rns::wifi_auto::{
    native_service_discovery, AutoWifi, AutoWifiDevicePolicy, AutoWifiSettings,
    AutoWifiSettingsError,
};

#[derive(Debug, Args)]
pub struct AutoWifiOptions {
    /// Participate only on these OS network devices (repeat for additional LANs).
    #[arg(long, value_parser = device_name)]
    pub auto_wifi_device: Vec<String>,
}

fn device_name(value: &str) -> Result<String, &'static str> {
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || c == '/' || c == '\0')
    {
        return Err("expected a nonempty OS network device name");
    }
    Ok(value.to_owned())
}

impl AutoWifiOptions {
    pub fn settings(&self) -> Result<Option<AutoWifiSettings>, AutoWifiSettingsError> {
        if self.auto_wifi_device.is_empty() {
            return Ok(None);
        }
        let mut devices = self.auto_wifi_device.clone();
        devices.sort();
        devices.dedup();
        AutoWifiSettings::new(
            contract::GROUP_ID,
            contract::DiscoveryScope::Link,
            contract::MulticastAddressType::Temporary,
            contract::DEFAULT_DISCOVERY_PORT,
            contract::DEFAULT_DATA_PORT,
            AutoWifiDevicePolicy::new(devices, Vec::new()),
        )
        .map(Some)
    }
}

pub fn supervisor(settings: AutoWifiSettings) -> AutoWifi {
    let discovery = native_service_discovery(settings.devices().clone());
    AutoWifi::with_policy_and_settings(contract::configured_policy(Default::default()), settings)
        .with_native_host_discovery(discovery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        options: AutoWifiOptions,
    }

    #[test]
    fn absence_never_enables_all_devices_and_selection_is_exact() {
        let disabled = Cli::try_parse_from(["host"]).unwrap();
        assert!(disabled.options.settings().unwrap().is_none());
        let selected = Cli::try_parse_from([
            "host",
            "--auto-wifi-device",
            "br-lan",
            "--auto-wifi-device",
            "eth0",
            "--auto-wifi-device",
            "br-lan",
        ])
        .unwrap()
        .options
        .settings()
        .unwrap()
        .unwrap();
        assert_eq!(selected.devices().allowed(), &["br-lan", "eth0"]);
        assert!(selected.devices().ignored().is_empty());
        assert_eq!(selected.group_id(), contract::GROUP_ID);
    }

    #[test]
    fn empty_or_combined_device_names_are_refused() {
        for invalid in ["", "br-lan wlan0", "../wlan0", "wlan0\0"] {
            assert!(Cli::try_parse_from(["host", "--auto-wifi-device", invalid]).is_err());
        }
    }
}
