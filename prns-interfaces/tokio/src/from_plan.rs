//! Stand up a [`DaemonPlan`]'s interfaces on a running node — the library side of
//! "read the interfaces from a stock RNS config". Construction lives here; each outcome
//! is reported through the caller's callback ([`PlanOutcome`]), so a daemon renders its
//! own lines.

use core::time::Duration;

pub use prns_config as config;
#[cfg(feature = "tracing")]
use prns_config::DeferReason;
use prns_config::{
    DaemonPlan, DeferredInterface, InterfaceAccessPlan, PlannedInterface, PlannedMedium,
};
use prns_core::interfaces::ifac::IfacContext;
use prns_core::interfaces::{BitrateBps, InterfaceId, InterfaceOriginKind};
use prns_runtime::interfaces::backbone::core as backbone_core;
use prns_runtime::interfaces::kiss::core::TncConfig;
use prns_runtime::interfaces::rnode::core::RadioConfig;
use prns_runtime::interfaces::tcp::core as tcp_core;
use prns_runtime::runtime::{AttachIntent, Attachable, TokioPrnsHandle};

use crate::ax25::Ax25KissInterface;
use crate::backbone::client::BackboneClientInterface;
use crate::backbone::server::BackboneServer;
use crate::kiss::{KissInterface, CONFIGURE_SETTLE};
use crate::pipe::PipeInterface;
use crate::rnode::RNodeInterface;
use crate::serial::SerialInterface;
use crate::serial_host::open_host_serial;
use crate::tcp::client::TcpClientInterface;
use crate::tcp::server::TcpServer;
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
            let wifi = match interface.bitrate_bps.and_then(BitrateBps::new) {
                Some(bitrate) => AutoWifi::with_bitrate(bitrate),
                None => {
                    warn_if_below_floor(interface);
                    AutoWifi::default()
                }
            };
            let attached = attach_with_access(handle, access, wifi);
            report_up(handle, interface, attached.id(), report);
        }
        PlannedMedium::TcpClient {
            host,
            port,
            framing,
        } => {
            let attached = attach_with_access(
                handle,
                access,
                TcpClientInterface::with_framing(
                    format!("{host}:{port}"),
                    bitrate(interface),
                    TCP_RECONNECT,
                    *framing,
                ),
            );
            report_up(handle, interface, attached.id(), report);
        }
        PlannedMedium::TcpServer { bind } => {
            match TcpServer::bind(bind.clone(), bitrate(interface)).await {
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
        PlannedMedium::Udp { listen, forward } => {
            match UdpInterface::bind(listen.clone(), forward.clone(), bitrate(interface)).await {
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
            let serial = SerialInterface::new(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial(&open_path, baud) }
                },
                SERIAL_RECONNECT,
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
            let kiss = KissInterface::with_settings(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial(&open_path, baud) }
                },
                SERIAL_RECONNECT,
                CONFIGURE_SETTLE,
                tnc,
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
            let opened = Ax25KissInterface::with_settings(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial(&open_path, baud) }
                },
                SERIAL_RECONNECT,
                CONFIGURE_SETTLE,
                tnc,
                callsign,
                *ssid,
                device.as_bytes(),
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
                    let rnode = RNodeInterface::new(
                        move || {
                            let open_path = open_path.clone();
                            async move { open_host_serial(&open_path, RNODE_BAUD) }
                        },
                        RNODE_RECONNECT,
                        radio,
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
        PlannedMedium::Backbone { bind } => {
            // Wire-identical to a TCP server, under the Backbone kind. The listener's default pipe
            // claim is the reference's gigabit `BackboneInterface.BITRATE_GUESS`.
            let bitrate = resolve_bitrate(interface, backbone_core::BACKBONE_BITRATE_GUESS_BPS);
            match BackboneServer::bind(bind.clone(), bitrate).await {
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
        PlannedMedium::BackboneClient { host, port } => {
            // Wire-identical to a TCP client, under the Backbone kind. The connector's default pipe
            // claim is the reference's 100 Mbps `BackboneClientInterface.BITRATE_GUESS`.
            let bitrate =
                resolve_bitrate(interface, backbone_core::BACKBONE_CLIENT_BITRATE_GUESS_BPS);
            let attached = attach_with_access(
                handle,
                access,
                BackboneClientInterface::new(format!("{host}:{port}"), bitrate, TCP_RECONNECT),
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
                    let pipe = PipeInterface::new(
                        move || {
                            let argv = argv.clone();
                            async move { crate::pipe_host::spawn(&argv).await }
                        },
                        respawn,
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

fn bitrate(interface: &PlannedInterface) -> BitrateBps {
    resolve_bitrate(interface, tcp_core::TCP_BITRATE_GUESS_BPS)
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

fn resolve_bitrate(interface: &PlannedInterface, default: BitrateBps) -> BitrateBps {
    match interface.bitrate_bps {
        Some(raw) => BitrateBps::new(raw).unwrap_or_else(|| {
            crate::diagnostic_log::warn!(
                "interface {} configured bitrate {raw} bps is below the {}-bps minimum; using the default {} bps",
                interface.name,
                BitrateBps::MINIMUM,
                default.get(),
            );
            default
        }),
        None => default,
    }
}

fn warn_if_below_floor(interface: &PlannedInterface) {
    if let Some(raw) = interface.bitrate_bps {
        crate::diagnostic_log::warn!(
            "interface {} configured bitrate {raw} bps is below the {}-bps minimum; using the medium default",
            interface.name,
            BitrateBps::MINIMUM,
        );
    }
}
