use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    Bitrate, DiscoveryScope, InterfaceKind, MulticastAddressType, SerialDataBits, SerialParity,
    SerialStopBits, WebSocketFramingSelection,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct SerialLineConfig {
    pub baud: u32,
    pub data_bits: SerialDataBits,
    pub parity: SerialParity,
    pub stop_bits: SerialStopBits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct RNodeRadioConfig {
    pub frequency_hz: u64,
    pub bandwidth_hz: u32,
    pub tx_power_dbm: i16,
    pub spreading_factor: u8,
    pub coding_rate: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct MultiRNodeMemberConfig {
    pub name: String,
    pub virtual_port: u8,
    pub radio: RNodeRadioConfig,
    pub flow_control: bool,
    pub outgoing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub enum InterfaceConfig {
    AutoLan {
        group_id: Option<String>,
        discovery_scope: Option<DiscoveryScope>,
        discovery_port: Option<u16>,
        data_port: Option<u16>,
        devices: Vec<String>,
        ignored_devices: Vec<String>,
        multicast_address_type: Option<MulticastAddressType>,
    },
    TcpClient {
        target: String,
        bitrate: Bitrate,
    },
    TcpServer {
        bind: String,
        bitrate: Bitrate,
    },
    Udp {
        local: String,
        peer: String,
        bitrate: Bitrate,
    },
    Serial {
        port: String,
        line: SerialLineConfig,
    },
    Kiss {
        port: String,
        line: SerialLineConfig,
        flow_control: bool,
        preamble_millis: u32,
        transmit_tail_millis: u32,
        persistence: u8,
        slot_time_millis: u32,
        station_callsign: Option<String>,
        station_interval_seconds: Option<u64>,
    },
    Ax25Kiss {
        port: String,
        line: SerialLineConfig,
        flow_control: bool,
        preamble_millis: u32,
        transmit_tail_millis: u32,
        persistence: u8,
        slot_time_millis: u32,
        callsign: String,
        ssid: u8,
    },
    RNode {
        port: String,
        radio: RNodeRadioConfig,
        flow_control: bool,
        station_callsign: Option<String>,
        station_interval_seconds: Option<u64>,
        airtime_limit_short_centi_percent: Option<u16>,
        airtime_limit_long_centi_percent: Option<u16>,
    },
    MultiRNode {
        port: String,
        station_callsign: Option<String>,
        station_interval_seconds: Option<u64>,
        members: Vec<MultiRNodeMemberConfig>,
    },
    Pipe {
        command: Vec<String>,
        respawn_delay_millis: u64,
    },
    BackboneClient {
        target: String,
        bitrate: Bitrate,
    },
    BackboneServer {
        bind: String,
        bitrate: Bitrate,
    },
    I2p {
        peers: Vec<String>,
        connectable: bool,
    },
    Weave {
        port: String,
    },
    AutomaticUsb,
    AutomaticBluetoothLe,
    ConfiguredBluetoothLe {
        discovery_groups: Vec<String>,
    },
    WebSocketClient {
        target: String,
        framing: WebSocketFramingSelection,
    },
    WebSocketServer {
        bind: String,
        framing: WebSocketFramingSelection,
    },
    BrowserRendezvous {
        url: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfaceConfigError {
    EmptyRequiredValue,
    InvalidPort,
    InvalidSerialBaud,
    InvalidRadio,
    InvalidCallsign,
    InvalidSsid,
    MissingMembers,
    InvalidCommand,
    InvalidWebSocketUrl,
    InvalidDiscoveryGroups,
}

impl InterfaceConfig {
    #[must_use]
    pub const fn kind(&self) -> InterfaceKind {
        match self {
            Self::AutoLan { .. } => InterfaceKind::AutoLan,
            Self::TcpClient { .. } => InterfaceKind::TcpClient,
            Self::TcpServer { .. } => InterfaceKind::TcpServer,
            Self::Udp { .. } => InterfaceKind::Udp,
            Self::Serial { .. } => InterfaceKind::Serial,
            Self::Kiss { .. } => InterfaceKind::Kiss,
            Self::Ax25Kiss { .. } => InterfaceKind::Ax25Kiss,
            Self::RNode { .. } => InterfaceKind::RNode,
            Self::MultiRNode { .. } => InterfaceKind::MultiRNode,
            Self::Pipe { .. } => InterfaceKind::Pipe,
            Self::BackboneClient { .. } => InterfaceKind::BackboneClient,
            Self::BackboneServer { .. } => InterfaceKind::BackboneServer,
            Self::I2p { .. } => InterfaceKind::I2p,
            Self::Weave { .. } => InterfaceKind::Weave,
            Self::AutomaticUsb => InterfaceKind::AutomaticUsb,
            Self::AutomaticBluetoothLe | Self::ConfiguredBluetoothLe { .. } => {
                InterfaceKind::AutomaticBluetoothLe
            }
            Self::WebSocketClient { .. } => InterfaceKind::WebSocketClient,
            Self::WebSocketServer { .. } => InterfaceKind::WebSocketServer,
            Self::BrowserRendezvous { .. } => InterfaceKind::BrowserRendezvous,
        }
    }

    pub fn validate(&self) -> Result<(), InterfaceConfigError> {
        match self {
            Self::AutoLan {
                discovery_port,
                data_port,
                devices,
                ignored_devices,
                ..
            } => {
                if discovery_port.is_some_and(|port| port == 0 || port == u16::MAX)
                    || data_port.is_some_and(|port| port == 0)
                {
                    return Err(InterfaceConfigError::InvalidPort);
                }
                if devices.iter().chain(ignored_devices).any(String::is_empty) {
                    return Err(InterfaceConfigError::EmptyRequiredValue);
                }
            }
            Self::TcpClient { target, .. }
            | Self::TcpServer { bind: target, .. }
            | Self::BackboneClient { target, .. }
            | Self::BackboneServer { bind: target, .. }
            | Self::Weave { port: target } => required(target)?,
            Self::Udp { local, peer, .. } => {
                required(local)?;
                required(peer)?;
            }
            Self::Serial { port, line } => {
                required(port)?;
                validate_line(*line)?;
            }
            Self::Kiss {
                port,
                line,
                station_callsign,
                ..
            } => {
                required(port)?;
                validate_line(*line)?;
                validate_optional_callsign(station_callsign.as_deref())?;
            }
            Self::Ax25Kiss {
                port,
                line,
                callsign,
                ssid,
                ..
            } => {
                required(port)?;
                validate_line(*line)?;
                validate_callsign(callsign)?;
                if *ssid > 15 {
                    return Err(InterfaceConfigError::InvalidSsid);
                }
            }
            Self::RNode {
                port,
                radio,
                station_callsign,
                ..
            } => {
                required(port)?;
                validate_radio(*radio)?;
                validate_optional_callsign(station_callsign.as_deref())?;
            }
            Self::MultiRNode {
                port,
                station_callsign,
                members,
                ..
            } => {
                required(port)?;
                validate_optional_callsign(station_callsign.as_deref())?;
                if members.is_empty() {
                    return Err(InterfaceConfigError::MissingMembers);
                }
                for member in members {
                    required(&member.name)?;
                    validate_radio(member.radio)?;
                }
            }
            Self::Pipe { command, .. } => {
                if command.is_empty() || command.iter().any(String::is_empty) {
                    return Err(InterfaceConfigError::InvalidCommand);
                }
            }
            Self::I2p { peers, .. } => {
                if peers.iter().any(String::is_empty) {
                    return Err(InterfaceConfigError::EmptyRequiredValue);
                }
            }
            Self::AutomaticUsb | Self::AutomaticBluetoothLe => {}
            Self::ConfiguredBluetoothLe { discovery_groups } => {
                bluetooth_discovery_groups(discovery_groups)?;
            }
            Self::WebSocketClient { target, framing: _ }
            | Self::BrowserRendezvous { url: target } => validate_websocket(target)?,
            Self::WebSocketServer { bind, framing: _ } => required(bind)?,
        }
        Ok(())
    }
}

/// Parse the complete configuration using the transport's canonical bounds and ordering.
pub fn bluetooth_discovery_groups(
    names: &[String],
) -> Result<prns_runtime::interfaces::DiscoveryGroupSet, InterfaceConfigError> {
    use prns_runtime::interfaces::{DiscoveryGroupId, DiscoveryGroupSet, MAX_DISCOVERY_GROUPS};
    if names.is_empty() || names.len() > MAX_DISCOVERY_GROUPS {
        return Err(InterfaceConfigError::InvalidDiscoveryGroups);
    }
    let parsed = names
        .iter()
        .map(|name| DiscoveryGroupId::parse(name))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| InterfaceConfigError::InvalidDiscoveryGroups)?;
    DiscoveryGroupSet::try_from_slice(&parsed)
        .map_err(|_| InterfaceConfigError::InvalidDiscoveryGroups)
}

fn required(value: &str) -> Result<(), InterfaceConfigError> {
    if value.is_empty() {
        Err(InterfaceConfigError::EmptyRequiredValue)
    } else {
        Ok(())
    }
}

fn validate_line(line: SerialLineConfig) -> Result<(), InterfaceConfigError> {
    if line.baud == 0 {
        Err(InterfaceConfigError::InvalidSerialBaud)
    } else {
        Ok(())
    }
}

fn validate_radio(radio: RNodeRadioConfig) -> Result<(), InterfaceConfigError> {
    if radio.frequency_hz == 0
        || radio.bandwidth_hz == 0
        || !(5..=12).contains(&radio.spreading_factor)
        || !(5..=8).contains(&radio.coding_rate)
    {
        Err(InterfaceConfigError::InvalidRadio)
    } else {
        Ok(())
    }
}

fn validate_callsign(value: &str) -> Result<(), InterfaceConfigError> {
    if value.is_empty()
        || value.len() > 6
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        Err(InterfaceConfigError::InvalidCallsign)
    } else {
        Ok(())
    }
}

fn validate_optional_callsign(value: Option<&str>) -> Result<(), InterfaceConfigError> {
    match value {
        Some(value) => validate_callsign(value),
        None => Ok(()),
    }
}

fn validate_websocket(value: &str) -> Result<(), InterfaceConfigError> {
    let address = value
        .strip_prefix("ws://")
        .or_else(|| value.strip_prefix("wss://"));
    if address.is_some_and(|address| !address.is_empty()) && !value.chars().any(char::is_whitespace)
    {
        Ok(())
    } else {
        Err(InterfaceConfigError::InvalidWebSocketUrl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_bluetooth_uses_canonical_discovery_groups() {
        let groups = bluetooth_discovery_groups(&["work".into(), "reticulum".into()]).unwrap();
        assert_eq!(
            groups
                .iter()
                .map(|group| group.as_str())
                .collect::<Vec<_>>(),
            vec!["reticulum", "work"]
        );
        for invalid in [
            vec![],
            vec!["".into()],
            vec!["x".repeat(33)],
            vec!["x".into(); 5],
        ] {
            assert_eq!(
                bluetooth_discovery_groups(&invalid),
                Err(InterfaceConfigError::InvalidDiscoveryGroups)
            );
        }
        assert_eq!(
            InterfaceConfig::ConfiguredBluetoothLe {
                discovery_groups: vec!["work".into()]
            }
            .kind(),
            InterfaceKind::AutomaticBluetoothLe
        );
        assert!(InterfaceConfig::AutomaticBluetoothLe.validate().is_ok());
    }

    fn line() -> SerialLineConfig {
        SerialLineConfig {
            baud: 115_200,
            data_bits: SerialDataBits::Eight,
            parity: SerialParity::None,
            stop_bits: SerialStopBits::One,
        }
    }

    fn radio() -> RNodeRadioConfig {
        RNodeRadioConfig {
            frequency_hz: 915_000_000,
            bandwidth_hz: 125_000,
            tx_power_dbm: 17,
            spreading_factor: 7,
            coding_rate: 5,
        }
    }

    #[test]
    fn every_interface_family_has_a_valid_typed_configuration() {
        let cases = vec![
            InterfaceConfig::AutoLan {
                group_id: None,
                discovery_scope: None,
                discovery_port: Some(29_716),
                data_port: Some(42_410),
                devices: vec!["eth0".into()],
                ignored_devices: Vec::new(),
                multicast_address_type: None,
            },
            InterfaceConfig::TcpClient {
                target: "127.0.0.1:1".into(),
                bitrate: Bitrate::Auto,
            },
            InterfaceConfig::TcpServer {
                bind: "127.0.0.1:0".into(),
                bitrate: Bitrate::Auto,
            },
            InterfaceConfig::Udp {
                local: "127.0.0.1:1".into(),
                peer: "127.0.0.1:2".into(),
                bitrate: Bitrate::Auto,
            },
            InterfaceConfig::Serial {
                port: "/dev/ttyS0".into(),
                line: line(),
            },
            InterfaceConfig::Kiss {
                port: "/dev/ttyS0".into(),
                line: line(),
                flow_control: false,
                preamble_millis: 150,
                transmit_tail_millis: 10,
                persistence: 64,
                slot_time_millis: 20,
                station_callsign: Some("N0CALL".into()),
                station_interval_seconds: Some(600),
            },
            InterfaceConfig::Ax25Kiss {
                port: "/dev/ttyS0".into(),
                line: line(),
                flow_control: false,
                preamble_millis: 150,
                transmit_tail_millis: 10,
                persistence: 64,
                slot_time_millis: 20,
                callsign: "N0CALL".into(),
                ssid: 1,
            },
            InterfaceConfig::RNode {
                port: "/dev/ttyACM0".into(),
                radio: radio(),
                flow_control: false,
                station_callsign: None,
                station_interval_seconds: None,
                airtime_limit_short_centi_percent: None,
                airtime_limit_long_centi_percent: None,
            },
            InterfaceConfig::MultiRNode {
                port: "/dev/ttyACM0".into(),
                station_callsign: None,
                station_interval_seconds: None,
                members: vec![MultiRNodeMemberConfig {
                    name: "mesh".into(),
                    virtual_port: 0,
                    radio: radio(),
                    flow_control: false,
                    outgoing: true,
                }],
            },
            InterfaceConfig::Pipe {
                command: vec!["stdio-peer".into()],
                respawn_delay_millis: 1_000,
            },
            InterfaceConfig::BackboneClient {
                target: "127.0.0.1:1".into(),
                bitrate: Bitrate::Auto,
            },
            InterfaceConfig::BackboneServer {
                bind: "127.0.0.1:0".into(),
                bitrate: Bitrate::Auto,
            },
            InterfaceConfig::I2p {
                peers: vec!["example.i2p".into()],
                connectable: true,
            },
            InterfaceConfig::Weave {
                port: "/dev/ttyACM0".into(),
            },
            InterfaceConfig::AutomaticUsb,
            InterfaceConfig::AutomaticBluetoothLe,
            InterfaceConfig::WebSocketClient {
                target: "wss://example.com/prns".into(),
                framing: WebSocketFramingSelection::Auto,
            },
            InterfaceConfig::WebSocketServer {
                bind: "127.0.0.1:0".into(),
                framing: WebSocketFramingSelection::Hdlc,
            },
            InterfaceConfig::BrowserRendezvous {
                url: "ws://localhost:1/prns".into(),
            },
        ];
        assert_eq!(cases.len(), 19);
        for case in cases {
            assert_eq!(case.validate(), Ok(()), "{:?}", case.kind());
            #[cfg(feature = "serde")]
            {
                let encoded = serde_json::to_vec(&case).unwrap();
                let decoded: InterfaceConfig = serde_json::from_slice(&encoded).unwrap();
                assert_eq!(decoded, case);
            }
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn configuration_decode_rejects_unknown_content_without_discarding_it() {
        for encoded in [
            r#"{"FutureTransport": {"target": "localhost:4242"}}"#,
            r#"{"TcpClient": {"target": "localhost:4242", "bitrate": "Auto", "futurePolicy": true}}"#,
            r#"{"Serial": {"port": "tty0", "line": {"baud": 115200, "data_bits": "Eight", "parity": "FutureParity", "stop_bits": "One"}}}"#,
            r#"{"Serial": {"port": "tty0", "line": {"baud": 115200, "data_bits": "Eight", "parity": "None", "stop_bits": "One", "futurePolicy": true}}}"#,
        ] {
            assert!(serde_json::from_str::<InterfaceConfig>(encoded).is_err());
        }
        let policy = crate::InterfaceRoutingPolicy {
            mode: Some(crate::InterfaceMode::Gateway),
            gravity: Some(-3),
            recursive_path_requests: Some(false),
            announces_from_internal: Some(true),
            announces_to_internal: None,
        };
        let encoded = serde_json::to_vec(&policy).unwrap();
        assert_eq!(
            serde_json::from_slice::<crate::InterfaceRoutingPolicy>(&encoded).unwrap(),
            policy
        );
        assert!(
            serde_json::from_str::<crate::InterfaceRoutingPolicy>(r#"{"futurePolicy": true}"#)
                .is_err()
        );
    }

    #[test]
    fn invalid_typed_interface_values_are_rejected_before_attachment() {
        assert_eq!(
            InterfaceConfig::WebSocketClient {
                target: "https://example.com".into(),
                framing: WebSocketFramingSelection::RawPacket,
            }
            .validate(),
            Err(InterfaceConfigError::InvalidWebSocketUrl)
        );
        assert_eq!(
            InterfaceConfig::Ax25Kiss {
                port: "/dev/ttyS0".into(),
                line: line(),
                flow_control: false,
                preamble_millis: 0,
                transmit_tail_millis: 0,
                persistence: 0,
                slot_time_millis: 0,
                callsign: "TOO-LONG".into(),
                ssid: 16
            }
            .validate(),
            Err(InterfaceConfigError::InvalidCallsign)
        );
        assert_eq!(
            InterfaceConfig::MultiRNode {
                port: "/dev/ttyACM0".into(),
                station_callsign: None,
                station_interval_seconds: None,
                members: Vec::new()
            }
            .validate(),
            Err(InterfaceConfigError::MissingMembers)
        );
    }
}
