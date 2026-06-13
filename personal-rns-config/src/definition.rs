use personal_rns::interfaces::{
    AirtimeDutyCycle, AnnounceBandwidthCap, AnnounceRateLimit, InterfaceDefinitionView,
    InterfaceKind, InterfaceMode,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedInterfaceKind {
    Auto { group: Option<String> },
    TcpClient { target: String },
    TcpServer { listen: String },
    Udp { listen: String, forward: Option<String> },
    Serial { device: String, baud: u32 },
}

impl OwnedInterfaceKind {
    fn as_view(&self) -> InterfaceKind<'_> {
        match self {
            Self::Auto { group } => InterfaceKind::Auto {
                group: group.as_deref(),
            },
            Self::TcpClient { target } => InterfaceKind::TcpClient { target },
            Self::TcpServer { listen } => InterfaceKind::TcpServer { listen },
            Self::Udp { listen, forward } => InterfaceKind::Udp {
                listen,
                forward: forward.as_deref(),
            },
            Self::Serial { device, baud } => InterfaceKind::Serial {
                device,
                baud: *baud,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceDefinition {
    pub name: String,
    pub enabled: bool,
    pub kind: OwnedInterfaceKind,
    pub mode: InterfaceMode,
    pub bitrate_bps: Option<u32>,
    pub hardware_mtu: Option<usize>,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
    pub announce_bandwidth_cap: AnnounceBandwidthCap,
    pub airtime_duty_cycle: Option<AirtimeDutyCycle>,
}

impl InterfaceDefinition {
    pub fn new(name: impl Into<String>, kind: OwnedInterfaceKind) -> Self {
        Self {
            name: name.into(),
            enabled: true,
            kind,
            mode: InterfaceMode::Full,
            bitrate_bps: None,
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
            airtime_duty_cycle: None,
        }
    }

    pub fn as_view(&self) -> InterfaceDefinitionView<'_> {
        InterfaceDefinitionView {
            name: &self.name,
            enabled: self.enabled,
            kind: self.kind.as_view(),
            mode: self.mode,
            bitrate_bps: self.bitrate_bps,
            hardware_mtu: self.hardware_mtu,
            announce_rate_limit: self.announce_rate_limit,
            announce_bandwidth_cap: self.announce_bandwidth_cap,
            airtime_duty_cycle: self.airtime_duty_cycle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_borrows_the_owned_storage_unchanged() {
        let definition = InterfaceDefinition::new(
            "Default Interface",
            OwnedInterfaceKind::TcpClient {
                target: "hub.example.com:4965".to_string(),
            },
        );
        let view = definition.as_view();
        assert_eq!(view.name, "Default Interface");
        assert_eq!(
            view.kind,
            InterfaceKind::TcpClient {
                target: "hub.example.com:4965"
            }
        );
        assert!(view.enabled);
    }

    #[test]
    fn optional_string_params_borrow_through_as_none() {
        let definition = InterfaceDefinition::new(
            "Mesh",
            OwnedInterfaceKind::Udp {
                listen: "0.0.0.0:4242".to_string(),
                forward: None,
            },
        );
        assert_eq!(
            definition.as_view().kind,
            InterfaceKind::Udp {
                listen: "0.0.0.0:4242",
                forward: None
            }
        );
    }
}
