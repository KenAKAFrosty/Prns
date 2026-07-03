//! Stand up a [`DaemonPlan`]'s interfaces on a running node — the library side of
//! "read the interfaces from a stock RNS config". Construction lives here; each outcome
//! is reported through the caller's callback ([`PlanOutcome`]), so a daemon renders its
//! own lines and the [`FromPlan`] recipe intent logs through the `log` facade.

use core::time::Duration;

pub use prns_config as config;
use prns_config::{DaemonPlan, DeferredInterface, PlannedInterface, PlannedMedium};
use prns_runtime::interfaces::backbone::core as backbone_core;
use prns_runtime::interfaces::kiss::core::TncConfig;
use prns_runtime::interfaces::rnode::core::RadioConfig;
use prns_runtime::interfaces::tcp::core as tcp_core;
use prns_runtime::runtime::{AttachIntent, TokioPrnsHandle};

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
    Up(&'a PlannedInterface),
    Failed {
        interface: &'a PlannedInterface,
        visible_error_message: String,
    },
    Unapplied(&'a PlannedInterface),
    Deferred(&'a DeferredInterface),
}

/// The recipe intent for a config-driven node: stand up everything `plan` names, reporting
/// through the `log` facade. Construction awaits socket binds, so it rides its own task off
/// `Prns::new`.
pub struct FromPlan(pub DaemonPlan);

impl AttachIntent for FromPlan {
    fn attach(self, handle: &TokioPrnsHandle) {
        let handle = handle.clone();
        let plan = self.0;
        tokio::spawn(async move {
            attach_plan(&handle, &plan, &mut |outcome| match outcome {
                PlanOutcome::Up(interface) => {
                    log::info!(
                        "interface up: {:?} ({:?})",
                        interface.name,
                        interface.medium
                    );
                }
                PlanOutcome::Failed {
                    interface,
                    visible_error_message,
                } => {
                    log::warn!(
                        "interface failed: {:?} ({visible_error_message})",
                        interface.name
                    );
                }
                PlanOutcome::Unapplied(interface) => {
                    log::info!(
                        "settings parsed but not applied on {:?}: {:?}",
                        interface.name,
                        interface.unapplied
                    );
                }
                PlanOutcome::Deferred(deferred) => {
                    log::info!(
                        "interface deferred: {:?} ({:?})",
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
    match &interface.medium {
        PlannedMedium::AutoWifi { .. } => {
            let wifi = match interface.bitrate_bps {
                Some(bitrate) => AutoWifi::with_bitrate(bitrate),
                None => AutoWifi::default(),
            };
            handle.attach(wifi);
            report(PlanOutcome::Up(interface));
        }
        PlannedMedium::TcpClient { host, port } => {
            handle.attach(TcpClientInterface::new(
                format!("{host}:{port}"),
                bitrate(interface),
                TCP_RECONNECT,
            ));
            report(PlanOutcome::Up(interface));
        }
        PlannedMedium::TcpServer { bind } => {
            match TcpServer::bind(bind.clone(), bitrate(interface)).await {
                Ok(server) => {
                    handle.attach(server);
                    report(PlanOutcome::Up(interface));
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
                    handle.attach(udp);
                    report(PlanOutcome::Up(interface));
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
            handle.attach(serial);
            report(PlanOutcome::Up(interface));
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
            handle.attach(kiss);
            report(PlanOutcome::Up(interface));
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
                    handle.attach(ax25);
                    report(PlanOutcome::Up(interface));
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
                    handle.attach(rnode);
                    report(PlanOutcome::Up(interface));
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
            let bitrate = interface
                .bitrate_bps
                .unwrap_or(backbone_core::BACKBONE_BITRATE_GUESS_BPS);
            match BackboneServer::bind(bind.clone(), bitrate).await {
                Ok(server) => {
                    handle.attach(server);
                    report(PlanOutcome::Up(interface));
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
            let bitrate = interface
                .bitrate_bps
                .unwrap_or(backbone_core::BACKBONE_CLIENT_BITRATE_GUESS_BPS);
            handle.attach(BackboneClientInterface::new(
                format!("{host}:{port}"),
                bitrate,
                TCP_RECONNECT,
            ));
            report(PlanOutcome::Up(interface));
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
                    handle.attach(pipe);
                    report(PlanOutcome::Up(interface));
                }
                _ => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: String::from("could not parse command into arguments"),
                }),
            }
        }
    }
}

fn bitrate(interface: &PlannedInterface) -> u32 {
    interface
        .bitrate_bps
        .unwrap_or(tcp_core::TCP_BITRATE_GUESS_BPS)
}
