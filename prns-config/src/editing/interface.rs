use std::collections::BTreeSet;
use std::fmt;

use prns_core::interfaces::rnode::multi::{RadioConfig, RadioConfigError, RadioConfigInput, VPort};

use crate::reference::keys::{common as common_key, interface as interface_key};
use crate::{parse_and_plan_named, ConfigErrors, InterfaceKind};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InterfaceName(String);

impl InterfaceName {
    pub fn new(value: impl Into<String>) -> Result<Self, InterfaceNameError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(InterfaceNameError::Empty);
        }
        if value
            .chars()
            .any(|character| matches!(character, '[' | ']' | '\r' | '\n'))
        {
            return Err(InterfaceNameError::ConfigObjDelimiter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InterfaceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceNameError {
    Empty,
    ConfigObjDelimiter,
}

impl fmt::Display for InterfaceNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "interface name cannot be empty",
            Self::ConfigObjDelimiter => "interface name cannot contain brackets or line separators",
        })
    }
}

impl std::error::Error for InterfaceNameError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InterfaceConfigKey(String);

impl InterfaceConfigKey {
    pub fn new(value: impl Into<String>) -> Result<Self, InterfaceConfigKeyError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(InterfaceConfigKeyError::Empty);
        }
        if value
            .chars()
            .any(|character| matches!(character, '=' | '[' | ']' | '\r' | '\n'))
        {
            return Err(InterfaceConfigKeyError::ConfigObjDelimiter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceConfigKeyError {
    Empty,
    ConfigObjDelimiter,
}

impl fmt::Display for InterfaceConfigKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "interface configuration key cannot be empty",
            Self::ConfigObjDelimiter => {
                "interface configuration key cannot contain ConfigObj delimiters"
            }
        })
    }
}

impl std::error::Error for InterfaceConfigKeyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct InterfaceSettingKey(&'static str);

impl InterfaceSettingKey {
    pub fn parse(value: &str) -> Option<Self> {
        ALL_SETTING_KEYS
            .iter()
            .copied()
            .find(|candidate| *candidate == value)
            .map(Self)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }

    pub fn is_secret(self) -> bool {
        matches!(
            self.0,
            interface_key::PASS_PHRASE | interface_key::PASSPHRASE
        )
    }

    pub fn canonical(self) -> Self {
        match self.0 {
            interface_key::ENABLED => Self(interface_key::INTERFACE_ENABLED),
            interface_key::MODE => Self(interface_key::INTERFACE_MODE),
            interface_key::NETWORKNAME => Self(interface_key::NETWORK_NAME),
            interface_key::PASSPHRASE => Self(interface_key::PASS_PHRASE),
            _ => self,
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self.canonical().0 {
            interface_key::INTERFACE_ENABLED => interface_key::ENABLED_ALIASES,
            interface_key::INTERFACE_MODE => interface_key::MODE_ALIASES,
            interface_key::NETWORK_NAME => interface_key::NETWORK_NAME_ALIASES,
            interface_key::PASS_PHRASE => interface_key::PASSPHRASE_ALIASES,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterfaceSettingValue {
    Bool(bool),
    Unsigned(u64),
    Signed(i64),
    Decimal(f64),
    Text(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceSetting {
    key: InterfaceSettingKey,
    value: InterfaceSettingValue,
}

impl InterfaceSetting {
    pub const fn new(key: InterfaceSettingKey, value: InterfaceSettingValue) -> Self {
        Self { key, value }
    }

    pub const fn key(&self) -> InterfaceSettingKey {
        self.key
    }

    pub fn value(&self) -> &InterfaceSettingValue {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceDefinition {
    name: InterfaceName,
    kind: InterfaceKind,
    enabled: bool,
    settings: Vec<InterfaceSetting>,
    rnode_multi_radios: Vec<RNodeMultiRadioDefinition>,
}

impl InterfaceDefinition {
    pub fn new(
        name: InterfaceName,
        kind: InterfaceKind,
        enabled: bool,
        settings: Vec<InterfaceSetting>,
    ) -> Result<Self, InterfaceDefinitionError> {
        Self::new_with_rnode_multi_radios(name, kind, enabled, settings, Vec::new())
    }

    pub fn new_with_rnode_multi_radios(
        name: InterfaceName,
        kind: InterfaceKind,
        enabled: bool,
        settings: Vec<InterfaceSetting>,
        rnode_multi_radios: Vec<RNodeMultiRadioDefinition>,
    ) -> Result<Self, InterfaceDefinitionError> {
        Self::new_named_with_rnode_multi_radios(
            "<interface definition>",
            name,
            kind,
            enabled,
            settings,
            rnode_multi_radios,
        )
    }

    pub fn new_named_with_rnode_multi_radios(
        source_name: impl Into<String>,
        name: InterfaceName,
        kind: InterfaceKind,
        enabled: bool,
        settings: Vec<InterfaceSetting>,
        rnode_multi_radios: Vec<RNodeMultiRadioDefinition>,
    ) -> Result<Self, InterfaceDefinitionError> {
        let mut keys = BTreeSet::new();
        for setting in &settings {
            if !keys.insert(setting.key) {
                return Err(InterfaceDefinitionError::DuplicateSetting(setting.key));
            }
        }
        if kind != InterfaceKind::RnodeMulti && !rnode_multi_radios.is_empty() {
            return Err(InterfaceDefinitionError::RadiosOnNonRnodeMulti(kind));
        }
        let mut radio_names = BTreeSet::new();
        for radio in &rnode_multi_radios {
            if !radio_names.insert(radio.name.clone()) {
                return Err(InterfaceDefinitionError::DuplicateRadio(radio.name.clone()));
            }
        }
        let candidate = Self {
            name,
            kind,
            enabled,
            settings,
            rnode_multi_radios,
        };
        let validation = candidate.render_with_enabled(true, "\n");
        let document = format!("[interfaces]\n{validation}");
        parse_and_plan_named(source_name, &document).map_err(InterfaceDefinitionError::Invalid)?;
        Ok(candidate)
    }

    pub fn name(&self) -> &InterfaceName {
        &self.name
    }

    pub const fn kind(&self) -> InterfaceKind {
        self.kind
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn settings(&self) -> &[InterfaceSetting] {
        &self.settings
    }

    pub fn rnode_multi_radios(&self) -> &[RNodeMultiRadioDefinition] {
        &self.rnode_multi_radios
    }

    pub(crate) fn render(&self, newline: &str) -> String {
        self.render_with_enabled(self.enabled, newline)
    }

    fn render_with_enabled(&self, enabled: bool, newline: &str) -> String {
        let mut rendered = format!(
            "  [[{}]]{newline}    type = {}{newline}    interface_enabled = {}{newline}",
            self.name,
            self.kind.canonical_name(),
            render_bool(enabled),
        );
        for setting in &self.settings {
            rendered.push_str("    ");
            rendered.push_str(setting.key.as_str());
            rendered.push_str(" = ");
            rendered.push_str(&render_value(&setting.value));
            rendered.push_str(newline);
        }
        for radio in &self.rnode_multi_radios {
            rendered.push_str(&radio.render(newline));
        }
        rendered
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RNodeMultiRadioDefinition {
    name: InterfaceName,
    vport: VPort,
    radio: RadioConfig,
}

impl RNodeMultiRadioDefinition {
    pub fn new(
        name: InterfaceName,
        vport: u8,
        frequency: u64,
        bandwidth: u32,
        txpower: i16,
        spreading_factor: u8,
        coding_rate: u8,
    ) -> Result<Self, RNodeMultiRadioDefinitionError> {
        let vport = VPort::new(vport).ok_or(RNodeMultiRadioDefinitionError::Vport(vport))?;
        let radio = RadioConfig::new(RadioConfigInput {
            frequency_hz: frequency,
            bandwidth_hz: bandwidth,
            tx_power_dbm: txpower,
            spreading_factor,
            coding_rate,
            airtime_limit_short_centi_percent: None,
            airtime_limit_long_centi_percent: None,
        })
        .map_err(RNodeMultiRadioDefinitionError::Radio)?;
        Ok(Self { name, vport, radio })
    }

    pub fn name(&self) -> &InterfaceName {
        &self.name
    }

    pub const fn vport(&self) -> u8 {
        self.vport.get()
    }

    pub const fn frequency(&self) -> u64 {
        self.radio.frequency().hz() as u64
    }

    pub const fn bandwidth(&self) -> u32 {
        self.radio.bandwidth_hz()
    }

    pub const fn txpower(&self) -> i16 {
        self.radio.tx_power_dbm() as i16
    }

    pub const fn spreading_factor(&self) -> u8 {
        self.radio.spreading_factor()
    }

    pub const fn coding_rate(&self) -> u8 {
        self.radio.coding_rate()
    }

    pub(crate) fn render(&self, newline: &str) -> String {
        format!(
            "    [[[{name}]]]{newline}      interface_enabled = Yes{newline}      vport = {vport}{newline}      frequency = {frequency}{newline}      bandwidth = {bandwidth}{newline}      txpower = {txpower}{newline}      spreadingfactor = {spreading_factor}{newline}      codingrate = {coding_rate}{newline}",
            name = self.name,
            vport = self.vport.get(),
            frequency = self.radio.frequency().hz(),
            bandwidth = self.radio.bandwidth_hz(),
            txpower = self.radio.tx_power_dbm(),
            spreading_factor = self.radio.spreading_factor(),
            coding_rate = self.radio.coding_rate(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RNodeMultiRadioDefinitionError {
    Vport(u8),
    Radio(RadioConfigError),
}

impl fmt::Display for RNodeMultiRadioDefinitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vport(value) => write!(
                formatter,
                "RNodeMulti vport {value} is outside 0 through 10"
            ),
            Self::Radio(error) => write!(formatter, "invalid RNodeMulti radio: {error:?}"),
        }
    }
}

impl std::error::Error for RNodeMultiRadioDefinitionError {}

#[derive(Debug)]
pub enum InterfaceDefinitionError {
    DuplicateSetting(InterfaceSettingKey),
    DuplicateRadio(InterfaceName),
    RadiosOnNonRnodeMulti(InterfaceKind),
    Invalid(ConfigErrors),
}

impl fmt::Display for InterfaceDefinitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSetting(key) => {
                write!(
                    formatter,
                    "interface setting {:?} was provided twice",
                    key.as_str()
                )
            }
            Self::DuplicateRadio(name) => {
                write!(formatter, "RNodeMulti radio {name} was provided twice")
            }
            Self::RadiosOnNonRnodeMulti(kind) => write!(
                formatter,
                "RNodeMulti radios do not apply to {}",
                kind.canonical_name()
            ),
            Self::Invalid(errors) => errors.fmt(formatter),
        }
    }
}

impl std::error::Error for InterfaceDefinitionError {}

pub(crate) fn render_bool(value: bool) -> &'static str {
    if value {
        "Yes"
    } else {
        "No"
    }
}

pub(crate) fn render_value(value: &InterfaceSettingValue) -> String {
    match value {
        InterfaceSettingValue::Bool(value) => render_bool(*value).to_string(),
        InterfaceSettingValue::Unsigned(value) => value.to_string(),
        InterfaceSettingValue::Signed(value) => value.to_string(),
        InterfaceSettingValue::Decimal(value) => value.to_string(),
        InterfaceSettingValue::Text(value) => render_text(value),
        InterfaceSettingValue::List(values) => values
            .iter()
            .map(|value| render_text(value))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn render_text(value: &str) -> String {
    let unquoted = !value.is_empty()
        && value.trim() == value
        && !value
            .chars()
            .any(|character| matches!(character, '#' | ',' | '\r' | '\n'));
    if unquoted {
        return value.to_string();
    }
    if !value.contains('"') {
        return format!("\"{value}\"");
    }
    if !value.contains('\'') {
        return format!("'{value}'");
    }
    format!("\"\"\"{value}\"\"\"")
}

pub(super) const ALL_SETTING_KEYS: &[&str] = &[
    interface_key::INTERFACE_ENABLED,
    interface_key::ENABLED,
    interface_key::INTERFACE_MODE,
    interface_key::MODE,
    interface_key::OUTGOING,
    interface_key::BITRATE,
    interface_key::GRAVITY,
    interface_key::ANNOUNCE_CAP,
    interface_key::ANNOUNCE_RATE_TARGET,
    interface_key::ANNOUNCE_RATE_GRACE,
    interface_key::ANNOUNCE_RATE_PENALTY,
    interface_key::NETWORK_NAME,
    interface_key::NETWORKNAME,
    interface_key::PASS_PHRASE,
    interface_key::PASSPHRASE,
    interface_key::IFAC_SIZE,
    interface_key::DISCOVERABLE,
    interface_key::ANNOUNCE_INTERVAL,
    interface_key::DISCOVERY_STAMP_VALUE,
    interface_key::DISCOVERY_NAME,
    interface_key::DISCOVERY_ENCRYPT,
    interface_key::REACHABLE_ON,
    interface_key::PUBLISH_IFAC,
    interface_key::LATITUDE,
    interface_key::LONGITUDE,
    interface_key::HEIGHT,
    interface_key::DISCOVERY_FREQUENCY,
    interface_key::DISCOVERY_BANDWIDTH,
    interface_key::DISCOVERY_MODULATION,
    interface_key::BOOTSTRAP_ONLY,
    interface_key::RECURSIVE_PRS,
    interface_key::ANNOUNCES_FROM_INTERNAL,
    interface_key::ANNOUNCES_TO_INTERNAL,
    interface_key::IGNORE_CONFIG_WARNINGS,
    interface_key::GROUP_ID,
    interface_key::DISCOVERY_SCOPE,
    interface_key::DISCOVERY_PORT,
    interface_key::DATA_PORT,
    interface_key::DEVICES,
    interface_key::IGNORED_DEVICES,
    interface_key::MULTICAST_ADDRESS_TYPE,
    interface_key::TARGET_HOST,
    interface_key::TARGET_PORT,
    interface_key::TARGET,
    interface_key::FRAMING,
    interface_key::KISS_FRAMING,
    interface_key::I2P_TUNNELED,
    interface_key::CONNECT_TIMEOUT,
    interface_key::MAX_RECONNECT_TRIES,
    interface_key::FIXED_MTU,
    interface_key::LISTEN_IP,
    interface_key::LISTEN_PORT,
    interface_key::DEVICE,
    interface_key::PORT,
    interface_key::PREFER_IPV6,
    interface_key::FORWARD_IP,
    interface_key::FORWARD_PORT,
    interface_key::SPEED,
    interface_key::DATABITS,
    interface_key::PARITY,
    interface_key::STOPBITS,
    interface_key::FLOW_CONTROL,
    interface_key::PREAMBLE,
    interface_key::TXTAIL,
    interface_key::PERSISTENCE,
    interface_key::SLOTTIME,
    interface_key::ID_CALLSIGN,
    interface_key::ID_INTERVAL,
    interface_key::CALLSIGN,
    interface_key::SSID,
    interface_key::FREQUENCY,
    interface_key::BANDWIDTH,
    interface_key::SPREADINGFACTOR,
    interface_key::CODINGRATE,
    interface_key::TXPOWER,
    interface_key::AIRTIME_LIMIT_SHORT,
    interface_key::AIRTIME_LIMIT_LONG,
    interface_key::COMMAND,
    interface_key::RESPAWN_DELAY,
    interface_key::REMOTE,
    interface_key::LISTEN_ON,
    interface_key::VPORT,
    interface_key::PEERS,
    interface_key::CONNECTABLE,
    common_key::INGRESS_CONTROL,
    common_key::EGRESS_CONTROL,
    common_key::IC_MAX_HELD_ANNOUNCES,
    common_key::IC_BURST_HOLD,
    common_key::IC_BURST_FREQ_NEW,
    common_key::IC_BURST_FREQ,
    common_key::IC_PR_BURST_FREQ_NEW,
    common_key::IC_PR_BURST_FREQ,
    common_key::EC_PR_FREQ,
    common_key::IC_NEW_TIME,
    common_key::IC_BURST_PENALTY,
    common_key::IC_HELD_RELEASE_INTERVAL,
];
