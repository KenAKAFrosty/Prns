//! Stand up a [`DaemonPlan`]'s interfaces on a running node — the library side of
//! "read the interfaces from a stock RNS config". Construction lives here; each outcome
//! is reported through the caller's callback ([`PlanOutcome`]), so a daemon renders its
//! own lines.

use core::time::Duration;

pub use prns_config as config;
#[cfg(feature = "tracing")]
use prns_config::DeferReason;
use prns_config::{
    AddressFamilyPreference as PlannedAddressFamilyPreference, DaemonPlan, DeferredInterface,
    InterfaceAccessPlan, PlannedInterface, PlannedMedium, ReconnectLimit as PlannedReconnectLimit,
    TcpDialPlan, TcpTunnelMode as PlannedTcpTunnelMode, UdpFlowPlan,
};
use prns_core::interfaces::ifac::IfacContext;
use prns_core::interfaces::{InterfaceId, InterfaceOriginKind};
use prns_runtime::interfaces::kiss::core::TncConfig;
use prns_runtime::interfaces::rnode::core::RadioConfig;
use prns_runtime::runtime::{AttachIntent, Attachable, TokioPrnsHandle};

use crate::ax25::{Ax25KissInterface, Ax25KissSettings};
use crate::backbone::client::BackboneClientInterface;
use crate::backbone::server::BackboneServer;
use crate::host_network::{
    resolve_tcp_listener, resolve_udp_endpoint, tcp_target, udp_ephemeral_bind,
};
use crate::kiss::{KissInterface, CONFIGURE_SETTLE};
use crate::pipe::PipeInterface;
use crate::rnode::RNodeInterface;
use crate::serial::SerialInterface;
use crate::serial_host::open_host_serial;
use crate::tcp::client::TcpClientInterface;
use crate::tcp::server::TcpServer;
use crate::tcp::tokio_socket::{
    AddressFamilyPreference, ReconnectLimit, TcpConnectionSettings, TcpTunnelMode,
};
use crate::udp::UdpInterface;
use crate::wifi::AutoWifi;

const TCP_RECONNECT: Duration = Duration::from_secs(5);
const SERIAL_RECONNECT: Duration = Duration::from_millis(500);
/// RNS `RNodeInterface.RECONNECT_WAIT`: an RNode's bring-up handshake is expensive (settle, detect,
/// configure, validate), so a dropped or absent device is retried on a slower cadence than a bare
/// serial port.
const RNODE_RECONNECT: Duration = Duration::from_secs(5);
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
    Unapplied(&'a PlannedInterface),
    Deferred(&'a DeferredInterface),
}

/// The recipe intent for a config-driven node. Construction awaits socket binds, so it rides its
/// own task off `Prns::new`.
pub struct FromPlan(pub DaemonPlan);

impl AttachIntent for FromPlan {
    fn attach(self, handle: &TokioPrnsHandle) {
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
                PlanOutcome::Unapplied(interface) => {
                    #[cfg(feature = "tracing")]
                    {
                        tracing::warn!(
                            target: "prns.interface",
                            event = "interface_settings_unapplied",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            setting_count = interface.unapplied.len(),
                        );
                        tracing::debug!(
                            target: "prns.interface",
                            event = "interface_settings_unapplied_detail",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            interface_name = ?interface.name,
                            settings = ?interface.unapplied,
                        );
                    }
                    #[cfg(not(feature = "tracing"))]
                    crate::diagnostic_log::warn!(
                        "settings parsed but not applied on [{}] {:?}: {:?}",
                        InterfaceOriginKind::Configured.as_str(),
                        interface.name,
                        interface.unapplied
                    );
                }
                PlanOutcome::Deferred(deferred) => {
                    #[cfg(feature = "tracing")]
                    {
                        tracing::info!(
                            target: "prns.interface",
                            event = "interface_deferred",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            reason = defer_reason_name(&deferred.why),
                        );
                        tracing::debug!(
                            target: "prns.interface",
                            event = "interface_deferred_detail",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            interface_name = ?deferred.name,
                            interface_type = ?deferred.type_name,
                            reason = ?deferred.why,
                        );
                    }
                    #[cfg(not(feature = "tracing"))]
                    crate::diagnostic_log::info!(
                        "interface deferred [{}]: {:?} ({:?})",
                        InterfaceOriginKind::Configured.as_str(),
                        deferred.name,
                        deferred.why
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
    handle: &TokioPrnsHandle,
    plan: &DaemonPlan,
    report: &mut impl FnMut(PlanOutcome<'_>),
) {
    for interface in &plan.interfaces {
        stand_up(handle, interface, report).await;
        if !interface.unapplied.is_empty() {
            report(PlanOutcome::Unapplied(interface));
        }
    }
    for deferred in &plan.deferred {
        report(PlanOutcome::Deferred(deferred));
    }
}

async fn stand_up(
    handle: &TokioPrnsHandle,
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
        PlannedMedium::TcpServer { listener } => {
            let resolved = resolve_tcp_listener(listener).await;
            let opened = match resolved {
                Ok(bind) => {
                    TcpServer::bind_with_policy_and_tunnel(
                        bind,
                        interface.policy,
                        tcp_tunnel_mode(listener.tunnel),
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
        PlannedMedium::Serial { device, baud } => {
            let baud = *baud;
            let open_path = device.clone();
            let serial = SerialInterface::with_policy(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial(&open_path, baud) }
                },
                SERIAL_RECONNECT,
                interface.policy,
                device.as_bytes(),
            );
            let attached = attach_with_access(handle, access, serial);
            report_up(handle, interface, attached.id(), report);
        }
        PlannedMedium::Kiss {
            device,
            baud,
            preamble_ms,
            txtail_ms,
            persistence,
            slottime_ms,
        } => {
            let baud = *baud;
            let open_path = device.clone();
            let tnc = TncConfig {
                preamble_ms: *preamble_ms,
                txtail_ms: *txtail_ms,
                persistence: *persistence,
                slottime_ms: *slottime_ms,
            };
            let kiss = KissInterface::with_settings_and_policy(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial(&open_path, baud) }
                },
                SERIAL_RECONNECT,
                CONFIGURE_SETTLE,
                tnc,
                interface.policy,
                device.as_bytes(),
            );
            let attached = attach_with_access(handle, access, kiss);
            report_up(handle, interface, attached.id(), report);
        }
        PlannedMedium::Ax25Kiss {
            device,
            baud,
            preamble_ms,
            txtail_ms,
            persistence,
            slottime_ms,
            callsign,
            ssid,
        } => {
            let baud = *baud;
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
                    async move { open_host_serial(&open_path, baud) }
                },
                SERIAL_RECONNECT,
                Ax25KissSettings {
                    settle: CONFIGURE_SETTLE,
                    tnc,
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
            airtime_limit_short_centi,
            airtime_limit_long_centi,
        } => {
            // Validate the radio against its operating envelope here (as RNS leaves range-checking to
            // the device): a config the radio cannot accept fails to stand up with a clear reason
            // rather than opening the port and timing out on validation.
            match RadioConfig::new(
                *frequency_hz,
                *bandwidth_hz,
                *txpower_dbm,
                *spreading_factor,
                *coding_rate,
                *airtime_limit_short_centi,
                *airtime_limit_long_centi,
            ) {
                Ok(radio) => {
                    let open_path = device.clone();
                    let rnode = RNodeInterface::new_with_policy(
                        move || {
                            let open_path = open_path.clone();
                            async move { open_host_serial(&open_path, RNODE_BAUD) }
                        },
                        RNODE_RECONNECT,
                        radio,
                        interface.policy,
                        device.as_bytes(),
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
            respawn_delay_ms,
        } => {
            let respawn = Duration::from_millis(*respawn_delay_ms);
            match shlex::split(command) {
                Some(argv) if !argv.is_empty() => {
                    let pipe = PipeInterface::with_policy(
                        move || {
                            let argv = argv.clone();
                            async move { crate::pipe_host::spawn(&argv).await }
                        },
                        respawn,
                        interface.policy,
                        command.as_bytes(),
                    );
                    let attached = attach_with_access(handle, access, pipe);
                    report_up(handle, interface, attached.id(), report);
                }
                _ => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: String::from("could not parse command into arguments"),
                }),
            }
        }
    }
}

fn tcp_connection_settings(plan: &TcpDialPlan) -> TcpConnectionSettings {
    TcpConnectionSettings {
        connect_timeout: Duration::from_secs(plan.connect_timeout.get()),
        reconnect_wait: TCP_RECONNECT,
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
    handle: &TokioPrnsHandle,
    interface: &'a PlannedInterface,
    id: InterfaceId,
    report: &mut impl FnMut(PlanOutcome<'a>),
) {
    let _ = handle.set_interface_name(id, interface.name.clone());
    report(PlanOutcome::Up { interface, id });
}

fn attach_with_access<A: Attachable>(
    handle: &TokioPrnsHandle,
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

#[cfg(feature = "tracing")]
fn defer_reason_name(reason: &DeferReason) -> &'static str {
    match reason {
        DeferReason::Disabled => "disabled",
        DeferReason::UnsupportedKind => "unsupported_kind",
        DeferReason::MissingRequiredField { .. } => "missing_required_field",
        DeferReason::InvalidSetting { .. } => "invalid_setting",
    }
}
