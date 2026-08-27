use std::time::Duration;

use prns_core::interfaces::rnode::policy as rnode_policy;
use prns_core::interfaces::tcp::TcpWireFraming;
use prns_core::interfaces::websocket::WebSocketFramingSelection;
pub use prns_core::interfaces::wifi_auto::{
    DiscoveryScope as AutoInterfaceDiscoveryScope,
    MulticastAddressType as AutoInterfaceMulticastAddressType,
};
use prns_core::interfaces::bluetooth_auto::GROUP_NAME as BLE_GROUP_NAME;
use prns_core::interfaces::wifi_auto::{DEFAULT_DATA_PORT, DEFAULT_DISCOVERY_PORT, GROUP_NAME};
use prns_core::interfaces::{BitrateBps, InterfaceDefaults};

use super::PlanErrorKind;
use crate::plan::rnode::RNodeTransportPlan;
use crate::plan::RNodeMultiMemberPlan;
use crate::reference::i2p::{validate_peer, validate_peers};
use crate::reference::keys::interface as interface_key;
use crate::reference::{ReferenceConfigParams, ReferenceInterface};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFamilyPreference {
    System,
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpTunnelMode {
    Direct,
    I2p,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectLimit {
    Unlimited,
    Attempts(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectTimeoutSeconds(u64);

impl ConnectTimeoutSeconds {
    pub const fn new(seconds: u64) -> Self {
        Self(seconds)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpDialPlan {
    pub host: String,
    pub port: u16,
    pub connect_timeout: ConnectTimeoutSeconds,
    pub reconnect_limit: ReconnectLimit,
    pub address_family: AddressFamilyPreference,
    pub tunnel: TcpTunnelMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TcpListenHost {
    Any,
    Address(String),
    Device(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpListenPlan {
    pub host: TcpListenHost,
    pub port: u16,
    pub address_family: AddressFamilyPreference,
    pub tunnel: TcpTunnelMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdpEndpointHost {
    Address(String),
    DeviceBroadcast(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpEndpointPlan {
    pub host: UdpEndpointHost,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdpFlowPlan {
    ReceiveOnly {
        listen: UdpEndpointPlan,
    },
    SendOnly {
        forward: UdpEndpointPlan,
    },
    Bidirectional {
        listen: UdpEndpointPlan,
        forward: UdpEndpointPlan,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialDataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialParity {
    None,
    Even,
    Odd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialStopBits {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialLinePlan {
    pub(in crate::plan) baud: u32,
    pub(in crate::plan) data_bits: SerialDataBits,
    pub(in crate::plan) parity: SerialParity,
    pub(in crate::plan) stop_bits: SerialStopBits,
}

impl SerialLinePlan {
    pub const fn baud(self) -> u32 {
        self.baud
    }

    pub const fn data_bits(self) -> SerialDataBits {
        self.data_bits
    }

    pub const fn parity(self) -> SerialParity {
        self.parity
    }

    pub const fn stop_bits(self) -> SerialStopBits {
        self.stop_bits
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyCommandFlowControl {
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationIdentificationPlan {
    pub(in crate::plan) callsign: String,
    pub(in crate::plan) interval_seconds: u64,
}

impl StationIdentificationPlan {
    pub fn callsign(&self) -> &str {
        &self.callsign
    }

    pub const fn interval_seconds(&self) -> u64 {
        self.interval_seconds
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AirtimeLimitCentiPercent(pub(in crate::plan) u16);

impl AirtimeLimitCentiPercent {
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipeRespawnDelay(pub(in crate::plan) std::time::Duration);

impl PipeRespawnDelay {
    pub const fn get(self) -> std::time::Duration {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeCommandPlan {
    pub(in crate::plan) source: String,
    pub(in crate::plan) argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketTargetPlan(String);

impl WebSocketTargetPlan {
    fn from_configured(target: String) -> Result<Self, PlanErrorKind> {
        let target = target.trim();
        if !crate::reference::supported_websocket_target(target) {
            return Err(PlanErrorKind::InvalidSetting {
                key: interface_key::TARGET,
            });
        }
        Ok(Self(target.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl PipeCommandPlan {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct I2pPeerPlan(String);

impl I2pPeerPlan {
    fn new(value: String) -> Result<Self, PlanErrorKind> {
        validate_peer(&value).map_err(|_| PlanErrorKind::InvalidSetting {
            key: interface_key::PEERS,
        })?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pPeersPlan(Vec<I2pPeerPlan>);

impl I2pPeersPlan {
    fn new(peers: Vec<String>) -> Result<Self, PlanErrorKind> {
        validate_peers(peers.iter().map(String::as_str)).map_err(|_| {
            PlanErrorKind::InvalidSetting {
                key: interface_key::PEERS,
            }
        })?;
        peers
            .into_iter()
            .map(I2pPeerPlan::new)
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    pub fn iter(&self) -> impl Iterator<Item = &I2pPeerPlan> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2pReachabilityPlan {
    OutboundOnly,
    Connectable,
}

impl I2pReachabilityPlan {
    pub const fn is_connectable(self) -> bool {
        matches!(self, Self::Connectable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoInterfaceGroupId(String);

impl AutoInterfaceGroupId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoInterfaceDiscoveryPort(u16);

impl AutoInterfaceDiscoveryPort {
    fn new(port: u16) -> Option<Self> {
        (port != 0 && port < u16::MAX).then_some(Self(port))
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    pub const fn reverse_discovery_port(self) -> u16 {
        self.0 + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoInterfaceDataPort(u16);

impl AutoInterfaceDataPort {
    fn new(port: u16) -> Option<Self> {
        (port != 0).then_some(Self(port))
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoInterfaceDevicePolicy {
    allowed: Vec<String>,
    ignored: Vec<String>,
}

impl AutoInterfaceDevicePolicy {
    pub fn allowed(&self) -> &[String] {
        &self.allowed
    }

    pub fn ignored(&self) -> &[String] {
        &self.ignored
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoInterfacePlan {
    group_id: AutoInterfaceGroupId,
    discovery_scope: AutoInterfaceDiscoveryScope,
    discovery_port: AutoInterfaceDiscoveryPort,
    data_port: AutoInterfaceDataPort,
    devices: AutoInterfaceDevicePolicy,
    multicast_address_type: AutoInterfaceMulticastAddressType,
}

impl AutoInterfacePlan {
    pub fn group_id(&self) -> &AutoInterfaceGroupId {
        &self.group_id
    }

    pub const fn discovery_scope(&self) -> AutoInterfaceDiscoveryScope {
        self.discovery_scope
    }

    pub const fn discovery_port(&self) -> AutoInterfaceDiscoveryPort {
        self.discovery_port
    }

    pub const fn data_port(&self) -> AutoInterfaceDataPort {
        self.data_port
    }

    pub const fn devices(&self) -> &AutoInterfaceDevicePolicy {
        &self.devices
    }

    pub const fn multicast_address_type(&self) -> AutoInterfaceMulticastAddressType {
        self.multicast_address_type
    }
}

/// The wire a planned interface runs on. Only mediums a host can stand up appear here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedMedium {
    /// RNS `AutoInterface`: multicast LAN discovery plus unicast peers (our `AutoWifi`).
    AutoWifi(AutoInterfacePlan),
    /// RNS `TCPClientInterface`: dial one peer.
    TcpClient {
        connection: TcpDialPlan,
        framing: TcpWireFraming,
    },
    /// RNS `TCPServerInterface`: accept peers on the configured listener.
    TcpServer {
        listener: TcpListenPlan,
        framing: TcpWireFraming,
    },
    /// RNS `UDPInterface`: receive, send, or do both over configured datagram endpoints.
    Udp {
        flow: UdpFlowPlan,
    },
    /// RNS `SerialInterface`: a configured serial device.
    Serial {
        device: String,
        line: SerialLinePlan,
    },
    /// RNS `KISSInterface`: a KISS TNC on a configured serial line, with the CSMA/timing config
    /// written to the TNC at startup (the millisecond values as the operator gave them).
    Kiss {
        device: String,
        line: SerialLinePlan,
        preamble_ms: u32,
        txtail_ms: u32,
        persistence: u8,
        slottime_ms: u32,
        flow_control: ReadyCommandFlowControl,
        station_id: Option<StationIdentificationPlan>,
    },
    /// RNS `AX25KISSInterface`: a KISS TNC carrying AX.25 UI frames, sourced from `callsign`/`ssid`.
    /// The callsign/SSID are validated before the daemon plan is constructed.
    Ax25Kiss {
        device: String,
        line: SerialLinePlan,
        preamble_ms: u32,
        txtail_ms: u32,
        persistence: u8,
        slottime_ms: u32,
        flow_control: ReadyCommandFlowControl,
        callsign: String,
        ssid: u8,
    },
    /// RNS `PipeInterface`: a subprocess `command` whose stdout/stdin carries HDLC-framed packets,
    /// respawned after the configured delay when it exits.
    Pipe {
        command: PipeCommandPlan,
        respawn_delay: PipeRespawnDelay,
    },
    /// RNS `RNodeInterface`: a LoRa RNode driven over a host byte stream, configured to a radio
    /// channel at bring-up. The radio parameters are required; the airtime locks are the wire-scaled
    /// `int(percent * 100)` values, absent when unconfigured.
    Rnode {
        transport: RNodeTransportPlan,
        frequency_hz: u64,
        bandwidth_hz: u32,
        tx_power_dbm: i16,
        spreading_factor: u8,
        coding_rate: u8,
        flow_control: ReadyCommandFlowControl,
        station_id: Option<StationIdentificationPlan>,
        airtime_limit_short: Option<AirtimeLimitCentiPercent>,
        airtime_limit_long: Option<AirtimeLimitCentiPercent>,
    },
    RnodeMulti {
        member: RNodeMultiMemberPlan,
    },
    /// RNS `BackboneInterface`: the listening end of a TCP backbone link.
    Backbone {
        listener: TcpListenPlan,
    },
    /// RNS `BackboneClientInterface`: dial one backbone peer. Wire-identical to
    /// [`TcpClient`](Self::TcpClient).
    BackboneClient {
        connection: TcpDialPlan,
    },
    I2p {
        peers: I2pPeersPlan,
        reachability: I2pReachabilityPlan,
    },
    Weave {
        device: String,
    },
    PrnsUsbAuto,
    PrnsBluetoothAuto {
        group_id: AutoInterfaceGroupId,
    },
    PrnsWebSocketClient {
        target: WebSocketTargetPlan,
        framing: WebSocketFramingSelection,
    },
    PrnsWebSocketServer {
        listener: TcpListenPlan,
        framing: WebSocketFramingSelection,
    },
}

pub(super) fn rnode_defaults(
    spreading_factor: u8,
    coding_rate: u8,
    bandwidth_hz: u32,
) -> Result<InterfaceDefaults, PlanErrorKind> {
    let raw = rnode_policy::nominal_bitrate_bps(spreading_factor, coding_rate, bandwidth_hz);
    let bitrate = BitrateBps::new(u64::from(raw)).ok_or(PlanErrorKind::InvalidSetting {
        key: interface_key::BANDWIDTH,
    })?;
    Ok(rnode_policy::defaults_for_bitrate(bitrate))
}

pub(super) fn plan_medium(interface: &ReferenceInterface) -> Result<PlannedMedium, PlanErrorKind> {
    match &interface.params {
        ReferenceConfigParams::Auto {
            group_id,
            discovery_scope,
            discovery_port,
            data_port,
            devices,
            ignored_devices,
            multicast_address_type,
        } => Ok(PlannedMedium::AutoWifi(auto_interface_plan(
            group_id,
            discovery_scope,
            *discovery_port,
            *data_port,
            devices,
            ignored_devices,
            multicast_address_type,
        )?)),
        ReferenceConfigParams::TcpClient {
            target_host,
            target_port,
            kiss_framing,
            i2p_tunneled,
            connect_timeout,
            max_reconnect_tries,
            fixed_mtu: _,
        } => {
            let host = target_host
                .clone()
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::TARGET_HOST,
                })?;
            let port = target_port.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::TARGET_PORT,
            })?;
            Ok(PlannedMedium::TcpClient {
                connection: tcp_dial_plan(
                    host,
                    port,
                    *connect_timeout,
                    *max_reconnect_tries,
                    AddressFamilyPreference::System,
                    *i2p_tunneled,
                ),
                framing: if *kiss_framing == Some(true) {
                    TcpWireFraming::Kiss
                } else {
                    TcpWireFraming::Hdlc
                },
            })
        }
        ReferenceConfigParams::TcpServer {
            listen_ip,
            listen_port,
            device,
            port,
            prefer_ipv6,
            i2p_tunneled,
            kiss_framing,
            fixed_mtu: _,
        } => {
            let listen_port = port
                .or(*listen_port)
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::LISTEN_PORT,
                })?;
            Ok(PlannedMedium::TcpServer {
                listener: TcpListenPlan {
                    host: tcp_listen_host(listen_ip, device),
                    port: listen_port,
                    address_family: preferred_ip_family(*prefer_ipv6),
                    tunnel: tunnel_mode(*i2p_tunneled),
                },
                framing: if *kiss_framing == Some(true) {
                    TcpWireFraming::Kiss
                } else {
                    TcpWireFraming::Hdlc
                },
            })
        }
        ReferenceConfigParams::Udp {
            listen_ip,
            listen_port,
            forward_ip,
            forward_port,
            device,
            port,
        } => {
            let listen = udp_endpoint(
                listen_ip.as_deref(),
                port.or(*listen_port),
                device.as_deref(),
                interface_key::LISTEN_PORT,
            )?;
            let forward = udp_endpoint(
                forward_ip.as_deref(),
                port.or(*forward_port),
                device.as_deref(),
                interface_key::FORWARD_PORT,
            )?;
            let flow = match (listen, forward) {
                (Some(listen), Some(forward)) => UdpFlowPlan::Bidirectional { listen, forward },
                (Some(listen), None) => UdpFlowPlan::ReceiveOnly { listen },
                (None, Some(forward)) => UdpFlowPlan::SendOnly { forward },
                (None, None) => {
                    return Err(PlanErrorKind::MissingRequiredField {
                        key: interface_key::LISTEN_IP,
                    })
                }
            };
            Ok(PlannedMedium::Udp { flow })
        }
        ReferenceConfigParams::Serial {
            port,
            speed,
            databits,
            parity,
            stopbits,
        } => {
            let device = port.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            Ok(PlannedMedium::Serial {
                device,
                line: serial_line(*speed, *databits, parity.as_deref(), *stopbits)?,
            })
        }
        ReferenceConfigParams::Kiss {
            port,
            speed,
            databits,
            parity,
            stopbits,
            flow_control,
            preamble,
            txtail,
            persistence,
            slottime,
            id_callsign,
            id_interval,
        } => {
            let device = port.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            Ok(PlannedMedium::Kiss {
                device,
                line: serial_line(*speed, *databits, parity.as_deref(), *stopbits)?,
                preamble_ms: preamble.unwrap_or(RNS_KISS_DEFAULT_PREAMBLE_MS),
                txtail_ms: txtail.unwrap_or(RNS_KISS_DEFAULT_TXTAIL_MS),
                persistence: persistence
                    .map(|p| p.min(u8::MAX as u32) as u8)
                    .unwrap_or(RNS_KISS_DEFAULT_PERSISTENCE),
                slottime_ms: slottime.unwrap_or(RNS_KISS_DEFAULT_SLOTTIME_MS),
                flow_control: ready_command_flow_control(*flow_control),
                station_id: station_identification(id_callsign.as_deref(), *id_interval, None)?,
            })
        }
        ReferenceConfigParams::Ax25Kiss {
            port,
            speed,
            databits,
            parity,
            stopbits,
            flow_control,
            preamble,
            txtail,
            persistence,
            slottime,
            callsign,
            ssid,
        } => {
            let device = port.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            let callsign = callsign
                .clone()
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::CALLSIGN,
                })?;
            let ssid = ssid.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::SSID,
            })?;
            Ok(PlannedMedium::Ax25Kiss {
                device,
                line: serial_line(*speed, *databits, parity.as_deref(), *stopbits)?,
                preamble_ms: preamble.unwrap_or(RNS_KISS_DEFAULT_PREAMBLE_MS),
                txtail_ms: txtail.unwrap_or(RNS_KISS_DEFAULT_TXTAIL_MS),
                persistence: persistence
                    .map(|p| p.min(u8::MAX as u32) as u8)
                    .unwrap_or(RNS_KISS_DEFAULT_PERSISTENCE),
                slottime_ms: slottime.unwrap_or(RNS_KISS_DEFAULT_SLOTTIME_MS),
                flow_control: ready_command_flow_control(*flow_control),
                callsign,
                ssid,
            })
        }
        ReferenceConfigParams::Rnode {
            port,
            radio,
            flow_control,
            id_callsign,
            id_interval,
            airtime_limit_short,
            airtime_limit_long,
        } => {
            let configured_port = port.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            let transport = RNodeTransportPlan::from_configured_port(configured_port)?;
            let frequency_hz = radio.frequency.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::FREQUENCY,
            })?;
            let bandwidth_hz = radio.bandwidth.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::BANDWIDTH,
            })?;
            let spreading_factor =
                radio
                    .spreadingfactor
                    .ok_or(PlanErrorKind::MissingRequiredField {
                        key: interface_key::SPREADINGFACTOR,
                    })?;
            let coding_rate = radio
                .codingrate
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::CODINGRATE,
                })?;
            let tx_power_dbm = radio.txpower.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::TXPOWER,
            })?;
            Ok(PlannedMedium::Rnode {
                transport,
                frequency_hz,
                bandwidth_hz,
                tx_power_dbm,
                spreading_factor,
                coding_rate,
                flow_control: ready_command_flow_control(*flow_control),
                station_id: station_identification(id_callsign.as_deref(), *id_interval, Some(32))?,
                airtime_limit_short: airtime_limit(
                    *airtime_limit_short,
                    interface_key::AIRTIME_LIMIT_SHORT,
                )?,
                airtime_limit_long: airtime_limit(
                    *airtime_limit_long,
                    interface_key::AIRTIME_LIMIT_LONG,
                )?,
            })
        }
        ReferenceConfigParams::Pipe {
            command,
            respawn_delay,
        } => {
            let command = command
                .as_deref()
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::COMMAND,
                })?;
            Ok(PlannedMedium::Pipe {
                command: pipe_command(command)?,
                respawn_delay: pipe_respawn_delay(*respawn_delay)?,
            })
        }
        ReferenceConfigParams::Backbone {
            listen_ip,
            listen_port,
            target_host,
            target_port,
            port,
            device,
            prefer_ipv6,
            i2p_tunneled,
            connect_timeout,
            max_reconnect_tries,
        } => {
            if target_host.is_some() || interface.type_name == "BackboneClientInterface" {
                let host = target_host
                    .clone()
                    .ok_or(PlanErrorKind::MissingRequiredField {
                        key: interface_key::TARGET_HOST,
                    })?;
                let port = port
                    .or(*target_port)
                    .ok_or(PlanErrorKind::MissingRequiredField {
                        key: interface_key::TARGET_PORT,
                    })?;
                Ok(PlannedMedium::BackboneClient {
                    connection: tcp_dial_plan(
                        host,
                        port,
                        *connect_timeout,
                        *max_reconnect_tries,
                        preferred_ip_family(*prefer_ipv6),
                        *i2p_tunneled,
                    ),
                })
            } else {
                let bind_port =
                    (*port)
                        .or(*listen_port)
                        .ok_or(PlanErrorKind::MissingRequiredField {
                            key: interface_key::LISTEN_PORT,
                        })?;
                Ok(PlannedMedium::Backbone {
                    listener: TcpListenPlan {
                        host: tcp_listen_host(listen_ip, device),
                        port: bind_port,
                        address_family: preferred_ip_family(*prefer_ipv6),
                        tunnel: TcpTunnelMode::Direct,
                    },
                })
            }
        }
        ReferenceConfigParams::I2p { peers, connectable } => Ok(PlannedMedium::I2p {
            peers: I2pPeersPlan::new(peers.clone().unwrap_or_default())?,
            reachability: if *connectable == Some(true) {
                I2pReachabilityPlan::Connectable
            } else {
                I2pReachabilityPlan::OutboundOnly
            },
        }),
        ReferenceConfigParams::Weave { port } => Ok(PlannedMedium::Weave {
            device: port.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::PORT,
            })?,
        }),
        ReferenceConfigParams::PrnsUsbAuto => Ok(PlannedMedium::PrnsUsbAuto),
        ReferenceConfigParams::PrnsBluetoothAuto { group_id } => {
            Ok(PlannedMedium::PrnsBluetoothAuto {
                group_id: AutoInterfaceGroupId(
                    group_id
                        .clone()
                        .unwrap_or_else(|| BLE_GROUP_NAME.to_string()),
                ),
            })
        }
        ReferenceConfigParams::PrnsWebSocketClient { target, framing } => {
            let target = target.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::TARGET,
            })?;
            Ok(PlannedMedium::PrnsWebSocketClient {
                target: WebSocketTargetPlan::from_configured(target)?,
                framing: websocket_framing_selection(framing.as_deref())?,
            })
        }
        ReferenceConfigParams::PrnsWebSocketServer {
            listen_ip,
            listen_port,
            device,
            port,
            prefer_ipv6,
            framing,
        } => {
            let port = port
                .or(*listen_port)
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::LISTEN_PORT,
                })?;
            Ok(PlannedMedium::PrnsWebSocketServer {
                listener: TcpListenPlan {
                    host: tcp_listen_host(listen_ip, device),
                    port,
                    address_family: preferred_ip_family(*prefer_ipv6),
                    tunnel: TcpTunnelMode::Direct,
                },
                framing: websocket_framing_selection(framing.as_deref())?,
            })
        }
        _ => Err(PlanErrorKind::UnsupportedKind),
    }
}

fn websocket_framing_selection(
    framing: Option<&str>,
) -> Result<WebSocketFramingSelection, PlanErrorKind> {
    let Some(framing) = framing else {
        return Ok(WebSocketFramingSelection::Auto);
    };
    WebSocketFramingSelection::from_name(framing.trim()).map_err(|_| {
        PlanErrorKind::InvalidSetting {
            key: interface_key::FRAMING,
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn auto_interface_plan(
    group_id: &Option<String>,
    discovery_scope: &Option<String>,
    discovery_port: Option<u16>,
    data_port: Option<u16>,
    devices: &Option<Vec<String>>,
    ignored_devices: &Option<Vec<String>>,
    multicast_address_type: &Option<String>,
) -> Result<AutoInterfacePlan, PlanErrorKind> {
    let discovery_scope =
        discovery_scope
            .as_deref()
            .map_or(Ok(AutoInterfaceDiscoveryScope::Link), |value| {
                AutoInterfaceDiscoveryScope::from_name(value.trim()).ok_or(
                    PlanErrorKind::InvalidSetting {
                        key: interface_key::DISCOVERY_SCOPE,
                    },
                )
            })?;
    let multicast_address_type = multicast_address_type.as_deref().map_or(
        Ok(AutoInterfaceMulticastAddressType::Temporary),
        |value| {
            AutoInterfaceMulticastAddressType::from_name(value.trim()).ok_or(
                PlanErrorKind::InvalidSetting {
                    key: interface_key::MULTICAST_ADDRESS_TYPE,
                },
            )
        },
    )?;
    let discovery_port = AutoInterfaceDiscoveryPort::new(
        discovery_port.unwrap_or(DEFAULT_DISCOVERY_PORT),
    )
    .ok_or(PlanErrorKind::InvalidSetting {
        key: interface_key::DISCOVERY_PORT,
    })?;
    let data_port = AutoInterfaceDataPort::new(data_port.unwrap_or(DEFAULT_DATA_PORT)).ok_or(
        PlanErrorKind::InvalidSetting {
            key: interface_key::DATA_PORT,
        },
    )?;
    Ok(AutoInterfacePlan {
        group_id: AutoInterfaceGroupId(group_id.clone().unwrap_or_else(|| GROUP_NAME.to_string())),
        discovery_scope,
        discovery_port,
        data_port,
        devices: AutoInterfaceDevicePolicy {
            allowed: devices.clone().unwrap_or_default(),
            ignored: ignored_devices.clone().unwrap_or_default(),
        },
        multicast_address_type,
    })
}

fn tcp_dial_plan(
    host: String,
    port: u16,
    connect_timeout_seconds: Option<u64>,
    max_reconnect_tries: Option<u32>,
    address_family: AddressFamilyPreference,
    i2p_tunneled: Option<bool>,
) -> TcpDialPlan {
    TcpDialPlan {
        host,
        port,
        connect_timeout: ConnectTimeoutSeconds::new(
            connect_timeout_seconds.unwrap_or(RNS_TCP_CONNECT_TIMEOUT_SECONDS),
        ),
        reconnect_limit: max_reconnect_tries
            .map(ReconnectLimit::Attempts)
            .unwrap_or(ReconnectLimit::Unlimited),
        address_family,
        tunnel: tunnel_mode(i2p_tunneled),
    }
}

fn tcp_listen_host(listen_ip: &Option<String>, device: &Option<String>) -> TcpListenHost {
    match (device, listen_ip) {
        (Some(device), _) => TcpListenHost::Device(device.clone()),
        (None, Some(address)) => TcpListenHost::Address(address.clone()),
        (None, None) => TcpListenHost::Any,
    }
}

const fn preferred_ip_family(prefer_ipv6: Option<bool>) -> AddressFamilyPreference {
    match prefer_ipv6 {
        Some(true) => AddressFamilyPreference::Ipv6,
        Some(false) | None => AddressFamilyPreference::Ipv4,
    }
}

const fn tunnel_mode(i2p_tunneled: Option<bool>) -> TcpTunnelMode {
    match i2p_tunneled {
        Some(true) => TcpTunnelMode::I2p,
        Some(false) | None => TcpTunnelMode::Direct,
    }
}

fn udp_endpoint(
    address: Option<&str>,
    port: Option<u16>,
    device: Option<&str>,
    port_key: &'static str,
) -> Result<Option<UdpEndpointPlan>, PlanErrorKind> {
    if address.is_none() && port.is_none() {
        return Ok(None);
    }
    let host = match (address, device) {
        (Some(address), _) => Some(UdpEndpointHost::Address(address.to_string())),
        (None, Some(device)) => Some(UdpEndpointHost::DeviceBroadcast(device.to_string())),
        (None, None) => None,
    };
    match (host, port) {
        (Some(host), Some(port)) => Ok(Some(UdpEndpointPlan { host, port })),
        (Some(_), None) => Err(PlanErrorKind::MissingRequiredField { key: port_key }),
        (None, _) => Ok(None),
    }
}

pub(in crate::plan) const RNS_DEFAULT_SERIAL_BAUD: u32 = 9_600;
const RNS_TCP_CONNECT_TIMEOUT_SECONDS: u64 = 5;

/// RNS `KISSInterface` TNC defaults, mirrored from `interfaces::kiss` (kept in this crate so
/// the config planner stays independent of the interface crate): 350 ms preamble, 20 ms TX-tail,
/// persistence 64, 20 ms slot time.
const RNS_KISS_DEFAULT_PREAMBLE_MS: u32 = 350;
const RNS_KISS_DEFAULT_TXTAIL_MS: u32 = 20;
const RNS_KISS_DEFAULT_PERSISTENCE: u8 = 64;
const RNS_KISS_DEFAULT_SLOTTIME_MS: u32 = 20;

const RNS_PIPE_DEFAULT_RESPAWN_SECONDS: u64 = 5;

fn serial_line(
    speed: Option<u32>,
    data_bits: Option<u8>,
    parity: Option<&str>,
    stop_bits: Option<u8>,
) -> Result<SerialLinePlan, PlanErrorKind> {
    let baud = speed.unwrap_or(RNS_DEFAULT_SERIAL_BAUD);
    if u64::from(baud) < BitrateBps::MINIMUM {
        return Err(PlanErrorKind::InvalidSetting {
            key: interface_key::SPEED,
        });
    }
    let data_bits = match data_bits.unwrap_or(8) {
        5 => SerialDataBits::Five,
        6 => SerialDataBits::Six,
        7 => SerialDataBits::Seven,
        8 => SerialDataBits::Eight,
        _ => {
            return Err(PlanErrorKind::InvalidSetting {
                key: interface_key::DATABITS,
            })
        }
    };
    let parity = match parity.unwrap_or("n").trim().to_ascii_lowercase().as_str() {
        "n" | "none" => SerialParity::None,
        "e" | "even" => SerialParity::Even,
        "o" | "odd" => SerialParity::Odd,
        _ => {
            return Err(PlanErrorKind::InvalidSetting {
                key: interface_key::PARITY,
            })
        }
    };
    let stop_bits = match stop_bits.unwrap_or(1) {
        1 => SerialStopBits::One,
        2 => SerialStopBits::Two,
        _ => {
            return Err(PlanErrorKind::InvalidSetting {
                key: interface_key::STOPBITS,
            })
        }
    };
    Ok(SerialLinePlan {
        baud,
        data_bits,
        parity,
        stop_bits,
    })
}

pub(in crate::plan) fn ready_command_flow_control(
    configured: Option<bool>,
) -> ReadyCommandFlowControl {
    match configured {
        Some(true) => ReadyCommandFlowControl::Enabled,
        Some(false) | None => ReadyCommandFlowControl::Disabled,
    }
}

pub(in crate::plan) fn station_identification(
    callsign: Option<&str>,
    interval_seconds: Option<u64>,
    maximum_callsign_bytes: Option<usize>,
) -> Result<Option<StationIdentificationPlan>, PlanErrorKind> {
    let (callsign, interval_seconds) = match (callsign, interval_seconds) {
        (None, None) => return Ok(None),
        (Some(_), None) => {
            return Err(PlanErrorKind::MissingRequiredField {
                key: interface_key::ID_INTERVAL,
            })
        }
        (None, Some(_)) => {
            return Err(PlanErrorKind::MissingRequiredField {
                key: interface_key::ID_CALLSIGN,
            })
        }
        (Some(callsign), Some(interval_seconds)) => (callsign, interval_seconds),
    };
    if callsign.is_empty() || maximum_callsign_bytes.is_some_and(|maximum| callsign.len() > maximum)
    {
        return Err(PlanErrorKind::InvalidSetting {
            key: interface_key::ID_CALLSIGN,
        });
    }
    Ok(Some(StationIdentificationPlan {
        callsign: callsign.to_string(),
        interval_seconds,
    }))
}

pub(in crate::plan) fn airtime_limit(
    percent: Option<f64>,
    key: &'static str,
) -> Result<Option<AirtimeLimitCentiPercent>, PlanErrorKind> {
    let Some(percent) = percent else {
        return Ok(None);
    };
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err(PlanErrorKind::InvalidSetting { key });
    }
    Ok(Some(AirtimeLimitCentiPercent((percent * 100.0) as u16)))
}

fn pipe_respawn_delay(seconds: Option<f64>) -> Result<PipeRespawnDelay, PlanErrorKind> {
    let duration = match seconds {
        Some(seconds) => {
            Duration::try_from_secs_f64(seconds).map_err(|_| PlanErrorKind::InvalidSetting {
                key: interface_key::RESPAWN_DELAY,
            })?
        }
        None => Duration::from_secs(RNS_PIPE_DEFAULT_RESPAWN_SECONDS),
    };
    Ok(PipeRespawnDelay(duration))
}

fn pipe_command(source: &str) -> Result<PipeCommandPlan, PlanErrorKind> {
    let argv = shlex::split(source).filter(|argv| !argv.is_empty()).ok_or(
        PlanErrorKind::InvalidSetting {
            key: interface_key::COMMAND,
        },
    )?;
    Ok(PipeCommandPlan {
        source: source.to_string(),
        argv,
    })
}
