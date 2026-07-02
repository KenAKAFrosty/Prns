//! Stand up a [`DaemonPlan`]'s interfaces on a running node and report what was not applied.
//!
//! Each [`PlannedMedium`] maps to its host constructor: a supervisor (`AutoWifi`) goes through
//! [`TokioPrnsHandle::supervise`], a one-to-one wire (TCP, UDP, serial) through
//! [`TokioPrnsHandle::add_interface`]. The runtime records each interface's live status centrally as
//! it is attached, so the daemon collects nothing by hand — the shared-instance control RPC reads the
//! whole fleet straight off the handle. Settings the plan parsed but a host constructor cannot yet
//! honor, and interfaces the plan could not stand up, are logged rather than dropped silently.

use core::time::Duration;

use prns_interfaces_tokio::ax25::Ax25KissInterface;
use prns_interfaces_tokio::backbone::client::BackboneClientInterface;
use personal_rns::interfaces::backbone::core as backbone_core;
use prns_interfaces_tokio::backbone::server::BackboneServer;
use personal_rns::interfaces::kiss::core::TncConfig;
use prns_interfaces_tokio::kiss::{KissInterface, CONFIGURE_SETTLE};
use prns_interfaces_tokio::pipe::PipeInterface;
use personal_rns::interfaces::rnode::core::RadioConfig;
use prns_interfaces_tokio::rnode::RNodeInterface;
use prns_interfaces_tokio::serial::SerialInterface;
use prns_interfaces_tokio::tcp::client::TcpClientInterface;
use personal_rns::interfaces::tcp::core as tcp_core;
use prns_interfaces_tokio::tcp::server::TcpServer;
use prns_interfaces_tokio::udp::UdpInterface;
use prns_interfaces_tokio::wifi::AutoWifi;
use personal_rns::runtime::TokioPrnsHandle;
use personal_rns_config::{
    DaemonPlan, DeferReason, PlannedInterface, PlannedMedium, UnappliedSetting,
};

const TCP_RECONNECT: Duration = Duration::from_secs(5);
const SERIAL_RECONNECT: Duration = Duration::from_millis(500);
/// RNS `RNodeInterface.RECONNECT_WAIT`: an RNode's bring-up handshake is expensive (settle, detect,
/// configure, validate), so a dropped or absent device is retried on a slower cadence than a bare
/// serial port.
const RNODE_RECONNECT: Duration = Duration::from_secs(5);
/// An RNode's host link is always 115200/8N1 — RNS hardcodes the speed; it is not a config knob.
const RNODE_BAUD: u32 = 115_200;

/// Stand up every planned interface on `handle`. The runtime tracks each attached interface's status
/// itself, so nothing is returned for the caller to hold; deferred interfaces and unapplied settings
/// are logged as they are encountered.
pub async fn construct_interfaces(handle: &TokioPrnsHandle, plan: &DaemonPlan) {
    for interface in &plan.interfaces {
        stand_up(handle, interface).await;
        report_unapplied(interface);
    }
    report_deferred(plan);
}

async fn stand_up(handle: &TokioPrnsHandle, interface: &PlannedInterface) {
    let name = &interface.name;
    match &interface.medium {
        PlannedMedium::AutoWifi { .. } => {
            let wifi = match interface.bitrate_bps {
                Some(bitrate) => AutoWifi::with_bitrate(bitrate),
                None => AutoWifi::new(),
            };
            handle.supervise(wifi);
            println!("RNSD_INTERFACE_UP name={name:?} medium=auto-wifi");
        }
        PlannedMedium::TcpClient { host, port } => {
            let target = format!("{host}:{port}");
            handle.add_interface(TcpClientInterface::new(
                target.clone(),
                bitrate(interface),
                TCP_RECONNECT,
            ));
            println!("RNSD_INTERFACE_UP name={name:?} medium=tcp-client target={target}");
        }
        PlannedMedium::TcpServer { bind } => {
            match TcpServer::bind(bind.clone(), bitrate(interface)).await {
                Ok(server) => {
                    handle.supervise(server);
                    println!("RNSD_INTERFACE_UP name={name:?} medium=tcp-server bind={bind}");
                }
                Err(error) => {
                    eprintln!(
                        "RNSD_INTERFACE_FAILED name={name:?} medium=tcp-server bind={bind} error={error}"
                    );
                }
            }
        }
        PlannedMedium::Udp { listen, forward } => {
            match UdpInterface::bind(listen.clone(), forward.clone(), bitrate(interface)).await {
                Ok(udp) => {
                    handle.add_interface(udp);
                    println!(
                        "RNSD_INTERFACE_UP name={name:?} medium=udp listen={listen} forward={forward}"
                    );
                }
                Err(error) => {
                    eprintln!(
                        "RNSD_INTERFACE_FAILED name={name:?} medium=udp listen={listen} error={error}"
                    );
                }
            }
        }
        PlannedMedium::Serial { device, baud } => {
            let baud = *baud;
            let open_path = device.clone();
            let serial = SerialInterface::new(
                move || {
                    let open_path = open_path.clone();
                    async move { crate::serial::open_host_serial(&open_path, baud) }
                },
                SERIAL_RECONNECT,
                device.as_bytes(),
            );
            handle.add_interface(serial);
            println!("RNSD_INTERFACE_UP name={name:?} medium=serial device={device} baud={baud}");
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
                    async move { crate::serial::open_host_serial(&open_path, baud) }
                },
                SERIAL_RECONNECT,
                CONFIGURE_SETTLE,
                tnc,
                device.as_bytes(),
            );
            handle.add_interface(kiss);
            println!("RNSD_INTERFACE_UP name={name:?} medium=kiss device={device} baud={baud}");
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
                    async move { crate::serial::open_host_serial(&open_path, baud) }
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
                    handle.add_interface(ax25);
                    println!(
                        "RNSD_INTERFACE_UP name={name:?} medium=ax25-kiss device={device} callsign={callsign} ssid={ssid}"
                    );
                }
                Err(error) => {
                    eprintln!(
                        "RNSD_INTERFACE_FAILED name={name:?} medium=ax25-kiss callsign={callsign} error={error:?}"
                    );
                }
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
                            async move { crate::serial::open_host_serial(&open_path, RNODE_BAUD) }
                        },
                        RNODE_RECONNECT,
                        radio,
                        device.as_bytes(),
                    );
                    handle.add_interface(rnode);
                    println!(
                        "RNSD_INTERFACE_UP name={name:?} medium=rnode device={device} freq={frequency_hz} bw={bandwidth_hz} sf={spreading_factor} cr={coding_rate} txpower={txpower_dbm}"
                    );
                }
                Err(error) => {
                    eprintln!(
                        "RNSD_INTERFACE_FAILED name={name:?} medium=rnode device={device} error={error:?}"
                    );
                }
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
                    handle.supervise(server);
                    println!("RNSD_INTERFACE_UP name={name:?} medium=backbone bind={bind}");
                }
                Err(error) => {
                    eprintln!(
                        "RNSD_INTERFACE_FAILED name={name:?} medium=backbone bind={bind} error={error}"
                    );
                }
            }
        }
        PlannedMedium::BackboneClient { host, port } => {
            // Wire-identical to a TCP client, under the Backbone kind. The connector's default pipe
            // claim is the reference's 100 Mbps `BackboneClientInterface.BITRATE_GUESS`.
            let target = format!("{host}:{port}");
            let bitrate = interface
                .bitrate_bps
                .unwrap_or(backbone_core::BACKBONE_CLIENT_BITRATE_GUESS_BPS);
            handle.add_interface(BackboneClientInterface::new(
                target.clone(),
                bitrate,
                TCP_RECONNECT,
            ));
            println!("RNSD_INTERFACE_UP name={name:?} medium=backbone-client target={target}");
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
                            async move { crate::pipe::spawn(&argv).await }
                        },
                        respawn,
                        command.as_bytes(),
                    );
                    handle.add_interface(pipe);
                    println!("RNSD_INTERFACE_UP name={name:?} medium=pipe command={command:?}");
                }
                _ => {
                    eprintln!(
                        "RNSD_INTERFACE_FAILED name={name:?} medium=pipe command={command:?} error=could not parse command into arguments"
                    );
                }
            }
        }
    }
}

fn bitrate(interface: &PlannedInterface) -> u32 {
    interface
        .bitrate_bps
        .unwrap_or(tcp_core::TCP_BITRATE_GUESS_BPS)
}

fn report_unapplied(interface: &PlannedInterface) {
    for setting in &interface.unapplied {
        let detail = match setting {
            UnappliedSetting::Mode(mode) => format!("mode={mode:?}"),
            UnappliedSetting::AnnounceBandwidthCap => String::from("announce_cap"),
            UnappliedSetting::AnnounceRateLimit => String::from("announce_rate_limit"),
            UnappliedSetting::IfacAuthentication => String::from("ifac"),
            UnappliedSetting::MediumOption(key) => format!("option={key}"),
        };
        println!("RNSD_SETTING_UNAPPLIED name={:?} {detail}", interface.name);
    }
}

fn report_deferred(plan: &DaemonPlan) {
    for deferred in &plan.deferred {
        let reason = match deferred.why {
            DeferReason::Disabled => String::from("disabled"),
            DeferReason::UnsupportedKind => String::from("unsupported-kind"),
            DeferReason::MissingRequiredField { key } => format!("missing-field:{key}"),
        };
        println!(
            "RNSD_INTERFACE_DEFERRED name={:?} type={:?} reason={reason}",
            deferred.name, deferred.type_name
        );
    }
}
