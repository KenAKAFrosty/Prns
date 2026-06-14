use crate::interfaces::{
    hardware_mtu_for_bitrate, AirtimeDutyCycle, AnnounceBandwidthCap, AnnounceRateLimit,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceConfig, InterfaceId,
    InterfaceMode, TransportCapability,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceKind<'a> {
    Auto {
        group: Option<&'a str>,
    },
    TcpClient {
        target: &'a str,
    },
    TcpServer {
        listen: &'a str,
    },
    Udp {
        listen: &'a str,
        forward: Option<&'a str>,
    },
    Serial {
        device: &'a str,
        baud: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDefinitionView<'a> {
    pub name: &'a str,
    pub enabled: bool,
    pub kind: InterfaceKind<'a>,
    pub mode: InterfaceMode,
    pub bitrate_bps: Option<u32>,
    pub hardware_mtu: Option<usize>,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
    pub announce_bandwidth_cap: AnnounceBandwidthCap,
    pub airtime_duty_cycle: Option<AirtimeDutyCycle>,
}

impl<'a> InterfaceDefinitionView<'a> {
    pub const fn new(name: &'a str, kind: InterfaceKind<'a>) -> Self {
        Self {
            name,
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

    pub const fn derived_capabilities(&self) -> InterfaceCapabilities {
        InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        }
    }

    pub fn to_config(&self, id: InterfaceId) -> InterfaceConfig {
        let hardware_mtu = self
            .hardware_mtu
            .or_else(|| self.bitrate_bps.and_then(hardware_mtu_for_bitrate));
        InterfaceConfig {
            id,
            capabilities: self.derived_capabilities(),
            mode: self.mode,
            bitrate_bps: self.bitrate_bps,
            hardware_mtu,
            announce_rate_limit: self.announce_rate_limit,
            announce_bandwidth_cap: self.announce_bandwidth_cap,
            airtime_duty_cycle: self.airtime_duty_cycle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: InterfaceId = InterfaceId::new([0x07; 16]);

    fn tcp_client(target: &str) -> InterfaceDefinitionView<'_> {
        InterfaceDefinitionView::new(target, InterfaceKind::TcpClient { target })
    }

    #[test]
    fn new_carries_the_reference_defaults() {
        let view = tcp_client("hub.example.com:4965");
        assert!(view.enabled);
        assert_eq!(view.mode, InterfaceMode::Full);
        assert_eq!(view.bitrate_bps, None);
        assert_eq!(view.hardware_mtu, None);
        assert_eq!(
            view.announce_bandwidth_cap,
            AnnounceBandwidthCap::RNS_DEFAULT
        );
    }

    #[test]
    fn every_configured_interface_listens_transmits_and_relays() {
        let capabilities = tcp_client("hub:1").derived_capabilities();
        assert_eq!(capabilities.ingress, IngressCapability::Enabled);
        assert_eq!(
            capabilities.egress,
            EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly)
        );
    }

    #[test]
    fn an_unset_mtu_is_materialized_from_the_bitrate_tier() {
        let mut view = tcp_client("hub:1");
        view.bitrate_bps = Some(1_000_000_000);
        assert_eq!(view.to_config(ID).hardware_mtu, Some(524_288));
    }

    #[test]
    fn an_explicit_mtu_outranks_the_bitrate_tier() {
        let mut view = tcp_client("hub:1");
        view.bitrate_bps = Some(1_000_000_000);
        view.hardware_mtu = Some(1_024);
        assert_eq!(view.to_config(ID).hardware_mtu, Some(1_024));
    }

    #[test]
    fn projection_carries_the_assigned_id_and_mode() {
        let mut view = tcp_client("hub:1");
        view.mode = InterfaceMode::Gateway;
        let config = view.to_config(ID);
        assert_eq!(config.id, ID);
        assert_eq!(config.mode, InterfaceMode::Gateway);
    }
}
