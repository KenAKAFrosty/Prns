//! Stand up a [`DaemonPlan`]'s interfaces on a running node and report what was not applied.
//!
//! Each [`PlannedMedium`] maps to its host constructor: a supervisor (`AutoWifi`) goes through
//! [`TokioPrnsHandle::supervise`], a one-to-one wire (TCP, UDP, serial) through
//! [`TokioPrnsHandle::add_interface`]. The `.status()` handle of every interface is collected into the
//! [`InterfaceViews`] the control-RPC shim reads, so a stock client's `rnstatus` sees the live
//! fleet. Settings the plan parsed but a host constructor cannot yet honor, and interfaces the plan
//! could not stand up, are logged rather than dropped silently.

use core::time::Duration;

use personal_rns::interfaces::rns_parity::serial::impls::tokio::SerialInterface;
use personal_rns::interfaces::rns_parity::tcp::core as tcp_core;
use personal_rns::interfaces::rns_parity::tcp::impls::tokio::{
    TcpClientInterface, TcpServerInterface,
};
use personal_rns::interfaces::rns_parity::udp::impls::tokio::UdpInterface;
use personal_rns::interfaces::rns_parity::wifi_auto::{AutoWifi, AutoWifiStatus};
use personal_rns::interfaces::InterfaceSnapshot;
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::runtime::TokioPrnsHandle;
use personal_rns_config::{
    DaemonPlan, DeferReason, PlannedInterface, PlannedMedium, UnappliedSetting,
};
use tokio_serial::SerialPortBuilderExt;

const TCP_RECONNECT: Duration = Duration::from_secs(5);
const SERIAL_RECONNECT: Duration = Duration::from_millis(500);

/// The live status handles of every interface the daemon stood up — the view the control-RPC shim
/// renders for a stock client. Cloneable (each handle is `Arc`-backed) so it can move into the
/// shim's snapshot closure.
#[derive(Clone, Default)]
pub struct InterfaceViews {
    plain: Vec<TokioInterfaceStatus>,
    wifi: Vec<AutoWifiStatus>,
}

impl InterfaceViews {
    /// One snapshot per interface the engine sees: each one-to-one wire, then each WiFi supervisor
    /// followed by a snapshot per live peer it stands up.
    #[must_use]
    pub fn snapshots(&self) -> Vec<InterfaceSnapshot> {
        let mut snapshots = Vec::new();
        for status in &self.plain {
            snapshots.push(InterfaceSnapshot::of(status));
        }
        for wifi in &self.wifi {
            snapshots.push(InterfaceSnapshot::of(wifi));
            for member in wifi.members() {
                snapshots.push(InterfaceSnapshot::of(&member));
            }
        }
        snapshots
    }
}

/// Stand up every planned interface on `handle`, returning their status handles. Deferred
/// interfaces and unapplied settings are logged as they are encountered.
pub async fn construct_interfaces(handle: &TokioPrnsHandle, plan: &DaemonPlan) -> InterfaceViews {
    let mut views = InterfaceViews::default();
    for interface in &plan.interfaces {
        stand_up(handle, interface, &mut views).await;
        report_unapplied(interface);
    }
    report_deferred(plan);
    views
}

async fn stand_up(handle: &TokioPrnsHandle, interface: &PlannedInterface, views: &mut InterfaceViews) {
    let name = &interface.name;
    match &interface.medium {
        PlannedMedium::AutoWifi { .. } => {
            let wifi = match interface.bitrate_bps {
                Some(bitrate) => AutoWifi::with_bitrate(bitrate),
                None => AutoWifi::new(),
            };
            views.wifi.push(wifi.status());
            handle.supervise(wifi);
            println!("RNSD_INTERFACE_UP name={name:?} medium=auto-wifi");
        }
        PlannedMedium::TcpClient { host, port } => {
            let target = format!("{host}:{port}");
            let tcp = TcpClientInterface::new(target.clone(), bitrate(interface), TCP_RECONNECT);
            views.plain.push(tcp.status());
            handle.add_interface(tcp);
            println!("RNSD_INTERFACE_UP name={name:?} medium=tcp-client target={target}");
        }
        PlannedMedium::TcpServer { bind } => {
            match TcpServerInterface::bind(bind.clone(), bitrate(interface)).await {
                Ok(server) => {
                    views.plain.push(server.status());
                    handle.add_interface(server);
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
                    views.plain.push(udp.status());
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
                    async move {
                        tokio_serial::new(&open_path, baud)
                            .open_native_async()
                            .map_err(std::io::Error::other)
                    }
                },
                SERIAL_RECONNECT,
                device.as_bytes(),
            );
            views.plain.push(serial.status());
            handle.add_interface(serial);
            println!("RNSD_INTERFACE_UP name={name:?} medium=serial device={device} baud={baud}");
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
