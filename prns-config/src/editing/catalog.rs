use std::fmt;

use crate::reference::keys::{common as common_key, interface as interface_key};
use crate::InterfaceKind;

use super::interface::ALL_SETTING_KEYS;
use super::{InterfaceSetting, InterfaceSettingKey, InterfaceSettingValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InterfaceSettingCategory {
    Connection,
    Network,
    Discovery,
    Policy,
    Radio,
    Advanced,
}

impl fmt::Display for InterfaceSettingCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connection => "Connection",
            Self::Network => "Network",
            Self::Discovery => "Discovery",
            Self::Policy => "Policy",
            Self::Radio => "Radio",
            Self::Advanced => "Advanced",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceSettingInputKind {
    Boolean,
    Unsigned,
    Signed,
    Decimal,
    Text,
    List,
    Port,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceSettingSpec {
    key: InterfaceSettingKey,
}

impl InterfaceSettingSpec {
    pub const fn key(self) -> InterfaceSettingKey {
        self.key
    }

    pub fn label(self) -> String {
        match self.key.as_str() {
            interface_key::IFAC_SIZE => "IFAC size".to_string(),
            interface_key::ID_CALLSIGN => "ID callsign".to_string(),
            interface_key::ID_INTERVAL => "ID interval".to_string(),
            interface_key::SSID => "SSID".to_string(),
            interface_key::TXPOWER => "TX power".to_string(),
            interface_key::TXTAIL => "TX tail".to_string(),
            common_key::EC_PR_FREQ => "Egress path-request frequency".to_string(),
            key => key
                .split('_')
                .enumerate()
                .map(|(index, word)| {
                    let acronym = match word {
                        "ax25" => Some("AX.25"),
                        "ec" => Some("EC"),
                        "ic" => Some("IC"),
                        "id" => Some("ID"),
                        "ifac" => Some("IFAC"),
                        "ip" => Some("IP"),
                        "mtu" => Some("MTU"),
                        "pr" => Some("PR"),
                        "prs" => Some("PRs"),
                        "ssid" => Some("SSID"),
                        "tcp" => Some("TCP"),
                        "tx" => Some("TX"),
                        "udp" => Some("UDP"),
                        _ => None,
                    };
                    if let Some(acronym) = acronym {
                        return acronym.to_string();
                    }
                    if index == 0 {
                        let mut characters = word.chars();
                        match characters.next() {
                            Some(first) => {
                                first.to_uppercase().collect::<String>() + characters.as_str()
                            }
                            None => String::new(),
                        }
                    } else {
                        word.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    pub fn category(self) -> InterfaceSettingCategory {
        match self.key.as_str() {
            interface_key::TARGET_HOST
            | interface_key::TARGET_PORT
            | interface_key::TARGET
            | interface_key::LISTEN_IP
            | interface_key::LISTEN_PORT
            | interface_key::FORWARD_IP
            | interface_key::FORWARD_PORT
            | interface_key::DEVICE
            | interface_key::PORT
            | interface_key::REMOTE
            | interface_key::LISTEN_ON
            | interface_key::PEERS
            | interface_key::CONNECTABLE
            | interface_key::CONNECT_TIMEOUT
            | interface_key::MAX_RECONNECT_TRIES
            | interface_key::PREFER_IPV6 => InterfaceSettingCategory::Connection,
            interface_key::DISCOVERABLE
            | interface_key::ANNOUNCE_INTERVAL
            | interface_key::DISCOVERY_STAMP_VALUE
            | interface_key::DISCOVERY_NAME
            | interface_key::DISCOVERY_ENCRYPT
            | interface_key::REACHABLE_ON
            | interface_key::PUBLISH_IFAC
            | interface_key::LATITUDE
            | interface_key::LONGITUDE
            | interface_key::HEIGHT
            | interface_key::DISCOVERY_FREQUENCY
            | interface_key::DISCOVERY_BANDWIDTH
            | interface_key::DISCOVERY_MODULATION => InterfaceSettingCategory::Discovery,
            interface_key::INTERFACE_MODE
            | interface_key::OUTGOING
            | interface_key::ANNOUNCE_CAP
            | interface_key::ANNOUNCE_RATE_TARGET
            | interface_key::ANNOUNCE_RATE_GRACE
            | interface_key::ANNOUNCE_RATE_PENALTY
            | interface_key::BOOTSTRAP_ONLY
            | interface_key::RECURSIVE_PRS
            | interface_key::ANNOUNCES_FROM_INTERNAL
            | interface_key::IGNORE_CONFIG_WARNINGS
            | common_key::INGRESS_CONTROL
            | common_key::EGRESS_CONTROL
            | common_key::IC_MAX_HELD_ANNOUNCES
            | common_key::IC_BURST_HOLD
            | common_key::IC_BURST_FREQ_NEW
            | common_key::IC_BURST_FREQ
            | common_key::IC_PR_BURST_FREQ_NEW
            | common_key::IC_PR_BURST_FREQ
            | common_key::EC_PR_FREQ
            | common_key::IC_NEW_TIME
            | common_key::IC_BURST_PENALTY
            | common_key::IC_HELD_RELEASE_INTERVAL => InterfaceSettingCategory::Policy,
            interface_key::SPEED
            | interface_key::DATABITS
            | interface_key::PARITY
            | interface_key::STOPBITS
            | interface_key::FLOW_CONTROL
            | interface_key::PREAMBLE
            | interface_key::TXTAIL
            | interface_key::PERSISTENCE
            | interface_key::SLOTTIME
            | interface_key::ID_CALLSIGN
            | interface_key::ID_INTERVAL
            | interface_key::CALLSIGN
            | interface_key::SSID
            | interface_key::FREQUENCY
            | interface_key::BANDWIDTH
            | interface_key::SPREADINGFACTOR
            | interface_key::CODINGRATE
            | interface_key::TXPOWER
            | interface_key::AIRTIME_LIMIT_SHORT
            | interface_key::AIRTIME_LIMIT_LONG => InterfaceSettingCategory::Radio,
            interface_key::BITRATE
            | interface_key::NETWORK_NAME
            | interface_key::PASS_PHRASE
            | interface_key::IFAC_SIZE
            | interface_key::GROUP_ID
            | interface_key::DISCOVERY_SCOPE
            | interface_key::DISCOVERY_PORT
            | interface_key::DATA_PORT
            | interface_key::DEVICES
            | interface_key::IGNORED_DEVICES
            | interface_key::MULTICAST_ADDRESS_TYPE
            | interface_key::FIXED_MTU => InterfaceSettingCategory::Network,
            _ => InterfaceSettingCategory::Advanced,
        }
    }

    pub fn input_kind(self, kind: InterfaceKind) -> InterfaceSettingInputKind {
        match self.key.as_str() {
            interface_key::OUTGOING
            | interface_key::DISCOVERABLE
            | interface_key::DISCOVERY_ENCRYPT
            | interface_key::PUBLISH_IFAC
            | interface_key::BOOTSTRAP_ONLY
            | interface_key::RECURSIVE_PRS
            | interface_key::ANNOUNCES_FROM_INTERNAL
            | interface_key::IGNORE_CONFIG_WARNINGS
            | interface_key::KISS_FRAMING
            | interface_key::I2P_TUNNELED
            | interface_key::PREFER_IPV6
            | interface_key::FLOW_CONTROL
            | interface_key::CONNECTABLE
            | common_key::INGRESS_CONTROL
            | common_key::EGRESS_CONTROL => InterfaceSettingInputKind::Boolean,
            interface_key::BITRATE
            | interface_key::ANNOUNCE_RATE_TARGET
            | interface_key::ANNOUNCE_RATE_GRACE
            | interface_key::ANNOUNCE_RATE_PENALTY
            | interface_key::IFAC_SIZE
            | interface_key::DISCOVERY_STAMP_VALUE
            | interface_key::DISCOVERY_FREQUENCY
            | interface_key::DISCOVERY_BANDWIDTH
            | interface_key::DISCOVERY_PORT
            | interface_key::DATA_PORT
            | interface_key::TARGET_PORT
            | interface_key::LISTEN_PORT
            | interface_key::FORWARD_PORT
            | interface_key::CONNECT_TIMEOUT
            | interface_key::MAX_RECONNECT_TRIES
            | interface_key::FIXED_MTU
            | interface_key::SPEED
            | interface_key::DATABITS
            | interface_key::STOPBITS
            | interface_key::PREAMBLE
            | interface_key::TXTAIL
            | interface_key::PERSISTENCE
            | interface_key::SLOTTIME
            | interface_key::ID_INTERVAL
            | interface_key::SSID
            | interface_key::FREQUENCY
            | interface_key::BANDWIDTH
            | interface_key::SPREADINGFACTOR
            | interface_key::CODINGRATE => InterfaceSettingInputKind::Unsigned,
            interface_key::ANNOUNCE_INTERVAL
            | interface_key::TXPOWER
            | common_key::IC_MAX_HELD_ANNOUNCES => InterfaceSettingInputKind::Signed,
            interface_key::ANNOUNCE_CAP
            | interface_key::LATITUDE
            | interface_key::LONGITUDE
            | interface_key::HEIGHT
            | interface_key::AIRTIME_LIMIT_SHORT
            | interface_key::AIRTIME_LIMIT_LONG
            | interface_key::RESPAWN_DELAY
            | common_key::IC_BURST_HOLD
            | common_key::IC_BURST_FREQ_NEW
            | common_key::IC_BURST_FREQ
            | common_key::IC_PR_BURST_FREQ_NEW
            | common_key::IC_PR_BURST_FREQ
            | common_key::EC_PR_FREQ
            | common_key::IC_NEW_TIME
            | common_key::IC_BURST_PENALTY
            | common_key::IC_HELD_RELEASE_INTERVAL => InterfaceSettingInputKind::Decimal,
            interface_key::DEVICES | interface_key::IGNORED_DEVICES | interface_key::PEERS => {
                InterfaceSettingInputKind::List
            }
            interface_key::PORT
                if matches!(
                    kind,
                    InterfaceKind::TcpServer
                        | InterfaceKind::Udp
                        | InterfaceKind::Backbone
                        | InterfaceKind::BackboneClient
                        | InterfaceKind::PrnsWebSocketServer
                ) =>
            {
                InterfaceSettingInputKind::Port
            }
            _ => InterfaceSettingInputKind::Text,
        }
    }

    pub fn accepted(self, kind: InterfaceKind) -> &'static str {
        match self.key.as_str() {
            interface_key::INTERFACE_MODE => {
                "full, access_point, pointtopoint, roaming, boundary, gateway, or internal"
            }
            interface_key::PARITY => "none, even, or odd",
            _ => match self.input_kind(kind) {
                InterfaceSettingInputKind::Boolean => "yes or no",
                InterfaceSettingInputKind::Unsigned => "a non-negative whole number",
                InterfaceSettingInputKind::Signed => "a whole number",
                InterfaceSettingInputKind::Decimal => "a number",
                InterfaceSettingInputKind::Text => "text",
                InterfaceSettingInputKind::List => "a comma-separated list",
                InterfaceSettingInputKind::Port => "a port from 0 through 65535",
            },
        }
    }

    pub fn parse(
        self,
        kind: InterfaceKind,
        input: &str,
    ) -> Result<InterfaceSetting, InterfaceSettingInputError> {
        let value = match self.input_kind(kind) {
            InterfaceSettingInputKind::Boolean => parse_bool(input)
                .map(InterfaceSettingValue::Bool)
                .ok_or(InterfaceSettingInputError::Boolean)?,
            InterfaceSettingInputKind::Unsigned => InterfaceSettingValue::Unsigned(
                cleaned_number(input)
                    .parse()
                    .map_err(|_| InterfaceSettingInputError::Unsigned)?,
            ),
            InterfaceSettingInputKind::Signed => InterfaceSettingValue::Signed(
                cleaned_number(input)
                    .parse()
                    .map_err(|_| InterfaceSettingInputError::Signed)?,
            ),
            InterfaceSettingInputKind::Decimal => {
                let value = cleaned_number(input)
                    .parse::<f64>()
                    .map_err(|_| InterfaceSettingInputError::Decimal)?;
                if !value.is_finite() {
                    return Err(InterfaceSettingInputError::Decimal);
                }
                InterfaceSettingValue::Decimal(value)
            }
            InterfaceSettingInputKind::Text => InterfaceSettingValue::Text(input.to_string()),
            InterfaceSettingInputKind::List => {
                let values = input
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                if values.is_empty() {
                    return Err(InterfaceSettingInputError::List);
                }
                InterfaceSettingValue::List(values)
            }
            InterfaceSettingInputKind::Port => InterfaceSettingValue::Unsigned(
                input
                    .trim()
                    .parse::<u16>()
                    .map(u64::from)
                    .map_err(|_| InterfaceSettingInputError::Port)?,
            ),
        };
        Ok(InterfaceSetting::new(self.key, value))
    }

    pub fn is_secret(self) -> bool {
        self.key.is_secret()
    }
}

impl InterfaceKind {
    pub fn setting_specs(self) -> Vec<InterfaceSettingSpec> {
        let mut specs = Vec::new();
        for key in ALL_SETTING_KEYS {
            let Some(key) = InterfaceSettingKey::parse(key) else {
                continue;
            };
            let canonical = key.canonical();
            if canonical != key
                || matches!(
                    canonical.as_str(),
                    interface_key::TYPE | interface_key::INTERFACE_ENABLED | interface_key::VPORT
                )
                || !self.accepts_setting(canonical.as_str())
                || specs
                    .iter()
                    .any(|spec: &InterfaceSettingSpec| spec.key == canonical)
            {
                continue;
            }
            specs.push(InterfaceSettingSpec { key: canonical });
        }
        specs.sort_by_key(|spec| (spec.category(), spec.key.as_str()));
        specs
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredInterfaceSetting {
    spec: InterfaceSettingSpec,
    source_key: String,
    value: String,
}

impl ConfiguredInterfaceSetting {
    pub(crate) fn new(spec: InterfaceSettingSpec, source_key: String, value: String) -> Self {
        Self {
            spec,
            source_key,
            value,
        }
    }

    pub const fn spec(&self) -> InterfaceSettingSpec {
        self.spec
    }

    pub fn source_key(&self) -> &str {
        &self.source_key
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceSettingInputError {
    Boolean,
    Unsigned,
    Signed,
    Decimal,
    List,
    Port,
}

impl fmt::Display for InterfaceSettingInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Boolean => "enter yes or no",
            Self::Unsigned => "enter a non-negative whole number",
            Self::Signed => "enter a whole number",
            Self::Decimal => "enter a finite number",
            Self::List => "enter at least one comma-separated value",
            Self::Port => "enter a port from 0 through 65535",
        })
    }
}

impl std::error::Error for InterfaceSettingInputError {}

fn parse_bool(input: &str) -> Option<bool> {
    match input.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Some(true),
        "no" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn cleaned_number(input: &str) -> String {
    input
        .trim()
        .chars()
        .filter(|character| *character != '_')
        .collect()
}
