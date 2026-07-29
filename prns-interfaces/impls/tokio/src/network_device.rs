#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoWifiDevicePolicy {
    allowed: std::vec::Vec<String>,
    ignored: std::vec::Vec<String>,
}

impl AutoWifiDevicePolicy {
    pub fn new(
        allowed: impl Into<std::vec::Vec<String>>,
        ignored: impl Into<std::vec::Vec<String>>,
    ) -> Self {
        Self {
            allowed: allowed.into(),
            ignored: ignored.into(),
        }
    }

    pub fn allowed(&self) -> &[String] {
        &self.allowed
    }

    pub fn ignored(&self) -> &[String] {
        &self.ignored
    }

    pub(crate) fn allows(&self, name: &str, is_loopback: bool) -> bool {
        if is_loopback || self.ignored.iter().any(|ignored| ignored == name) {
            return false;
        }
        if !self.allowed.is_empty() {
            return self.allowed.iter().any(|allowed| allowed == name);
        }
        !is_virtual(name)
    }
}

impl Default for AutoWifiDevicePolicy {
    fn default() -> Self {
        Self::new(std::vec::Vec::new(), std::vec::Vec::new())
    }
}

fn is_virtual(name: &str) -> bool {
    const VIRTUAL_PREFIXES: [&str; 14] = [
        "utun", "tun", "tap", "ppp", "ipsec", "awdl", "llw", "gif", "stf", "bridge", "vmnet",
        "vnic", "docker", "p2p",
    ];
    VIRTUAL_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::{is_virtual, AutoWifiDevicePolicy};

    #[test]
    fn wifi_direct_group_netdevs_read_as_virtual() {
        assert!(is_virtual("p2p-wlan0-0"));
        assert!(is_virtual("p2p-dev-wlan0"));
        assert!(!is_virtual("wlan0"));
        assert!(!is_virtual("wlp0s20f3"));
    }

    #[test]
    fn configured_device_allow_and_ignore_lists_have_stock_precedence() {
        let default = AutoWifiDevicePolicy::default();
        assert!(!default.allows("awdl0", false));
        assert!(default.allows("en0", false));

        let configured = AutoWifiDevicePolicy::new(
            std::vec![String::from("awdl0"), String::from("en0")],
            std::vec![String::from("en0")],
        );
        assert!(configured.allows("awdl0", false));
        assert!(!configured.allows("en0", false));
        assert!(!configured.allows("wlan0", false));
    }
}
