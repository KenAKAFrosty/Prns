use std::fmt;

use prns_core::interfaces::{
    AnnounceBandwidthCap, EgressCapability, InterfaceMode, RecursivePathRequestPolicy,
};

use crate::reference::{
    announce_rate_target_is_explicit_off,
    keys::{common as common_key, interface as interface_key},
};
use crate::{
    ConfiguredInterfaceLifecycle, DiscoveryAdvertisementPlan, DiscoveryEncryption,
    DiscoveryIfacPublication, InterfaceAccessPlan, InterfaceDiscoveryPlan, InterfaceKind,
    PlannedInterface, PlannedMedium,
};

use super::interface::ALL_SETTING_KEYS;
use super::{InterfaceSetting, InterfaceSettingKey, InterfaceSettingValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InterfaceSettingCategory {
    Connectivity,
    Access,
    Behavior,
    Discovery,
    Announcements,
    Radio,
    TrafficControl,
    Advanced,
}

impl fmt::Display for InterfaceSettingCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connectivity => "Connectivity",
            Self::Access => "Network access",
            Self::Behavior => "Interface behavior",
            Self::Discovery => "Discovery publication",
            Self::Announcements => "Announcement limits",
            Self::Radio => "Radio",
            Self::TrafficControl => "Advanced traffic control",
            Self::Advanced => "Advanced",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceSettingTier {
    Standard,
    Advanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceSettingCondition {
    IfacEnabled,
    Discoverable,
    DiscoverableKiss,
    AnnounceRateLimit,
    IngressControl,
    EgressControl,
    KissFraming,
}

impl fmt::Display for InterfaceSettingCondition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IfacEnabled => "network name or pass phrase must be configured",
            Self::Discoverable => "Discoverable must be Yes",
            Self::DiscoverableKiss => "Discoverable and KISS framing must both be Yes",
            Self::AnnounceRateLimit => {
                "an interface or transport announcement-rate target must be active"
            }
            Self::IngressControl => "Ingress control must be Yes",
            Self::EgressControl => "Egress control must be Yes",
            Self::KissFraming => "KISS framing must be Yes",
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
            interface_key::OUTGOING => "Outgoing traffic allowed".to_string(),
            interface_key::IFAC_SIZE => "IFAC size".to_string(),
            interface_key::ID_CALLSIGN => "ID callsign".to_string(),
            interface_key::ID_INTERVAL => "ID interval".to_string(),
            interface_key::SSID => "SSID".to_string(),
            interface_key::TXPOWER => "Transmit power".to_string(),
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
            | interface_key::FRAMING
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
            | interface_key::PREFER_IPV6
            | interface_key::GROUP_ID
            | interface_key::DISCOVERY_SCOPE
            | interface_key::DISCOVERY_PORT
            | interface_key::DATA_PORT
            | interface_key::DEVICES
            | interface_key::IGNORED_DEVICES
            | interface_key::MULTICAST_ADDRESS_TYPE => InterfaceSettingCategory::Connectivity,
            interface_key::NETWORK_NAME | interface_key::PASS_PHRASE | interface_key::IFAC_SIZE => {
                InterfaceSettingCategory::Access
            }
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
            | interface_key::BITRATE
            | interface_key::GRAVITY
            | interface_key::BOOTSTRAP_ONLY
            | interface_key::RECURSIVE_PRS
            | interface_key::ANNOUNCES_FROM_INTERNAL
            | interface_key::ANNOUNCES_TO_INTERNAL => InterfaceSettingCategory::Behavior,
            interface_key::ANNOUNCE_CAP
            | interface_key::ANNOUNCE_RATE_TARGET
            | interface_key::ANNOUNCE_RATE_GRACE
            | interface_key::ANNOUNCE_RATE_PENALTY => InterfaceSettingCategory::Announcements,
            common_key::INGRESS_CONTROL
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
            | common_key::IC_HELD_RELEASE_INTERVAL => InterfaceSettingCategory::TrafficControl,
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
            interface_key::FIXED_MTU | interface_key::IGNORE_CONFIG_WARNINGS => {
                InterfaceSettingCategory::Advanced
            }
            _ => InterfaceSettingCategory::Advanced,
        }
    }

    pub fn tier(self) -> InterfaceSettingTier {
        match self.category() {
            InterfaceSettingCategory::Connectivity | InterfaceSettingCategory::Access => {
                InterfaceSettingTier::Standard
            }
            InterfaceSettingCategory::Behavior
                if matches!(
                    self.key.as_str(),
                    interface_key::INTERFACE_MODE | interface_key::OUTGOING
                ) =>
            {
                InterfaceSettingTier::Standard
            }
            InterfaceSettingCategory::Discovery
                if self.key.as_str() == interface_key::DISCOVERABLE =>
            {
                InterfaceSettingTier::Standard
            }
            InterfaceSettingCategory::Radio
                if !matches!(
                    self.key.as_str(),
                    interface_key::AIRTIME_LIMIT_SHORT
                        | interface_key::AIRTIME_LIMIT_LONG
                        | interface_key::ID_CALLSIGN
                        | interface_key::ID_INTERVAL
                ) =>
            {
                InterfaceSettingTier::Standard
            }
            _ => InterfaceSettingTier::Advanced,
        }
    }

    pub fn condition(self, kind: InterfaceKind) -> Option<InterfaceSettingCondition> {
        match self.key.as_str() {
            interface_key::IFAC_SIZE => Some(InterfaceSettingCondition::IfacEnabled),
            interface_key::DISCOVERABLE if kind == InterfaceKind::TcpClient => {
                Some(InterfaceSettingCondition::KissFraming)
            }
            interface_key::ANNOUNCE_INTERVAL
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
            | interface_key::DISCOVERY_MODULATION
                if kind == InterfaceKind::TcpClient =>
            {
                Some(InterfaceSettingCondition::DiscoverableKiss)
            }
            interface_key::ANNOUNCE_INTERVAL
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
            | interface_key::DISCOVERY_MODULATION => Some(InterfaceSettingCondition::Discoverable),
            interface_key::ANNOUNCE_RATE_GRACE | interface_key::ANNOUNCE_RATE_PENALTY => {
                Some(InterfaceSettingCondition::AnnounceRateLimit)
            }
            common_key::IC_MAX_HELD_ANNOUNCES
            | common_key::IC_BURST_HOLD
            | common_key::IC_BURST_FREQ_NEW
            | common_key::IC_BURST_FREQ
            | common_key::IC_PR_BURST_FREQ_NEW
            | common_key::IC_PR_BURST_FREQ
            | common_key::IC_NEW_TIME
            | common_key::IC_BURST_PENALTY
            | common_key::IC_HELD_RELEASE_INTERVAL => {
                Some(InterfaceSettingCondition::IngressControl)
            }
            common_key::EC_PR_FREQ => Some(InterfaceSettingCondition::EgressControl),
            _ => None,
        }
    }

    pub fn is_supported(self, kind: InterfaceKind) -> bool {
        let discovery_capable = matches!(
            kind,
            InterfaceKind::TcpClient
                | InterfaceKind::TcpServer
                | InterfaceKind::Kiss
                | InterfaceKind::Rnode
                | InterfaceKind::RnodeMulti
                | InterfaceKind::Backbone
        );
        match self.key.as_str() {
            interface_key::IGNORE_CONFIG_WARNINGS => false,
            interface_key::DISCOVERABLE
            | interface_key::ANNOUNCE_INTERVAL
            | interface_key::DISCOVERY_STAMP_VALUE
            | interface_key::DISCOVERY_NAME
            | interface_key::DISCOVERY_ENCRYPT
            | interface_key::PUBLISH_IFAC
            | interface_key::LATITUDE
            | interface_key::LONGITUDE
            | interface_key::HEIGHT => discovery_capable,
            interface_key::REACHABLE_ON => {
                matches!(kind, InterfaceKind::TcpServer | InterfaceKind::Backbone)
            }
            interface_key::DISCOVERY_FREQUENCY
            | interface_key::DISCOVERY_BANDWIDTH
            | interface_key::DISCOVERY_MODULATION => {
                matches!(kind, InterfaceKind::TcpClient | InterfaceKind::Kiss)
            }
            _ => true,
        }
    }

    pub fn unsupported_reason(self, kind: InterfaceKind) -> Option<&'static str> {
        if self.is_supported(kind) {
            return None;
        }
        Some(match self.key.as_str() {
            interface_key::IGNORE_CONFIG_WARNINGS => {
                "Prns does not suppress configuration warnings per interface"
            }
            interface_key::REACHABLE_ON => {
                "only listening TCP and Backbone interfaces publish a reachable address"
            }
            interface_key::DISCOVERY_FREQUENCY
            | interface_key::DISCOVERY_BANDWIDTH
            | interface_key::DISCOVERY_MODULATION => {
                "only KISS discovery advertisements use separately configured radio metadata"
            }
            _ => "this interface type cannot publish interface-discovery advertisements",
        })
    }

    pub fn description(self) -> &'static str {
        match self.key.as_str() {
            interface_key::INTERFACE_MODE => {
                "Controls how routing and path discovery treat this interface."
            }
            interface_key::OUTGOING => {
                "Allows or prevents Prns from transmitting traffic through this interface."
            }
            interface_key::BITRATE => {
                "Overrides the interface bitrate used for pacing, MTU selection, and route costs."
            }
            interface_key::GRAVITY => {
                "Prefers this interface when equally fresh valid announce evidence arrives through multiple paths."
            }
            interface_key::ANNOUNCE_CAP => {
                "Limits announcement traffic to a percentage of this interface's bitrate."
            }
            interface_key::ANNOUNCE_RATE_TARGET => {
                "Sets the minimum target interval between announcements from one destination."
            }
            interface_key::ANNOUNCE_RATE_GRACE => {
                "Sets how many announcement-rate violations are tolerated before penalizing a destination."
            }
            interface_key::ANNOUNCE_RATE_PENALTY => {
                "Sets the penalty interval applied after the announcement-rate grace is exhausted."
            }
            interface_key::NETWORK_NAME => {
                "Adds this interface to a named IFAC network and restricts traffic to matching peers."
            }
            interface_key::PASS_PHRASE => {
                "Adds secret IFAC key material used to authenticate traffic on this interface."
            }
            interface_key::IFAC_SIZE => {
                "Sets the number of authentication bits carried by each IFAC-protected packet."
            }
            interface_key::DISCOVERABLE => {
                "Publishes this interface through Prns interface discovery."
            }
            interface_key::ANNOUNCE_INTERVAL => {
                "Sets how often this interface publishes its discovery advertisement."
            }
            interface_key::DISCOVERY_STAMP_VALUE => {
                "Sets the proof-of-work cost required for this interface's discovery advertisement."
            }
            interface_key::DISCOVERY_NAME => {
                "Publishes a human-readable name with this interface's discovery advertisement."
            }
            interface_key::DISCOVERY_ENCRYPT => {
                "Encrypts discovery advertisements to the configured network identity."
            }
            interface_key::REACHABLE_ON => {
                "Publishes the address peers should use to reach this listening interface."
            }
            interface_key::PUBLISH_IFAC => {
                "Includes this interface's IFAC identity in its discovery advertisement."
            }
            interface_key::LATITUDE => {
                "Publishes the interface's latitude in discovery metadata."
            }
            interface_key::LONGITUDE => {
                "Publishes the interface's longitude in discovery metadata."
            }
            interface_key::HEIGHT => {
                "Publishes the interface's height in discovery metadata."
            }
            interface_key::DISCOVERY_FREQUENCY => {
                "Publishes a radio frequency for a KISS discovery advertisement."
            }
            interface_key::DISCOVERY_BANDWIDTH => {
                "Publishes a radio bandwidth for a KISS discovery advertisement."
            }
            interface_key::DISCOVERY_MODULATION => {
                "Publishes a modulation name for a KISS discovery advertisement."
            }
            interface_key::BOOTSTRAP_ONLY => {
                "Marks this interface as temporary bootstrap connectivity that can retire after discovery succeeds."
            }
            interface_key::RECURSIVE_PRS => {
                "Allows recursive path-request forwarding according to this interface's routing policy."
            }
            interface_key::ANNOUNCES_FROM_INTERNAL => {
                "Allows announcements arriving from internal-mode interfaces to leave through this interface."
            }
            interface_key::ANNOUNCES_TO_INTERNAL => {
                "Allows announcements arriving on this interface to enter internal-mode interfaces."
            }
            interface_key::IGNORE_CONFIG_WARNINGS => {
                "Requests stock RNS warning suppression; Prns intentionally does not apply this setting."
            }
            interface_key::GROUP_ID => {
                "Selects the discovery group whose nearby AutoInterface or PrnsBluetoothAuto members can find each other."
            }
            interface_key::DISCOVERY_SCOPE => {
                "Sets how far AutoInterface multicast discovery packets may travel."
            }
            interface_key::DISCOVERY_PORT => {
                "Sets the UDP port used for AutoInterface peer discovery."
            }
            interface_key::DATA_PORT => {
                "Sets the UDP port used for AutoInterface packet traffic."
            }
            interface_key::DEVICES => {
                "Restricts AutoInterface to the listed network device names."
            }
            interface_key::IGNORED_DEVICES => {
                "Prevents AutoInterface from using the listed network device names."
            }
            interface_key::MULTICAST_ADDRESS_TYPE => {
                "Chooses temporary or permanently assigned IPv6 multicast addressing for AutoInterface discovery."
            }
            interface_key::TARGET_HOST => "Sets the host name or address this client connects to.",
            interface_key::TARGET_PORT => "Sets the remote port this client connects to.",
            interface_key::TARGET => "Sets the complete remote WebSocket URL.",
            interface_key::FRAMING => {
                "Automatically detects raw packet, HDLC, or KISS framing, or fixes one explicitly."
            }
            interface_key::KISS_FRAMING => {
                "Wraps packets in KISS framing while they cross this TCP connection."
            }
            interface_key::I2P_TUNNELED => {
                "Treats this TCP connection as already carried through an I2P tunnel."
            }
            interface_key::CONNECT_TIMEOUT => {
                "Limits how long each outbound connection attempt may take."
            }
            interface_key::MAX_RECONNECT_TRIES => {
                "Limits reconnect attempts after an established connection is lost."
            }
            interface_key::FIXED_MTU => {
                "Overrides automatic MTU selection with a fixed packet size."
            }
            interface_key::LISTEN_IP => "Sets the local IP address this server listens on.",
            interface_key::LISTEN_PORT => "Sets the local port this server listens on.",
            interface_key::DEVICE => {
                "Selects a local network or serial device, depending on the interface type."
            }
            interface_key::PORT => {
                "Sets a listener port or serial path accepted by this interface type."
            }
            interface_key::PREFER_IPV6 => {
                "Prefers IPv6 addresses when both address families are available."
            }
            interface_key::FORWARD_IP => "Sets the UDP address that outgoing packets are sent to.",
            interface_key::FORWARD_PORT => "Sets the UDP port that outgoing packets are sent to.",
            interface_key::SPEED => "Sets the serial line speed in bits per second.",
            interface_key::DATABITS => "Sets the number of data bits in each serial character.",
            interface_key::PARITY => "Sets serial parity checking.",
            interface_key::STOPBITS => "Sets the number of serial stop bits.",
            interface_key::FLOW_CONTROL => {
                "Enables hardware or ready-command flow control supported by this interface."
            }
            interface_key::PREAMBLE => "Sets the KISS modem preamble duration.",
            interface_key::TXTAIL => "Sets the KISS modem transmit-tail duration.",
            interface_key::PERSISTENCE => "Sets the KISS channel-access persistence value.",
            interface_key::SLOTTIME => "Sets the KISS channel-access slot duration.",
            interface_key::ID_CALLSIGN => "Sets the station-identification callsign.",
            interface_key::ID_INTERVAL => {
                "Sets the interval between station-identification transmissions."
            }
            interface_key::CALLSIGN => "Sets the AX.25 callsign used by this interface.",
            interface_key::SSID => "Sets the AX.25 secondary station identifier.",
            interface_key::FREQUENCY => "Sets the RNode radio carrier frequency in hertz.",
            interface_key::BANDWIDTH => "Sets the RNode radio bandwidth in hertz.",
            interface_key::SPREADINGFACTOR => "Sets the RNode LoRa spreading factor.",
            interface_key::CODINGRATE => "Sets the RNode LoRa coding rate.",
            interface_key::TXPOWER => "Sets the RNode transmit power in dBm.",
            interface_key::AIRTIME_LIMIT_SHORT => {
                "Sets the short-term radio airtime limit as a percentage."
            }
            interface_key::AIRTIME_LIMIT_LONG => {
                "Sets the long-term radio airtime limit as a percentage."
            }
            interface_key::COMMAND => "Sets the executable command used by this pipe interface.",
            interface_key::RESPAWN_DELAY => {
                "Sets how long the pipe interface waits before restarting its command."
            }
            interface_key::REMOTE => "Sets the remote Backbone address.",
            interface_key::LISTEN_ON => "Sets the local Backbone listener address.",
            interface_key::PEERS => "Lists the I2P destinations this interface connects to.",
            interface_key::CONNECTABLE => {
                "Allows other I2P peers to establish inbound connections to this interface."
            }
            common_key::INGRESS_CONTROL => {
                "Enables burst control for announcements and path requests entering this interface."
            }
            common_key::EGRESS_CONTROL => {
                "Enables rate control for path requests leaving this interface."
            }
            common_key::IC_MAX_HELD_ANNOUNCES => {
                "Sets the maximum announcements ingress control may hold for later release."
            }
            common_key::IC_BURST_HOLD => {
                "Sets how long ingress control holds traffic after detecting a burst."
            }
            common_key::IC_BURST_FREQ_NEW => {
                "Sets the announcement burst threshold while an interface is considered new."
            }
            common_key::IC_BURST_FREQ => {
                "Sets the normal announcement burst threshold for ingress control."
            }
            common_key::IC_PR_BURST_FREQ_NEW => {
                "Sets the path-request burst threshold while an interface is considered new."
            }
            common_key::IC_PR_BURST_FREQ => {
                "Sets the normal path-request burst threshold for ingress control."
            }
            common_key::EC_PR_FREQ => {
                "Sets the maximum path-request frequency allowed by egress control."
            }
            common_key::IC_NEW_TIME => {
                "Sets how long ingress control treats this interface as newly started."
            }
            common_key::IC_BURST_PENALTY => {
                "Sets the additional hold time applied after repeated ingress bursts."
            }
            common_key::IC_HELD_RELEASE_INTERVAL => {
                "Sets the interval between announcements released from the ingress-control queue."
            }
            _ => "Configures an advanced value accepted by this interface type.",
        }
    }

    pub fn default_hint(self, kind: InterfaceKind) -> Option<&'static str> {
        match self.key.as_str() {
            interface_key::INTERFACE_MODE
                if matches!(
                    kind,
                    InterfaceKind::PrnsUsbAuto
                        | InterfaceKind::PrnsWebSocketClient
                        | InterfaceKind::PrnsWebSocketServer
                ) =>
            {
                Some("pointtopoint")
            }
            interface_key::INTERFACE_MODE => Some("full"),
            interface_key::OUTGOING => Some("Yes"),
            interface_key::GRAVITY => Some("0"),
            interface_key::ANNOUNCE_CAP => Some("2%"),
            interface_key::NETWORK_NAME | interface_key::PASS_PHRASE => Some("not set"),
            interface_key::IFAC_SIZE
                if matches!(
                    kind,
                    InterfaceKind::Serial
                        | InterfaceKind::Kiss
                        | InterfaceKind::Ax25Kiss
                        | InterfaceKind::Rnode
                        | InterfaceKind::RnodeMulti
                        | InterfaceKind::Pipe
                        | InterfaceKind::PrnsBluetoothAuto
                ) =>
            {
                Some("64 bits when IFAC is enabled")
            }
            interface_key::IFAC_SIZE => Some("128 bits when IFAC is enabled"),
            interface_key::DISCOVERABLE => Some("No"),
            interface_key::ANNOUNCE_INTERVAL => Some("360 minutes"),
            interface_key::DISCOVERY_STAMP_VALUE => Some("14"),
            interface_key::DISCOVERY_ENCRYPT | interface_key::PUBLISH_IFAC => Some("No"),
            interface_key::BOOTSTRAP_ONLY => Some("No"),
            interface_key::RECURSIVE_PRS => Some("No"),
            interface_key::ANNOUNCES_FROM_INTERNAL => Some("Yes"),
            interface_key::ANNOUNCES_TO_INTERNAL => Some("No"),
            interface_key::GROUP_ID
                if matches!(kind, InterfaceKind::Auto | InterfaceKind::PrnsBluetoothAuto) =>
            {
                Some("reticulum")
            }
            interface_key::DISCOVERY_SCOPE if kind == InterfaceKind::Auto => Some("link"),
            interface_key::DISCOVERY_PORT if kind == InterfaceKind::Auto => Some("29716"),
            interface_key::DATA_PORT if kind == InterfaceKind::Auto => Some("42671"),
            interface_key::DEVICES if kind == InterfaceKind::Auto => Some("all usable devices"),
            interface_key::IGNORED_DEVICES if kind == InterfaceKind::Auto => Some("none"),
            interface_key::MULTICAST_ADDRESS_TYPE if kind == InterfaceKind::Auto => {
                Some("temporary")
            }
            common_key::INGRESS_CONTROL => Some("Yes"),
            common_key::EGRESS_CONTROL => Some("No"),
            common_key::IC_MAX_HELD_ANNOUNCES => Some("256"),
            common_key::IC_BURST_HOLD => Some("15 seconds"),
            common_key::IC_BURST_FREQ_NEW => Some("3 Hz"),
            common_key::IC_BURST_FREQ => Some("10 Hz"),
            common_key::IC_PR_BURST_FREQ_NEW => Some("3 Hz"),
            common_key::IC_PR_BURST_FREQ => Some("8 Hz"),
            common_key::EC_PR_FREQ => Some("5 Hz"),
            common_key::IC_NEW_TIME => Some("7200 seconds"),
            common_key::IC_BURST_PENALTY => Some("15 seconds"),
            common_key::IC_HELD_RELEASE_INTERVAL => Some("5 seconds"),
            _ => None,
        }
    }

    pub fn required_hint(self, kind: InterfaceKind) -> Option<&'static str> {
        match (kind, self.key.as_str()) {
            (InterfaceKind::TcpClient, interface_key::TARGET_HOST)
            | (InterfaceKind::BackboneClient, interface_key::TARGET_HOST) => {
                Some("a remote host is required")
            }
            (InterfaceKind::TcpClient, interface_key::TARGET_PORT)
            | (InterfaceKind::BackboneClient, interface_key::TARGET_PORT) => {
                Some("a remote port is required")
            }
            (
                InterfaceKind::TcpServer
                | InterfaceKind::Backbone
                | InterfaceKind::PrnsWebSocketServer,
                interface_key::LISTEN_PORT,
            ) => Some("a listener port is required"),
            (
                InterfaceKind::Serial
                | InterfaceKind::Kiss
                | InterfaceKind::Ax25Kiss
                | InterfaceKind::Rnode
                | InterfaceKind::RnodeMulti
                | InterfaceKind::Weave,
                interface_key::PORT,
            ) => Some("a device or transport target is required"),
            (InterfaceKind::Ax25Kiss, interface_key::CALLSIGN) => Some("a callsign is required"),
            (InterfaceKind::Ax25Kiss, interface_key::SSID) => Some("an SSID is required"),
            (
                InterfaceKind::Rnode,
                interface_key::FREQUENCY
                | interface_key::BANDWIDTH
                | interface_key::SPREADINGFACTOR
                | interface_key::CODINGRATE
                | interface_key::TXPOWER,
            ) => Some("a radio value is required"),
            (InterfaceKind::Pipe, interface_key::COMMAND) => Some("a command is required"),
            (InterfaceKind::PrnsWebSocketClient, interface_key::TARGET) => {
                Some("a ws:// or wss:// target is required")
            }
            _ => None,
        }
    }

    pub fn inherits_when_unset(self) -> bool {
        matches!(
            self.key.as_str(),
            interface_key::ANNOUNCE_RATE_TARGET
                | interface_key::ANNOUNCE_RATE_GRACE
                | interface_key::ANNOUNCE_RATE_PENALTY
                | interface_key::GRAVITY
                | interface_key::RECURSIVE_PRS
                | interface_key::ANNOUNCES_FROM_INTERNAL
                | interface_key::ANNOUNCES_TO_INTERNAL
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
                | common_key::IC_HELD_RELEASE_INTERVAL
        )
    }

    pub fn effective_value(self, planned: &PlannedInterface) -> Option<String> {
        let policy = &planned.policy;
        let common = &policy.common;
        match self.key.as_str() {
            interface_key::INTERFACE_MODE => Some(interface_mode_name(policy.mode).to_string()),
            interface_key::OUTGOING => Some(yes_no(!matches!(
                policy.capabilities.egress,
                EgressCapability::Disabled
            ))),
            interface_key::BITRATE => Some(policy.bitrate.get().to_string()),
            interface_key::ANNOUNCE_CAP => Some(match policy.announce_bandwidth_cap {
                AnnounceBandwidthCap::Unlimited => "unlimited".to_string(),
                AnnounceBandwidthCap::Limited { cap_per_mille } => {
                    format!("{}%", concise_decimal(f64::from(cap_per_mille) / 10.0))
                }
            }),
            interface_key::ANNOUNCE_RATE_TARGET => policy
                .announce_rate_limit
                .map(|limit| concise_decimal(limit.target_ms as f64 / 1_000.0)),
            interface_key::ANNOUNCE_RATE_GRACE => policy
                .announce_rate_limit
                .map(|limit| limit.grace.to_string()),
            interface_key::ANNOUNCE_RATE_PENALTY => policy
                .announce_rate_limit
                .map(|limit| concise_decimal(limit.penalty_ms as f64 / 1_000.0)),
            interface_key::NETWORK_NAME => match &planned.access {
                InterfaceAccessPlan::Ifac { network_name, .. } => network_name.clone(),
                InterfaceAccessPlan::Open => None,
            },
            interface_key::PASS_PHRASE => match &planned.access {
                InterfaceAccessPlan::Ifac { passphrase, .. } => passphrase.clone(),
                InterfaceAccessPlan::Open => None,
            },
            interface_key::IFAC_SIZE => match planned.access {
                InterfaceAccessPlan::Ifac { size, .. } => Some((size.bytes() * 8).to_string()),
                InterfaceAccessPlan::Open => None,
            },
            interface_key::DISCOVERABLE => Some(yes_no(!matches!(
                planned.discovery,
                InterfaceDiscoveryPlan::Disabled
            ))),
            interface_key::ANNOUNCE_INTERVAL => discovery_announcement(planned)
                .map(|announcement| (announcement.interval.0 / 60_000).to_string()),
            interface_key::DISCOVERY_STAMP_VALUE => discovery_announcement(planned)
                .map(|announcement| announcement.stamp_cost.get().to_string()),
            interface_key::DISCOVERY_NAME => {
                discovery_announcement(planned).and_then(|announcement| announcement.name.clone())
            }
            interface_key::DISCOVERY_ENCRYPT => {
                discovery_announcement(planned).map(|announcement| {
                    yes_no(matches!(
                        announcement.encryption,
                        DiscoveryEncryption::NetworkIdentity
                    ))
                })
            }
            interface_key::PUBLISH_IFAC => discovery_announcement(planned).map(|announcement| {
                yes_no(matches!(
                    announcement.ifac,
                    DiscoveryIfacPublication::Include
                ))
            }),
            interface_key::REACHABLE_ON => {
                discovery_advertisement(planned).and_then(|value| match value {
                    DiscoveryAdvertisementPlan::Backbone { reachable_on, .. }
                    | DiscoveryAdvertisementPlan::TcpServer { reachable_on, .. } => {
                        Some(reachable_on.clone())
                    }
                    _ => None,
                })
            }
            interface_key::REACHABLE_PORT => {
                discovery_advertisement(planned).and_then(|value| match value {
                    DiscoveryAdvertisementPlan::Backbone { port, .. }
                    | DiscoveryAdvertisementPlan::TcpServer { port, .. } => Some(port.to_string()),
                    _ => None,
                })
            }
            interface_key::LATITUDE => discovery_announcement(planned)
                .and_then(|announcement| announcement.location.latitude)
                .map(concise_decimal),
            interface_key::LONGITUDE => discovery_announcement(planned)
                .and_then(|announcement| announcement.location.longitude)
                .map(concise_decimal),
            interface_key::HEIGHT => discovery_announcement(planned)
                .and_then(|announcement| announcement.location.height)
                .map(concise_decimal),
            interface_key::DISCOVERY_FREQUENCY => {
                discovery_advertisement(planned).and_then(|value| match value {
                    DiscoveryAdvertisementPlan::Kiss { frequency_hz, .. } => {
                        Some(frequency_hz.to_string())
                    }
                    _ => None,
                })
            }
            interface_key::DISCOVERY_BANDWIDTH => {
                discovery_advertisement(planned).and_then(|value| match value {
                    DiscoveryAdvertisementPlan::Kiss { bandwidth_hz, .. } => {
                        Some(bandwidth_hz.to_string())
                    }
                    _ => None,
                })
            }
            interface_key::DISCOVERY_MODULATION => {
                discovery_advertisement(planned).and_then(|value| match value {
                    DiscoveryAdvertisementPlan::Kiss { modulation, .. } => Some(modulation.clone()),
                    _ => None,
                })
            }
            interface_key::GRAVITY => Some(planned.policy.gravity.get().to_string()),
            interface_key::BOOTSTRAP_ONLY => Some(yes_no(matches!(
                planned.lifecycle,
                ConfiguredInterfaceLifecycle::BootstrapOnly
            ))),
            interface_key::RECURSIVE_PRS => match common.forwarding.recursive_path_requests {
                RecursivePathRequestPolicy::InheritNode => None,
                RecursivePathRequestPolicy::Enabled => Some(yes_no(true)),
                RecursivePathRequestPolicy::Disabled => Some(yes_no(false)),
            },
            interface_key::ANNOUNCES_FROM_INTERNAL => {
                Some(yes_no(common.forwarding.announces_from_internal))
            }
            interface_key::ANNOUNCES_TO_INTERNAL => {
                Some(yes_no(common.forwarding.announces_to_internal))
            }
            common_key::INGRESS_CONTROL => Some(yes_no(common.ingress_control.enabled)),
            common_key::EGRESS_CONTROL => Some(yes_no(common.path_request_egress.enabled)),
            common_key::IC_MAX_HELD_ANNOUNCES => {
                Some(common.ingress_control.max_held_announces.to_string())
            }
            common_key::IC_BURST_HOLD => Some(concise_decimal(
                common.ingress_control.burst_hold_millis as f64 / 1_000.0,
            )),
            common_key::IC_BURST_FREQ_NEW => Some(concise_decimal(
                common.ingress_control.announce_burst_frequency_new.get() as f64 / 1_000.0,
            )),
            common_key::IC_BURST_FREQ => Some(concise_decimal(
                common.ingress_control.announce_burst_frequency.get() as f64 / 1_000.0,
            )),
            common_key::IC_PR_BURST_FREQ_NEW => Some(concise_decimal(
                common
                    .ingress_control
                    .path_request_burst_frequency_new
                    .get() as f64
                    / 1_000.0,
            )),
            common_key::IC_PR_BURST_FREQ => Some(concise_decimal(
                common.ingress_control.path_request_burst_frequency.get() as f64 / 1_000.0,
            )),
            common_key::EC_PR_FREQ => Some(concise_decimal(
                common.path_request_egress.frequency.get() as f64 / 1_000.0,
            )),
            common_key::IC_NEW_TIME => Some(concise_decimal(
                common.ingress_control.new_interface_millis as f64 / 1_000.0,
            )),
            common_key::IC_BURST_PENALTY => Some(concise_decimal(
                common.ingress_control.burst_penalty_millis as f64 / 1_000.0,
            )),
            common_key::IC_HELD_RELEASE_INTERVAL => Some(concise_decimal(
                common.ingress_control.held_release_interval_millis as f64 / 1_000.0,
            )),
            interface_key::SPEED => serial_line(planned).map(|line| line.baud().to_string()),
            interface_key::DATABITS => serial_line(planned).map(|line| match line.data_bits() {
                crate::SerialDataBits::Five => "5".to_string(),
                crate::SerialDataBits::Six => "6".to_string(),
                crate::SerialDataBits::Seven => "7".to_string(),
                crate::SerialDataBits::Eight => "8".to_string(),
            }),
            interface_key::PARITY => serial_line(planned).map(|line| match line.parity() {
                crate::SerialParity::None => "none".to_string(),
                crate::SerialParity::Even => "even".to_string(),
                crate::SerialParity::Odd => "odd".to_string(),
            }),
            interface_key::STOPBITS => serial_line(planned).map(|line| match line.stop_bits() {
                crate::SerialStopBits::One => "1".to_string(),
                crate::SerialStopBits::Two => "2".to_string(),
            }),
            interface_key::FLOW_CONTROL => match &planned.medium {
                PlannedMedium::Kiss { flow_control, .. }
                | PlannedMedium::Ax25Kiss { flow_control, .. }
                | PlannedMedium::Rnode { flow_control, .. } => Some(yes_no(matches!(
                    flow_control,
                    crate::ReadyCommandFlowControl::Enabled
                ))),
                _ => None,
            },
            interface_key::PREAMBLE => match &planned.medium {
                PlannedMedium::Kiss { preamble_ms, .. }
                | PlannedMedium::Ax25Kiss { preamble_ms, .. } => Some(preamble_ms.to_string()),
                _ => None,
            },
            interface_key::TXTAIL => match &planned.medium {
                PlannedMedium::Kiss { txtail_ms, .. }
                | PlannedMedium::Ax25Kiss { txtail_ms, .. } => Some(txtail_ms.to_string()),
                _ => None,
            },
            interface_key::PERSISTENCE => match &planned.medium {
                PlannedMedium::Kiss { persistence, .. }
                | PlannedMedium::Ax25Kiss { persistence, .. } => Some(persistence.to_string()),
                _ => None,
            },
            interface_key::SLOTTIME => match &planned.medium {
                PlannedMedium::Kiss { slottime_ms, .. }
                | PlannedMedium::Ax25Kiss { slottime_ms, .. } => Some(slottime_ms.to_string()),
                _ => None,
            },
            interface_key::ID_CALLSIGN => match &planned.medium {
                PlannedMedium::Kiss {
                    station_id: Some(station),
                    ..
                }
                | PlannedMedium::Rnode {
                    station_id: Some(station),
                    ..
                } => Some(station.callsign().to_string()),
                _ => None,
            },
            interface_key::ID_INTERVAL => match &planned.medium {
                PlannedMedium::Kiss {
                    station_id: Some(station),
                    ..
                }
                | PlannedMedium::Rnode {
                    station_id: Some(station),
                    ..
                } => Some(station.interval_seconds().to_string()),
                _ => None,
            },
            interface_key::CALLSIGN => match &planned.medium {
                PlannedMedium::Ax25Kiss { callsign, .. } => Some(callsign.clone()),
                _ => None,
            },
            interface_key::SSID => match &planned.medium {
                PlannedMedium::Ax25Kiss { ssid, .. } => Some(ssid.to_string()),
                _ => None,
            },
            interface_key::FREQUENCY => match &planned.medium {
                PlannedMedium::Rnode { frequency_hz, .. } => Some(frequency_hz.to_string()),
                _ => None,
            },
            interface_key::BANDWIDTH => match &planned.medium {
                PlannedMedium::Rnode { bandwidth_hz, .. } => Some(bandwidth_hz.to_string()),
                _ => None,
            },
            interface_key::SPREADINGFACTOR => match &planned.medium {
                PlannedMedium::Rnode {
                    spreading_factor, ..
                } => Some(spreading_factor.to_string()),
                _ => None,
            },
            interface_key::CODINGRATE => match &planned.medium {
                PlannedMedium::Rnode { coding_rate, .. } => Some(coding_rate.to_string()),
                _ => None,
            },
            interface_key::TXPOWER => match &planned.medium {
                PlannedMedium::Rnode { tx_power_dbm, .. } => Some(tx_power_dbm.to_string()),
                _ => None,
            },
            interface_key::AIRTIME_LIMIT_SHORT => match &planned.medium {
                PlannedMedium::Rnode {
                    airtime_limit_short,
                    ..
                } => {
                    airtime_limit_short.map(|limit| concise_decimal(f64::from(limit.get()) / 100.0))
                }
                _ => None,
            },
            interface_key::AIRTIME_LIMIT_LONG => match &planned.medium {
                PlannedMedium::Rnode {
                    airtime_limit_long, ..
                } => {
                    airtime_limit_long.map(|limit| concise_decimal(f64::from(limit.get()) / 100.0))
                }
                _ => None,
            },
            interface_key::COMMAND => match &planned.medium {
                PlannedMedium::Pipe { command, .. } => Some(command.source().to_string()),
                _ => None,
            },
            interface_key::RESPAWN_DELAY => match &planned.medium {
                PlannedMedium::Pipe { respawn_delay, .. } => {
                    Some(concise_decimal(respawn_delay.get().as_secs_f64()))
                }
                _ => None,
            },
            interface_key::PEERS => match &planned.medium {
                PlannedMedium::I2p { peers, .. } => Some(if peers.is_empty() {
                    "none".to_string()
                } else {
                    peers
                        .iter()
                        .map(|peer| peer.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }),
                _ => None,
            },
            interface_key::CONNECTABLE => match &planned.medium {
                PlannedMedium::I2p { reachability, .. } => {
                    Some(yes_no(reachability.is_connectable()))
                }
                _ => None,
            },
            interface_key::TARGET => match &planned.medium {
                PlannedMedium::PrnsWebSocketClient { target, .. } => {
                    Some(target.as_str().to_string())
                }
                _ => None,
            },
            interface_key::FRAMING => match &planned.medium {
                PlannedMedium::PrnsWebSocketClient { framing, .. }
                | PlannedMedium::PrnsWebSocketServer { framing, .. } => {
                    Some(framing.name().to_string())
                }
                _ => None,
            },
            interface_key::GROUP_ID => auto_plan(planned)
                .map(|auto| auto.group_id().as_str().to_string())
                .or_else(|| bluetooth_auto_group_id(planned).map(str::to_string)),
            interface_key::DISCOVERY_SCOPE => auto_plan(planned)
                .map(|auto| format!("{:?}", auto.discovery_scope()).to_ascii_lowercase()),
            interface_key::DISCOVERY_PORT => {
                auto_plan(planned).map(|auto| auto.discovery_port().get().to_string())
            }
            interface_key::DATA_PORT => {
                auto_plan(planned).map(|auto| auto.data_port().get().to_string())
            }
            interface_key::DEVICES => auto_plan(planned).map(|auto| {
                if auto.devices().allowed().is_empty() {
                    "all usable devices".to_string()
                } else {
                    auto.devices().allowed().join(", ")
                }
            }),
            interface_key::IGNORED_DEVICES => auto_plan(planned).map(|auto| {
                if auto.devices().ignored().is_empty() {
                    "none".to_string()
                } else {
                    auto.devices().ignored().join(", ")
                }
            }),
            interface_key::MULTICAST_ADDRESS_TYPE => auto_plan(planned)
                .map(|auto| format!("{:?}", auto.multicast_address_type()).to_ascii_lowercase()),
            _ => None,
        }
    }

    pub fn input_kind(self, kind: InterfaceKind) -> InterfaceSettingInputKind {
        match self.key.as_str() {
            interface_key::ANNOUNCE_RATE_TARGET => InterfaceSettingInputKind::Text,
            interface_key::OUTGOING
            | interface_key::DISCOVERABLE
            | interface_key::DISCOVERY_ENCRYPT
            | interface_key::PUBLISH_IFAC
            | interface_key::BOOTSTRAP_ONLY
            | interface_key::RECURSIVE_PRS
            | interface_key::ANNOUNCES_FROM_INTERNAL
            | interface_key::ANNOUNCES_TO_INTERNAL
            | interface_key::IGNORE_CONFIG_WARNINGS
            | interface_key::KISS_FRAMING
            | interface_key::I2P_TUNNELED
            | interface_key::PREFER_IPV6
            | interface_key::FLOW_CONTROL
            | interface_key::CONNECTABLE
            | common_key::INGRESS_CONTROL
            | common_key::EGRESS_CONTROL => InterfaceSettingInputKind::Boolean,
            interface_key::BITRATE
            | interface_key::ANNOUNCE_RATE_GRACE
            | interface_key::ANNOUNCE_RATE_PENALTY
            | interface_key::IFAC_SIZE
            | interface_key::DISCOVERY_STAMP_VALUE
            | interface_key::DISCOVERY_FREQUENCY
            | interface_key::DISCOVERY_BANDWIDTH
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
            | interface_key::GRAVITY
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
            interface_key::DISCOVERY_PORT
            | interface_key::DATA_PORT
            | interface_key::TARGET_PORT
            | interface_key::LISTEN_PORT
            | interface_key::FORWARD_PORT => InterfaceSettingInputKind::Port,
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
            interface_key::ANNOUNCE_CAP => "a percentage from 0 through 100",
            interface_key::ANNOUNCE_RATE_TARGET => {
                "off, no, false, or seconds as a non-negative whole number"
            }
            interface_key::ANNOUNCE_RATE_PENALTY
            | interface_key::CONNECT_TIMEOUT
            | interface_key::ID_INTERVAL => "seconds as a non-negative whole number",
            interface_key::RESPAWN_DELAY => "seconds as a non-negative number",
            interface_key::ANNOUNCE_INTERVAL => "minutes as a whole number",
            interface_key::IFAC_SIZE => "an IFAC size from 8 through 512 bits",
            interface_key::FIXED_MTU => "bytes as a non-negative whole number",
            interface_key::PREAMBLE | interface_key::TXTAIL | interface_key::SLOTTIME => {
                "milliseconds as a non-negative whole number"
            }
            interface_key::DISCOVERY_SCOPE => "link, admin, site, organisation, or global",
            interface_key::MULTICAST_ADDRESS_TYPE => "temporary or permanent",
            interface_key::BITRATE | interface_key::SPEED => {
                "bits per second as a non-negative whole number"
            }
            interface_key::DISCOVERY_FREQUENCY | interface_key::FREQUENCY => {
                "hertz as a non-negative whole number"
            }
            interface_key::DISCOVERY_BANDWIDTH | interface_key::BANDWIDTH => {
                "hertz as a non-negative whole number"
            }
            interface_key::TXPOWER => "dBm as a whole number",
            interface_key::AIRTIME_LIMIT_SHORT | interface_key::AIRTIME_LIMIT_LONG => {
                "a percentage"
            }
            interface_key::PARITY => "none, even, or odd",
            interface_key::FRAMING => "auto, raw, hdlc, or kiss",
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

    pub fn format_value(self, value: impl AsRef<str>) -> String {
        let value = value.as_ref();
        if self.key.as_str() == interface_key::ANNOUNCE_RATE_TARGET
            && announce_rate_target_is_explicit_off(value)
        {
            return "off".to_string();
        }
        if matches!(
            self.key.as_str(),
            interface_key::AIRTIME_LIMIT_SHORT | interface_key::AIRTIME_LIMIT_LONG
        ) {
            return format!("{value}%");
        }
        if matches!(
            self.key.as_str(),
            interface_key::BITRATE | interface_key::SPEED
        ) {
            return value
                .replace('_', "")
                .parse::<u64>()
                .map_or_else(|_| value.to_string(), format_si_bitrate);
        }
        if matches!(
            self.key.as_str(),
            interface_key::DISCOVERY_FREQUENCY
                | interface_key::DISCOVERY_BANDWIDTH
                | interface_key::FREQUENCY
                | interface_key::BANDWIDTH
        ) {
            return value
                .replace('_', "")
                .parse::<u64>()
                .map_or_else(|_| format!("{value} Hz"), format_si_frequency);
        }
        let unit = match self.key.as_str() {
            interface_key::ANNOUNCE_RATE_TARGET
            | interface_key::ANNOUNCE_RATE_PENALTY
            | interface_key::CONNECT_TIMEOUT
            | interface_key::ID_INTERVAL
            | interface_key::RESPAWN_DELAY
            | common_key::IC_BURST_HOLD
            | common_key::IC_NEW_TIME
            | common_key::IC_BURST_PENALTY
            | common_key::IC_HELD_RELEASE_INTERVAL => Some("seconds"),
            interface_key::ANNOUNCE_INTERVAL => Some("minutes"),
            interface_key::IFAC_SIZE => Some("bits"),
            common_key::IC_BURST_FREQ_NEW
            | common_key::IC_BURST_FREQ
            | common_key::IC_PR_BURST_FREQ_NEW
            | common_key::IC_PR_BURST_FREQ
            | common_key::EC_PR_FREQ => Some("Hz"),
            interface_key::PREAMBLE | interface_key::TXTAIL | interface_key::SLOTTIME => Some("ms"),
            interface_key::FIXED_MTU => Some("bytes"),
            interface_key::HEIGHT => Some("m"),
            interface_key::TXPOWER => Some("dBm"),
            _ => None,
        };
        unit.map_or_else(
            || value.to_string(),
            |unit| format!("{} {unit}", grouped_integer(value)),
        )
    }

    pub fn parse(
        self,
        kind: InterfaceKind,
        input: &str,
    ) -> Result<InterfaceSetting, InterfaceSettingInputError> {
        if self.key.as_str() == interface_key::ANNOUNCE_RATE_TARGET {
            let value = if announce_rate_target_is_explicit_off(input) {
                InterfaceSettingValue::Text("off".to_string())
            } else {
                InterfaceSettingValue::Unsigned(
                    cleaned_number(input)
                        .parse()
                        .map_err(|_| InterfaceSettingInputError::AnnounceRateTarget)?,
                )
            };
            return Ok(InterfaceSetting::new(self.key, value));
        }
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

fn interface_mode_name(mode: InterfaceMode) -> &'static str {
    match mode {
        InterfaceMode::Full => "full",
        InterfaceMode::PointToPoint => "pointtopoint",
        InterfaceMode::AccessPoint => "access_point",
        InterfaceMode::Roaming => "roaming",
        InterfaceMode::Boundary => "boundary",
        InterfaceMode::Gateway => "gateway",
        InterfaceMode::Internal => "internal",
    }
}

fn yes_no(value: bool) -> String {
    if value { "Yes" } else { "No" }.to_string()
}

fn concise_decimal(value: f64) -> String {
    let rendered = format!("{value:.3}");
    rendered
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn grouped_integer(value: &str) -> String {
    let cleaned = value.replace('_', "");
    let (sign, digits) = cleaned
        .strip_prefix('-')
        .map_or(("", cleaned.as_str()), |digits| ("-", digits));
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.to_string();
    }
    let mut grouped = String::with_capacity(cleaned.len() + cleaned.len() / 3);
    grouped.push_str(sign);
    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn format_si_bitrate(bits_per_second: u64) -> String {
    if bits_per_second >= 1_000_000_000 {
        format_scaled_quantity(bits_per_second, 1_000_000_000, 9, "Gbps")
    } else if bits_per_second >= 1_000_000 {
        format_scaled_quantity(bits_per_second, 1_000_000, 6, "Mbps")
    } else if bits_per_second >= 1_000 {
        format_scaled_quantity(bits_per_second, 1_000, 3, "kbps")
    } else {
        format!("{bits_per_second} bps")
    }
}

fn format_si_frequency(hertz: u64) -> String {
    if hertz >= 1_000_000_000 {
        format_scaled_quantity(hertz, 1_000_000_000, 9, "GHz")
    } else if hertz >= 1_000_000 {
        format_scaled_quantity(hertz, 1_000_000, 6, "MHz")
    } else if hertz >= 1_000 {
        format_scaled_quantity(hertz, 1_000, 3, "kHz")
    } else {
        format!("{hertz} Hz")
    }
}

fn format_scaled_quantity(value: u64, scale: u64, decimal_places: usize, unit: &str) -> String {
    let whole = value / scale;
    let remainder = value % scale;
    if remainder == 0 {
        return format!("{whole} {unit}");
    }
    let mut fractional = format!("{remainder:0decimal_places$}");
    while fractional.ends_with('0') {
        fractional.pop();
    }
    format!("{whole}.{fractional} {unit}")
}

fn discovery_announcement(planned: &PlannedInterface) -> Option<&crate::DiscoveryAnnouncementPlan> {
    match &planned.discovery {
        InterfaceDiscoveryPlan::Announce(announcement) => Some(announcement),
        InterfaceDiscoveryPlan::Disabled | InterfaceDiscoveryPlan::Unpublishable(_) => None,
    }
}

fn discovery_advertisement(planned: &PlannedInterface) -> Option<&DiscoveryAdvertisementPlan> {
    discovery_announcement(planned).map(|announcement| &announcement.advertisement)
}

fn auto_plan(planned: &PlannedInterface) -> Option<&crate::AutoInterfacePlan> {
    match &planned.medium {
        PlannedMedium::AutoWifi(auto) => Some(auto),
        _ => None,
    }
}

fn bluetooth_auto_group_id(planned: &PlannedInterface) -> Option<&str> {
    match &planned.medium {
        PlannedMedium::PrnsBluetoothAuto { group_id } => Some(group_id.as_str()),
        _ => None,
    }
}

fn serial_line(planned: &PlannedInterface) -> Option<crate::SerialLinePlan> {
    match &planned.medium {
        PlannedMedium::Serial { line, .. }
        | PlannedMedium::Kiss { line, .. }
        | PlannedMedium::Ax25Kiss { line, .. } => Some(*line),
        _ => None,
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

    pub fn supported_setting_specs(self) -> Vec<InterfaceSettingSpec> {
        self.setting_specs()
            .into_iter()
            .filter(|spec| spec.is_supported(self))
            .collect()
    }

    pub fn supports_editing_setting(self, key: InterfaceSettingKey) -> bool {
        self.setting_specs()
            .into_iter()
            .find(|spec| spec.key() == key.canonical())
            .is_some_and(|spec| spec.is_supported(self))
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
    AnnounceRateTarget,
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
            Self::AnnounceRateTarget => {
                "enter off, no, false, or a non-negative whole number of seconds"
            }
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
