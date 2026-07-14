//! Zero-config USB Auto for a std host: the library scan (USB CDC serial ports) and open
//! (the platform serial opener) behind one shopping-list type.

use std::sync::Arc;

use prns_runtime::interfaces::ifac::IfacContext;
use prns_runtime::interfaces::InterfaceId;
use prns_runtime::runtime::{Attachable, AttachedInterface, TokioPrnsHandle};
use tokio::sync::Notify;

use crate::serial_host::open_host_serial;
use crate::usb::UsbAutoHost;

pub const DEFAULT_USB_AUTO_ID: InterfaceId = InterfaceId::new([0xD0; 8]);
pub const DEFAULT_USB_BAUD: u32 = 115_200;

/// USB Auto with everything defaulted: discover USB CDC serial ports and speak the
/// usb-auto handshake to whichever carry a Prns node. Discovery rides the host's fallback
/// scan timer; a platform hot-plug watcher can poke [`rescan_signal`](Self::rescan_signal)
/// to make plug-in instant.
pub struct AutoUsb {
    baud: u32,
    rescan: Arc<Notify>,
}

impl Default for AutoUsb {
    fn default() -> Self {
        Self {
            baud: DEFAULT_USB_BAUD,
            rescan: Arc::new(Notify::new()),
        }
    }
}

impl AutoUsb {
    #[must_use]
    pub fn rescan_signal(&self) -> Arc<Notify> {
        self.rescan.clone()
    }

    #[must_use]
    pub fn with_baud(mut self, baud: u32) -> Self {
        self.baud = baud;
        self
    }
}

fn scan_cdc_targets() -> Vec<String> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|info| matches!(info.port_type, serialport::SerialPortType::UsbPort(_)))
        .map(|info| info.port_name)
        .collect()
}

impl Attachable for AutoUsb {
    type Attached = AttachedInterface;
    fn attach_to(self, handle: &TokioPrnsHandle) -> AttachedInterface {
        let baud = self.baud;
        handle.add_interface(UsbAutoHost::new(
            DEFAULT_USB_AUTO_ID,
            scan_cdc_targets,
            move |name: String| async move { open_host_serial(&name, baud) },
            self.rescan,
        ))
    }

    fn attach_to_with_ifac(
        self,
        handle: &TokioPrnsHandle,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedInterface {
        let baud = self.baud;
        handle.add_interface_with_ifac_name(
            UsbAutoHost::new(
                DEFAULT_USB_AUTO_ID,
                scan_cdc_targets,
                move |name: String| async move { open_host_serial(&name, baud) },
                self.rescan,
            ),
            ifac,
            network_name,
        )
    }
}
