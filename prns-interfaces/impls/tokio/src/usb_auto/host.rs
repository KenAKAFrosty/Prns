use std::sync::Arc;

use prns_runtime::interfaces::IfacContext;
use prns_runtime::interfaces::{ConfiguredInterfacePolicy, EffectiveInterfacePolicy, InterfaceId};
use prns_runtime::runtime::{Attachable, AttachedInterface, PrnsNodeHandle};
use tokio::sync::Notify;

use super::{UsbAutoCandidate, UsbAutoHost, UsbAutoIncarnation};
use crate::serial::{open_host_serial, scan_usb_serial_ports};

pub const DEFAULT_USB_AUTO_ID: InterfaceId = InterfaceId::new([0xD0; 8]);
pub const DEFAULT_USB_BAUD: u32 = 115_200;

pub struct AutoUsb {
    baud: u32,
    policy: EffectiveInterfacePolicy,
    rescan: Arc<Notify>,
}

impl Default for AutoUsb {
    fn default() -> Self {
        Self {
            baud: DEFAULT_USB_BAUD,
            policy: prns_runtime::interfaces::usb_auto::HOST_DEFAULTS
                .configured(ConfiguredInterfacePolicy::default()),
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

    #[must_use]
    pub fn with_policy(mut self, policy: EffectiveInterfacePolicy) -> Self {
        self.policy = policy;
        self
    }
}

fn scan_cdc_targets() -> Vec<UsbAutoCandidate> {
    scan_usb_serial_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|port| {
            UsbAutoCandidate::unclassified_attachment(
                port.path(),
                UsbAutoIncarnation::new(port.incarnation()),
            )
        })
        .collect()
}

impl Attachable for AutoUsb {
    type Attached = AttachedInterface;
    fn attach_to(self, handle: &PrnsNodeHandle) -> AttachedInterface {
        let baud = self.baud;
        handle.add_interface(UsbAutoHost::with_policy(
            DEFAULT_USB_AUTO_ID,
            scan_cdc_targets,
            move |candidate: UsbAutoCandidate| async move {
                open_host_serial(candidate.locator(), baud)
            },
            self.rescan,
            self.policy,
        ))
    }

    fn attach_to_with_ifac(
        self,
        handle: &PrnsNodeHandle,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedInterface {
        let baud = self.baud;
        handle.add_interface_with_ifac_name(
            UsbAutoHost::with_policy(
                DEFAULT_USB_AUTO_ID,
                scan_cdc_targets,
                move |candidate: UsbAutoCandidate| async move {
                    open_host_serial(candidate.locator(), baud)
                },
                self.rescan,
                self.policy,
            ),
            ifac,
            network_name,
        )
    }
}
