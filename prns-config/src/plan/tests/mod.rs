use std::time::Duration;

use super::interface::RNS_DEFAULT_SERIAL_BAUD;
use super::*;
use crate::reference::keys::interface as interface_key;
use crate::reference::parse;
use crate::ConfigDiagnosticCode;
use prns_core::interface_discovery::{InterfaceDiscoveryPolicy, DEFAULT_STAMP_COST};
use prns_core::interfaces::tcp::TcpWireFraming;
use prns_core::interfaces::IfacSize;
use prns_core::interfaces::{
    AnnounceBandwidthCap, AnnounceRateLimit, BitrateBps, EgressCapability, IngressCapability,
    InterfaceMode,
};
use prns_core::units::DurationMillis;

fn plan_of(config: &str) -> DaemonPlan {
    parse_and_plan(config).expect("config plans").value
}

fn named<'a>(plan: &'a DaemonPlan, name: &str) -> &'a PlannedInterface {
    plan.interfaces
        .iter()
        .find(|interface| interface.name == name)
        .unwrap_or_else(|| panic!("interface '{name}' was planned"))
}

fn tcp_dial(host: &str, port: u16) -> TcpDialPlan {
    TcpDialPlan {
        host: host.to_string(),
        port,
        connect_timeout: ConnectTimeoutSeconds::new(5),
        reconnect_limit: ReconnectLimit::Unlimited,
        address_family: AddressFamilyPreference::System,
        tunnel: TcpTunnelMode::Direct,
    }
}

fn tcp_listener(host: TcpListenHost, port: u16) -> TcpListenPlan {
    TcpListenPlan {
        host,
        port,
        address_family: AddressFamilyPreference::Ipv4,
        tunnel: TcpTunnelMode::Direct,
    }
}

fn udp_address(host: &str, port: u16) -> UdpEndpointPlan {
    UdpEndpointPlan {
        host: UdpEndpointHost::Address(host.to_string()),
        port,
    }
}

fn serial_line_plan(baud: u32) -> SerialLinePlan {
    SerialLinePlan {
        baud,
        data_bits: SerialDataBits::Eight,
        parity: SerialParity::None,
        stop_bits: SerialStopBits::One,
    }
}

const STOCK: &str = "[reticulum]\n\
        enable_transport = Yes\n\
        share_instance = Yes\n\
        [interfaces]\n\
          [[Default Interface]]\n\
            type = AutoInterface\n\
            interface_enabled = Yes\n\
          [[Hub]]\n\
            type = TCPClientInterface\n\
            interface_enabled = Yes\n\
            target_host = hub.example.com\n\
            target_port = 4965\n\
          [[Listener]]\n\
            type = TCPServerInterface\n\
            interface_enabled = Yes\n\
            listen_ip = 0.0.0.0\n\
            listen_port = 4242\n\
          [[Mesh]]\n\
            type = UDPInterface\n\
            interface_enabled = Yes\n\
            listen_ip = 0.0.0.0\n\
            listen_port = 4848\n\
            forward_ip = 255.255.255.255\n\
            forward_port = 4848\n\
          [[Modem]]\n\
            type = SerialInterface\n\
            interface_enabled = Yes\n\
            port = /dev/ttyUSB0\n\
            speed = 115200\n";

mod discovery;
mod medium;
mod node;
mod policy;
