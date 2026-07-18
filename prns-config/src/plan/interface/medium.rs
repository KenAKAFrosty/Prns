use prns_core::interfaces::tcp::core::TcpWireFraming;

use super::{DeferReason, UnappliedSetting};
use crate::reference::keys::interface as interface_key;
use crate::reference::{ReferenceInterface, ReferenceParams};

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

/// The wire a planned interface runs on. Only mediums a host can stand up appear here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedMedium {
    /// RNS `AutoInterface`: multicast LAN discovery plus unicast peers (our `AutoWifi`).
    AutoWifi { group: Option<String> },
    /// RNS `TCPClientInterface`: dial one peer.
    TcpClient {
        connection: TcpDialPlan,
        framing: TcpWireFraming,
    },
    /// RNS `TCPServerInterface`: accept peers on the configured listener.
    TcpServer { listener: TcpListenPlan },
    /// RNS `UDPInterface`: receive, send, or do both over configured datagram endpoints.
    Udp { flow: UdpFlowPlan },
    /// RNS `SerialInterface`: a serial device at `baud`.
    Serial { device: String, baud: u32 },
    /// RNS `KISSInterface`: a KISS TNC on a serial device at `baud`, with the CSMA/timing config written to the TNC at startup (the millisecond values as the operator gave them).
    Kiss {
        device: String,
        baud: u32,
        preamble_ms: u32,
        txtail_ms: u32,
        persistence: u8,
        slottime_ms: u32,
    },
    /// RNS `AX25KISSInterface`: a KISS TNC carrying AX.25 UI frames, sourced from `callsign`/`ssid`. The callsign/SSID are validated when the interface is constructed, as RNS does.
    Ax25Kiss {
        device: String,
        baud: u32,
        preamble_ms: u32,
        txtail_ms: u32,
        persistence: u8,
        slottime_ms: u32,
        callsign: String,
        ssid: u8,
    },
    /// RNS `PipeInterface`: a subprocess `command` whose stdout/stdin carries HDLC-framed packets, respawned `respawn_delay_ms` after it exits.
    Pipe {
        command: String,
        respawn_delay_ms: u64,
    },
    /// RNS `RNodeInterface`: a LoRa RNode driven over a USB-serial KISS link, configured to a radio channel at bring-up. The radio parameters are required; the airtime locks are the wire-scaled `int(percent * 100)` values, absent when unconfigured. Range validation happens at construction (as RNS leaves it to the device's echo-back), so the plan only carries the values; an out-of-range radio fails to stand up rather than deferring.
    Rnode {
        device: String,
        frequency_hz: u64,
        bandwidth_hz: u32,
        txpower_dbm: i16,
        spreading_factor: u8,
        coding_rate: u8,
        airtime_limit_short_centi: Option<u16>,
        airtime_limit_long_centi: Option<u16>,
    },
    /// RNS `BackboneInterface`: the listening end of a TCP backbone link.
    Backbone { listener: TcpListenPlan },
    /// RNS `BackboneClientInterface`: dial one backbone peer. Wire-identical to [`TcpClient`](Self::TcpClient).
    BackboneClient { connection: TcpDialPlan },
}

pub(super) fn plan_medium(
    interface: &ReferenceInterface,
    unapplied: &mut Vec<UnappliedSetting>,
) -> Result<PlannedMedium, DeferReason> {
    match &interface.params {
        ReferenceParams::Auto {
            group_id,
            discovery_scope,
            discovery_port,
            data_port,
            devices,
            ignored_devices,
            multicast_address_type,
        } => {
            note_present(
                unapplied,
                interface_key::DISCOVERY_SCOPE,
                discovery_scope.is_some(),
            );
            note_present(
                unapplied,
                interface_key::DISCOVERY_PORT,
                discovery_port.is_some(),
            );
            note_present(unapplied, interface_key::DATA_PORT, data_port.is_some());
            note_present(unapplied, interface_key::DEVICES, devices.is_some());
            note_present(
                unapplied,
                interface_key::IGNORED_DEVICES,
                ignored_devices.is_some(),
            );
            note_present(
                unapplied,
                interface_key::MULTICAST_ADDRESS_TYPE,
                multicast_address_type.is_some(),
            );
            Ok(PlannedMedium::AutoWifi {
                group: group_id.clone(),
            })
        }
        ReferenceParams::TcpClient {
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
                .ok_or(DeferReason::MissingRequiredField {
                    key: interface_key::TARGET_HOST,
                })?;
            let port = target_port.ok_or(DeferReason::MissingRequiredField {
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
        ReferenceParams::TcpServer {
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
                .ok_or(DeferReason::MissingRequiredField {
                    key: interface_key::LISTEN_PORT,
                })?;
            note_present(
                unapplied,
                interface_key::KISS_FRAMING,
                kiss_framing.is_some(),
            );
            Ok(PlannedMedium::TcpServer {
                listener: TcpListenPlan {
                    host: tcp_listen_host(listen_ip, device),
                    port: listen_port,
                    address_family: preferred_ip_family(*prefer_ipv6),
                    tunnel: tunnel_mode(*i2p_tunneled),
                },
            })
        }
        ReferenceParams::Udp {
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
                    return Err(DeferReason::MissingRequiredField {
                        key: interface_key::LISTEN_IP,
                    })
                }
            };
            Ok(PlannedMedium::Udp { flow })
        }
        ReferenceParams::Serial {
            port,
            speed,
            databits,
            parity,
            stopbits,
        } => {
            let device = port.clone().ok_or(DeferReason::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            note_present(unapplied, interface_key::DATABITS, databits.is_some());
            note_present(unapplied, interface_key::PARITY, parity.is_some());
            note_present(unapplied, interface_key::STOPBITS, stopbits.is_some());
            Ok(PlannedMedium::Serial {
                device,
                baud: speed.unwrap_or(RNS_DEFAULT_SERIAL_BAUD),
            })
        }
        ReferenceParams::Kiss {
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
            let device = port.clone().ok_or(DeferReason::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            note_present(unapplied, interface_key::DATABITS, databits.is_some());
            note_present(unapplied, interface_key::PARITY, parity.is_some());
            note_present(unapplied, interface_key::STOPBITS, stopbits.is_some());
            note_present(
                unapplied,
                interface_key::FLOW_CONTROL,
                flow_control.is_some(),
            );
            note_present(unapplied, interface_key::ID_CALLSIGN, id_callsign.is_some());
            note_present(unapplied, interface_key::ID_INTERVAL, id_interval.is_some());
            Ok(PlannedMedium::Kiss {
                device,
                baud: speed.unwrap_or(RNS_DEFAULT_SERIAL_BAUD),
                preamble_ms: preamble.unwrap_or(RNS_KISS_DEFAULT_PREAMBLE_MS),
                txtail_ms: txtail.unwrap_or(RNS_KISS_DEFAULT_TXTAIL_MS),
                persistence: persistence
                    .map(|p| p.min(u8::MAX as u32) as u8)
                    .unwrap_or(RNS_KISS_DEFAULT_PERSISTENCE),
                slottime_ms: slottime.unwrap_or(RNS_KISS_DEFAULT_SLOTTIME_MS),
            })
        }
        ReferenceParams::Ax25Kiss {
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
            let device = port.clone().ok_or(DeferReason::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            let callsign = callsign.clone().ok_or(DeferReason::MissingRequiredField {
                key: interface_key::CALLSIGN,
            })?;
            let ssid = ssid.ok_or(DeferReason::MissingRequiredField {
                key: interface_key::SSID,
            })?;
            note_present(unapplied, interface_key::DATABITS, databits.is_some());
            note_present(unapplied, interface_key::PARITY, parity.is_some());
            note_present(unapplied, interface_key::STOPBITS, stopbits.is_some());
            note_present(
                unapplied,
                interface_key::FLOW_CONTROL,
                flow_control.is_some(),
            );
            Ok(PlannedMedium::Ax25Kiss {
                device,
                baud: speed.unwrap_or(RNS_DEFAULT_SERIAL_BAUD),
                preamble_ms: preamble.unwrap_or(RNS_KISS_DEFAULT_PREAMBLE_MS),
                txtail_ms: txtail.unwrap_or(RNS_KISS_DEFAULT_TXTAIL_MS),
                persistence: persistence
                    .map(|p| p.min(u8::MAX as u32) as u8)
                    .unwrap_or(RNS_KISS_DEFAULT_PERSISTENCE),
                slottime_ms: slottime.unwrap_or(RNS_KISS_DEFAULT_SLOTTIME_MS),
                callsign,
                ssid,
            })
        }
        ReferenceParams::Rnode {
            port,
            radio,
            flow_control,
            id_callsign,
            id_interval,
            airtime_limit_short,
            airtime_limit_long,
        } => {
            let device = port.clone().ok_or(DeferReason::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            let frequency_hz = radio.frequency.ok_or(DeferReason::MissingRequiredField {
                key: interface_key::FREQUENCY,
            })?;
            let bandwidth_hz = radio.bandwidth.ok_or(DeferReason::MissingRequiredField {
                key: interface_key::BANDWIDTH,
            })?;
            let spreading_factor =
                radio
                    .spreadingfactor
                    .ok_or(DeferReason::MissingRequiredField {
                        key: interface_key::SPREADINGFACTOR,
                    })?;
            let coding_rate = radio.codingrate.ok_or(DeferReason::MissingRequiredField {
                key: interface_key::CODINGRATE,
            })?;
            let txpower_dbm = radio.txpower.ok_or(DeferReason::MissingRequiredField {
                key: interface_key::TXPOWER,
            })?;
            note_present(
                unapplied,
                interface_key::FLOW_CONTROL,
                flow_control.is_some(),
            );
            note_present(unapplied, interface_key::ID_CALLSIGN, id_callsign.is_some());
            note_present(unapplied, interface_key::ID_INTERVAL, id_interval.is_some());
            Ok(PlannedMedium::Rnode {
                device,
                frequency_hz,
                bandwidth_hz,
                txpower_dbm,
                spreading_factor,
                coding_rate,
                airtime_limit_short_centi: airtime_limit_short.map(pct_to_centi),
                airtime_limit_long_centi: airtime_limit_long.map(pct_to_centi),
            })
        }
        ReferenceParams::Pipe {
            command,
            respawn_delay,
        } => {
            let command = command.clone().ok_or(DeferReason::MissingRequiredField {
                key: interface_key::COMMAND,
            })?;
            Ok(PlannedMedium::Pipe {
                command,
                respawn_delay_ms: respawn_delay
                    .map(|secs| (secs.max(0.0) * 1000.0) as u64)
                    .unwrap_or(RNS_PIPE_DEFAULT_RESPAWN_MS),
            })
        }
        ReferenceParams::Backbone {
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
                    .ok_or(DeferReason::MissingRequiredField {
                        key: interface_key::TARGET_HOST,
                    })?;
                let port = port
                    .or(*target_port)
                    .ok_or(DeferReason::MissingRequiredField {
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
                        .ok_or(DeferReason::MissingRequiredField {
                            key: interface_key::LISTEN_PORT,
                        })?;
                note_present(
                    unapplied,
                    interface_key::I2P_TUNNELED,
                    i2p_tunneled.is_some(),
                );
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
        _ => Err(DeferReason::UnsupportedKind),
    }
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
) -> Result<Option<UdpEndpointPlan>, DeferReason> {
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
        (Some(_), None) => Err(DeferReason::MissingRequiredField { key: port_key }),
        (None, _) => Ok(None),
    }
}

pub(in crate::plan) const RNS_DEFAULT_SERIAL_BAUD: u32 = 9_600;
const RNS_TCP_CONNECT_TIMEOUT_SECONDS: u64 = 5;

/// RNS `KISSInterface` TNC defaults, mirrored from `interfaces::kiss::core` (kept in this crate so the config planner stays independent of the interface crate): 350 ms preamble, 20 ms TX-tail, persistence 64, 20 ms slot time.
const RNS_KISS_DEFAULT_PREAMBLE_MS: u32 = 350;
const RNS_KISS_DEFAULT_TXTAIL_MS: u32 = 20;
const RNS_KISS_DEFAULT_PERSISTENCE: u8 = 64;
const RNS_KISS_DEFAULT_SLOTTIME_MS: u32 = 20;

/// RNS `PipeInterface` default respawn delay: 5 seconds.
const RNS_PIPE_DEFAULT_RESPAWN_MS: u64 = 5_000;

/// An RNode airtime-limit percentage as the wire-scaled value RNS sends: `int(percent * 100)`, clamped to the two-byte width the device command carries.
fn pct_to_centi(percent: f64) -> u16 {
    (percent.max(0.0) * 100.0).min(f64::from(u16::MAX)) as u16
}

fn note_present(unapplied: &mut Vec<UnappliedSetting>, key: &'static str, present: bool) {
    if present {
        unapplied.push(UnappliedSetting::MediumOption(key));
    }
}
