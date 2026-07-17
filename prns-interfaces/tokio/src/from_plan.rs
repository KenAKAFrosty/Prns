//! Stand up a [`DaemonPlan`]'s interfaces on a running node — the library side of
//! "read the interfaces from a stock RNS config". Construction lives here; each outcome
//! is reported through the caller's callback ([`PlanOutcome`]), so a daemon renders its
//! own lines.

use core::time::Duration;

pub use prns_config as config;
use prns_config::{
    AddressFamilyPreference as PlannedAddressFamilyPreference, DaemonPlan, InterfaceAccessPlan,
    PlannedInterface, PlannedMedium, ReadyCommandFlowControl as PlannedReadyCommandFlowControl,
    ReconnectLimit as PlannedReconnectLimit, SerialDataBits, SerialLinePlan, SerialParity,
    SerialStopBits, StationIdentificationPlan, TcpDialPlan, TcpTunnelMode as PlannedTcpTunnelMode,
    UdpFlowPlan,
};
use prns_core::interfaces::ifac::IfacContext;
use prns_core::interfaces::{InterfaceId, InterfaceOriginKind};
use prns_runtime::interfaces::kiss::core::TncConfig;
use prns_runtime::interfaces::rnode::core::{RadioConfig, RadioConfigInput};
use prns_runtime::runtime::{AttachIntent, Attachable, PrnsNodeHandle};

use crate::ax25::{Ax25KissInterface, Ax25KissSettings};
use crate::backbone::client::BackboneClientInterface;
use crate::backbone::server::BackboneServer;
use crate::host_network::{
    resolve_tcp_listener, resolve_udp_endpoint, tcp_target, udp_ephemeral_bind,
};
use crate::kiss::{KissInterface, KissSettings, DEFAULT_TNC_CONFIGURE_DELAY};
use crate::pipe::{PipeInterface, PipeRespawnDelay};
use crate::reconnect::ReconnectDelay;
use crate::rnode::{RNodeInterface, RNodeSettings};
use crate::serial::SerialInterface;
use crate::serial_control::{ReadyCommandFlowControl, StationIdentification};
use crate::serial_control::{ReadyTimeout, StationIdInterval, StationIdWireFormat};
use crate::serial_host::{
    open_host_serial, open_host_serial_with_settings, HostSerialDataBits, HostSerialLineSettings,
    HostSerialParity, HostSerialStopBits,
};
use crate::tcp::client::TcpClientInterface;
use crate::tcp::server::TcpServer;
use crate::tcp::tokio_socket::{
    AddressFamilyPreference, ReconnectLimit, TcpConnectionSettings, TcpTunnelMode,
};
use crate::udp::UdpInterface;
use crate::wifi::AutoWifi;

const TCP_RECONNECT_DELAY: Duration = Duration::from_secs(5);
const SERIAL_RECONNECT_DELAY: ReconnectDelay = ReconnectDelay::new(Duration::from_millis(500));
const KISS_FLOW_CONTROL_TIMEOUT: ReadyTimeout = ReadyTimeout::new(Duration::from_secs(5));
/// RNS `RNodeInterface.RECONNECT_WAIT`: an RNode's bring-up handshake is expensive (settle, detect,
/// configure, validate), so a dropped or absent device is retried on a slower cadence than a bare
/// serial port.
const RNODE_RECONNECT_DELAY: ReconnectDelay = ReconnectDelay::new(Duration::from_secs(5));
/// An RNode's host link is always 115200/8N1 — RNS hardcodes the speed; it is not a config knob.
const RNODE_BAUD: u32 = 115_200;

/// What became of one planned item, handed to the caller's reporter as it happens.
pub enum PlanOutcome<'a> {
    Up {
        interface: &'a PlannedInterface,
        id: InterfaceId,
    },
    Failed {
        interface: &'a PlannedInterface,
        visible_error_message: String,
    },
}

/// The recipe intent for a config-driven node. Construction awaits socket binds, so it rides its
/// own task off `PrnsNode::new`.
pub struct FromPlan(pub DaemonPlan);

impl AttachIntent for FromPlan {
    fn attach(self, handle: &PrnsNodeHandle) {
        let handle = handle.clone();
        let plan = self.0;
        tokio::spawn(async move {
            attach_plan(&handle, &plan, &mut |outcome| match outcome {
                PlanOutcome::Up { interface, .. } => {
                    #[cfg(feature = "tracing")]
                    {
                        tracing::info!(
                            target: "prns.interface",
                            event = "interface_configured",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            medium = planned_medium_name(&interface.medium),
                        );
                        tracing::debug!(
                            target: "prns.interface",
                            event = "interface_configured_detail",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            interface_name = ?interface.name,
                            medium = ?interface.medium,
                        );
                    }
                    #[cfg(not(feature = "tracing"))]
                    crate::diagnostic_log::info!(
                        "interface up [{}]: {:?} ({:?})",
                        InterfaceOriginKind::Configured.as_str(),
                        interface.name,
                        interface.medium
                    );
                }
                PlanOutcome::Failed {
                    interface,
                    visible_error_message,
                } => {
                    #[cfg(feature = "tracing")]
                    {
                        tracing::warn!(
                            target: "prns.interface",
                            event = "interface_configuration_failed",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            medium = planned_medium_name(&interface.medium),
                        );
                        tracing::debug!(
                            target: "prns.interface",
                            event = "interface_configuration_failed_detail",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            interface_name = ?interface.name,
                            medium = ?interface.medium,
                            error = %visible_error_message,
                        );
                    }
                    #[cfg(not(feature = "tracing"))]
                    crate::diagnostic_log::warn!(
                        "interface failed [{}]: {:?} ({visible_error_message})",
                        InterfaceOriginKind::Configured.as_str(),
                        interface.name
                    );
                }
            })
            .await;
        });
    }
}

/// Stand up every planned interface on `handle`, reporting each outcome. The runtime tracks
/// attached interfaces' statuses itself, so nothing is returned to hold.
pub async fn attach_plan(
    handle: &PrnsNodeHandle,
    plan: &DaemonPlan,
    report: &mut impl FnMut(PlanOutcome<'_>),
) {
    for interface in &plan.interfaces {
        stand_up(handle, interface, report).await;
    }
}

async fn stand_up(
    handle: &PrnsNodeHandle,
    interface: &PlannedInterface,
    report: &mut impl FnMut(PlanOutcome<'_>),
) {
    let access = match &interface.access {
        InterfaceAccessPlan::Open => None,
        InterfaceAccessPlan::Ifac {
            network_name,
            passphrase,
            size,
        } => match IfacContext::derive(network_name.as_deref(), passphrase.as_deref(), *size) {
            Some(context) => Some((context, network_name.clone())),
            None => {
                report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: "IFAC requires a network name or passphrase".to_string(),
                });
                return;
            }
        },
    };
    match &interface.medium {
        PlannedMedium::AutoWifi { .. } => {
            let wifi = AutoWifi::with_policy(interface.policy);
            let attached = attach_with_access(handle, access, wifi);
            report_up(handle, interface, attached.id(), report);
        }
        PlannedMedium::TcpClient {
            connection,
            framing,
        } => {
            let attached = attach_with_access(
                handle,
                access,
                TcpClientInterface::with_policy_and_connection_settings(
                    tcp_target(connection),
                    interface.policy,
                    *framing,
                    tcp_connection_settings(connection),
                ),
            );
            report_up(handle, interface, attached.id(), report);
        }
        PlannedMedium::TcpServer { listener, framing } => {
            let resolved = resolve_tcp_listener(listener).await;
            let opened = match resolved {
                Ok(bind) => {
                    TcpServer::bind_with_policy_and_tunnel_and_framing(
                        bind,
                        interface.policy,
                        tcp_tunnel_mode(listener.tunnel),
                        *framing,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            match opened {
                Ok(server) => {
                    let attached = attach_with_access(handle, access, server);
                    report_up(handle, interface, attached.id(), report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: error.to_string(),
                }),
            }
        }
        PlannedMedium::Udp { flow } => {
            let opened = match flow {
                UdpFlowPlan::ReceiveOnly { listen } => match resolve_udp_endpoint(listen).await {
                    Ok(listen) => {
                        UdpInterface::bind_receive_with_policy(listen, interface.policy).await
                    }
                    Err(error) => Err(error),
                },
                UdpFlowPlan::SendOnly { forward } => match resolve_udp_endpoint(forward).await {
                    Ok(forward) => {
                        UdpInterface::bind_send_with_policy(
                            udp_ephemeral_bind(),
                            forward,
                            interface.policy,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                },
                UdpFlowPlan::Bidirectional { listen, forward } => {
                    match (
                        resolve_udp_endpoint(listen).await,
                        resolve_udp_endpoint(forward).await,
                    ) {
                        (Ok(listen), Ok(forward)) => {
                            UdpInterface::bind_with_policy(listen, forward, interface.policy).await
                        }
                        (Err(error), _) | (_, Err(error)) => Err(error),
                    }
                }
            };
            match opened {
                Ok(udp) => {
                    let attached = attach_with_access(handle, access, udp);
                    report_up(handle, interface, attached.id(), report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: error.to_string(),
                }),
            }
        }
        PlannedMedium::Serial { device, line } => {
            let line = host_serial_line(*line);
            let open_path = device.clone();
            let serial = SerialInterface::with_policy(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial_with_settings(&open_path, line) }
                },
                SERIAL_RECONNECT_DELAY,
                interface.policy,
                device.as_bytes(),
            );
            let attached = attach_with_access(handle, access, serial);
            report_up(handle, interface, attached.id(), report);
        }
        PlannedMedium::Kiss {
            device,
            line,
            preamble_ms,
            txtail_ms,
            persistence,
            slottime_ms,
            flow_control,
            station_id,
        } => {
            let line = host_serial_line(*line);
            let open_path = device.clone();
            let tnc = TncConfig {
                preamble_ms: *preamble_ms,
                txtail_ms: *txtail_ms,
                persistence: *persistence,
                slottime_ms: *slottime_ms,
            };
            let station_identification =
                match runtime_station_identification(station_id, StationIdWireFormat::KissPadded) {
                    Ok(station_identification) => station_identification,
                    Err(message) => {
                        report(PlanOutcome::Failed {
                            interface,
                            visible_error_message: message,
                        });
                        return;
                    }
                };
            let kiss = KissInterface::with_runtime_settings(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial_with_settings(&open_path, line) }
                },
                SERIAL_RECONNECT_DELAY,
                KissSettings {
                    configure_delay: DEFAULT_TNC_CONFIGURE_DELAY,
                    tnc,
                    flow_control: runtime_kiss_flow_control(*flow_control),
                    station_identification,
                    policy: interface.policy,
                    channel_tag: device.as_bytes(),
                },
            );
            let attached = attach_with_access(handle, access, kiss);
            report_up(handle, interface, attached.id(), report);
        }
        PlannedMedium::Ax25Kiss {
            device,
            line,
            preamble_ms,
            txtail_ms,
            persistence,
            slottime_ms,
            flow_control,
            callsign,
            ssid,
        } => {
            let line = host_serial_line(*line);
            let open_path = device.clone();
            let tnc = TncConfig {
                preamble_ms: *preamble_ms,
                txtail_ms: *txtail_ms,
                persistence: *persistence,
                slottime_ms: *slottime_ms,
            };
            let opened = Ax25KissInterface::with_policy(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial_with_settings(&open_path, line) }
                },
                SERIAL_RECONNECT_DELAY,
                Ax25KissSettings {
                    configure_delay: DEFAULT_TNC_CONFIGURE_DELAY,
                    tnc,
                    flow_control: runtime_kiss_flow_control(*flow_control),
                    callsign,
                    ssid: *ssid,
                    policy: interface.policy,
                    channel_tag: device.as_bytes(),
                },
            );
            match opened {
                Ok(ax25) => {
                    let attached = attach_with_access(handle, access, ax25);
                    report_up(handle, interface, attached.id(), report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: format!("{error:?}"),
                }),
            }
        }
        PlannedMedium::Rnode {
            device,
            frequency_hz,
            bandwidth_hz,
            txpower_dbm,
            spreading_factor,
            coding_rate,
            flow_control,
            station_id,
            airtime_limit_short,
            airtime_limit_long,
        } => {
            match RadioConfig::new(RadioConfigInput {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                txpower_dbm: *txpower_dbm,
                spreading_factor: *spreading_factor,
                coding_rate: *coding_rate,
                airtime_limit_short_centi_percent: airtime_limit_short.map(|limit| limit.get()),
                airtime_limit_long_centi_percent: airtime_limit_long.map(|limit| limit.get()),
            }) {
                Ok(radio) => {
                    let station_identification = match runtime_station_identification(
                        station_id,
                        StationIdWireFormat::Exact,
                    ) {
                        Ok(station_identification) => station_identification,
                        Err(message) => {
                            report(PlanOutcome::Failed {
                                interface,
                                visible_error_message: message,
                            });
                            return;
                        }
                    };
                    let open_path = device.clone();
                    let rnode = RNodeInterface::with_runtime_settings(
                        move || {
                            let open_path = open_path.clone();
                            async move { open_host_serial(&open_path, RNODE_BAUD) }
                        },
                        RNODE_RECONNECT_DELAY,
                        RNodeSettings {
                            reset_delay: crate::rnode::DEFAULT_RNODE_RESET_DELAY,
                            radio,
                            flow_control: runtime_rnode_flow_control(*flow_control),
                            station_identification,
                            policy: interface.policy,
                            channel_tag: device.as_bytes(),
                        },
                    );
                    let attached = attach_with_access(handle, access, rnode);
                    report_up(handle, interface, attached.id(), report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: format!("{error:?}"),
                }),
            }
        }
        PlannedMedium::Backbone { listener } => {
            let opened = match resolve_tcp_listener(listener).await {
                Ok(bind) => BackboneServer::bind_with_policy(bind, interface.policy).await,
                Err(error) => Err(error),
            };
            match opened {
                Ok(server) => {
                    let attached = attach_with_access(handle, access, server);
                    report_up(handle, interface, attached.id(), report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: error.to_string(),
                }),
            }
        }
        PlannedMedium::BackboneClient { connection } => {
            let attached = attach_with_access(
                handle,
                access,
                BackboneClientInterface::with_policy_and_connection_settings(
                    tcp_target(connection),
                    interface.policy,
                    tcp_connection_settings(connection),
                ),
            );
            report_up(handle, interface, attached.id(), report);
        }
        PlannedMedium::Pipe {
            command,
            respawn_delay,
        } => {
            let respawn_delay = PipeRespawnDelay::new(respawn_delay.get());
            let argv = command.argv().to_vec();
            let pipe = PipeInterface::with_policy(
                move || {
                    let argv = argv.clone();
                    async move { crate::pipe_host::spawn(&argv).await }
                },
                respawn_delay,
                interface.policy,
                command.source().as_bytes(),
            );
            let attached = attach_with_access(handle, access, pipe);
            report_up(handle, interface, attached.id(), report);
        }
    }
}

fn host_serial_line(line: SerialLinePlan) -> HostSerialLineSettings {
    HostSerialLineSettings::new(
        line.baud(),
        match line.data_bits() {
            SerialDataBits::Five => HostSerialDataBits::Five,
            SerialDataBits::Six => HostSerialDataBits::Six,
            SerialDataBits::Seven => HostSerialDataBits::Seven,
            SerialDataBits::Eight => HostSerialDataBits::Eight,
        },
        match line.parity() {
            SerialParity::None => HostSerialParity::None,
            SerialParity::Even => HostSerialParity::Even,
            SerialParity::Odd => HostSerialParity::Odd,
        },
        match line.stop_bits() {
            SerialStopBits::One => HostSerialStopBits::One,
            SerialStopBits::Two => HostSerialStopBits::Two,
        },
    )
}

fn runtime_kiss_flow_control(planned: PlannedReadyCommandFlowControl) -> ReadyCommandFlowControl {
    match planned {
        PlannedReadyCommandFlowControl::Disabled => ReadyCommandFlowControl::Disabled,
        PlannedReadyCommandFlowControl::Enabled => {
            ReadyCommandFlowControl::WaitForReadyOrTimeout(KISS_FLOW_CONTROL_TIMEOUT)
        }
    }
}

fn runtime_rnode_flow_control(planned: PlannedReadyCommandFlowControl) -> ReadyCommandFlowControl {
    match planned {
        PlannedReadyCommandFlowControl::Disabled => ReadyCommandFlowControl::Disabled,
        PlannedReadyCommandFlowControl::Enabled => ReadyCommandFlowControl::WaitForReady,
    }
}

fn runtime_station_identification(
    planned: &Option<StationIdentificationPlan>,
    wire_format: StationIdWireFormat,
) -> Result<Option<StationIdentification>, String> {
    planned
        .as_ref()
        .map(|planned| {
            StationIdentification::new(
                planned.callsign().as_bytes(),
                StationIdInterval::new(Duration::from_secs(planned.interval_seconds())),
                wire_format,
            )
            .map_err(|_| "station identification callsign cannot be empty".to_string())
        })
        .transpose()
}

fn tcp_connection_settings(plan: &TcpDialPlan) -> TcpConnectionSettings {
    TcpConnectionSettings {
        connect_timeout: Duration::from_secs(plan.connect_timeout.get()),
        reconnect_wait: TCP_RECONNECT_DELAY,
        reconnect_limit: match plan.reconnect_limit {
            PlannedReconnectLimit::Unlimited => ReconnectLimit::Unlimited,
            PlannedReconnectLimit::Attempts(attempts) => ReconnectLimit::Attempts(attempts),
        },
        address_family: match plan.address_family {
            PlannedAddressFamilyPreference::System => AddressFamilyPreference::System,
            PlannedAddressFamilyPreference::Ipv4 => AddressFamilyPreference::Ipv4,
            PlannedAddressFamilyPreference::Ipv6 => AddressFamilyPreference::Ipv6,
        },
        tunnel: tcp_tunnel_mode(plan.tunnel),
    }
}

const fn tcp_tunnel_mode(mode: PlannedTcpTunnelMode) -> TcpTunnelMode {
    match mode {
        PlannedTcpTunnelMode::Direct => TcpTunnelMode::Direct,
        PlannedTcpTunnelMode::I2p => TcpTunnelMode::I2p,
    }
}

fn report_up<'a>(
    handle: &PrnsNodeHandle,
    interface: &'a PlannedInterface,
    id: InterfaceId,
    report: &mut impl FnMut(PlanOutcome<'a>),
) {
    let _ = handle.set_interface_name(id, interface.name.clone());
    report(PlanOutcome::Up { interface, id });
}

fn attach_with_access<A: Attachable>(
    handle: &PrnsNodeHandle,
    access: Option<(IfacContext, Option<String>)>,
    attachable: A,
) -> A::Attached {
    match access {
        None => handle.attach(attachable),
        Some((context, network_name)) => {
            handle.attach_with_ifac_name(attachable, context, network_name)
        }
    }
}

#[cfg(feature = "tracing")]
fn planned_medium_name(medium: &PlannedMedium) -> &'static str {
    match medium {
        PlannedMedium::AutoWifi { .. } => "auto_wifi",
        PlannedMedium::TcpClient { .. } => "tcp_client",
        PlannedMedium::TcpServer { .. } => "tcp_server",
        PlannedMedium::Udp { .. } => "udp",
        PlannedMedium::Serial { .. } => "serial",
        PlannedMedium::Kiss { .. } => "kiss",
        PlannedMedium::Ax25Kiss { .. } => "ax25_kiss",
        PlannedMedium::Rnode { .. } => "rnode",
        PlannedMedium::Backbone { .. } => "backbone",
        PlannedMedium::BackboneClient { .. } => "backbone_client",
        PlannedMedium::Pipe { .. } => "pipe",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_serial_line_reaches_the_host_transport_without_defaulting() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Serial]]\ntype = SerialInterface\nenabled = Yes\nport = test\nspeed = 57600\ndatabits = 7\nparity = odd\nstopbits = 2\n",
        )
        .expect("valid serial configuration")
        .value;
        let PlannedMedium::Serial { line, .. } = &plan.interfaces[0].medium else {
            panic!("serial medium expected")
        };
        let host = host_serial_line(*line);
        assert_eq!(host.baud(), 57_600);
        assert_eq!(host.data_bits(), HostSerialDataBits::Seven);
        assert_eq!(host.parity(), HostSerialParity::Odd);
        assert_eq!(host.stop_bits(), HostSerialStopBits::Two);
    }
}
