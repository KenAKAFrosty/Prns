//! Bringing up the host's auto-discovering interfaces — WiFi/LAN and USB — so a
//! peer on the same network or USB-tethered is found with no address configured at
//! all. This is the part that makes `clone` name a destination and nothing else.
//!
//! It's the same wiring `personal-hopspot`'s desktop app uses; an engine consumer
//! supplies the platform bits the auto-interfaces can't own (how to enumerate and
//! open USB-serial ports, and — on Linux — the hot-plug signal).

use std::io;
use std::sync::Arc;

use tokio::sync::Notify;
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use personal_rns::interfaces::rns_parity::wifi_auto::AutoWifi;
use personal_rns::interfaces::usb_auto::impls::tokio::UsbAutoHost;
use personal_rns::interfaces::InterfaceId;
use personal_rns::runtime::PrnsHandle;

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 8]);
const USB_BAUD: u32 = 115_200;

/// Attach both auto-interfaces to a running node: the USB-auto host (which discovers
/// and multiplexes CDC ports behind one interface) and the WiFi/LAN `AutoInterface`
/// (multicast peer discovery on the routed link-local NIC). Neither takes an address;
/// both just find peers. The returned handles keep the interfaces attached — drop
/// them and the interfaces keep running for the node's lifetime.
pub fn bring_up_auto_interfaces(handle: &PrnsHandle) {
    let rescan = Arc::new(Notify::new());
    handle.add_interface(UsbAutoHost::new(
        USB_INTERFACE_ID,
        scan_cdc_ports,
        open_cdc_port,
        Arc::clone(&rescan),
    ));
    #[cfg(target_os = "linux")]
    spawn_hotplug_watcher(rescan);
    #[cfg(not(target_os = "linux"))]
    let _ = rescan;

    handle.supervise(AutoWifi::new());
}

/// Enumerate the CDC (USB-serial) ports currently present — the names the host probes
/// and multiplexes. The auto-host re-runs this on its own cadence to pick up
/// hot-plugged boards.
fn scan_cdc_ports() -> Vec<String> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|info| matches!(info.port_type, serialport::SerialPortType::UsbPort(_)))
        .map(|info| info.port_name)
        .collect()
}

/// Open one CDC port into an async stream, settling the modem lines so an ESP32's
/// native USB-serial-JTAG (which maps RTS→EN, DTR→GPIO0) is never knocked into reset.
async fn open_cdc_port(name: String) -> io::Result<SerialStream> {
    use serialport::SerialPort;
    let mut port = tokio_serial::new(&name, USB_BAUD)
        .open_native_async()
        .map_err(io::Error::other)?;
    let _ = port.write_request_to_send(false);
    let _ = port.write_data_terminal_ready(false);
    Ok(port)
}

/// Watch udev for serial-device hot-plug and poke the rescan signal on each event, so
/// a board appears the instant it's plugged in rather than on the fallback scan. The
/// monitor holds non-`Send` handles, so it rides its own thread with a current-thread
/// runtime while the cross-thread `Notify` pokes the host.
#[cfg(target_os = "linux")]
fn spawn_hotplug_watcher(rescan: Arc<Notify>) {
    let _ = std::thread::Builder::new()
        .name("git-over-rns-udev".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(watch_hotplug(rescan));
        });
}

#[cfg(target_os = "linux")]
async fn watch_hotplug(rescan: Arc<Notify>) {
    use tokio_stream::StreamExt;

    let listener = tokio_udev::MonitorBuilder::new()
        .and_then(|builder| builder.match_subsystem("tty"))
        .and_then(|builder| builder.listen());
    let Ok(listener) = listener else {
        return;
    };
    let Ok(mut events) = tokio_udev::AsyncMonitorSocket::new(listener) else {
        return;
    };
    while let Some(event) = events.next().await {
        if event.is_ok() {
            rescan.notify_one();
        }
    }
}
