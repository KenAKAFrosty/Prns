//! The recipe intent for "every zero-config interface this build carries": each enabled
//! auto family attaches with its defaults, so a fresh node hears its surroundings without
//! naming a single wire. `Auto` only exists when at least one auto family is enabled.

#[cfg(any(feature = "wifi", feature = "usb-host", feature = "ble-host"))]
use prns_runtime::runtime::{AttachIntent, PrnsNodeHandle};

#[cfg(any(feature = "wifi", feature = "usb-host", feature = "ble-host"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Auto;

#[cfg(any(feature = "wifi", feature = "usb-host", feature = "ble-host"))]
impl AttachIntent for Auto {
    fn attach(self, handle: &PrnsNodeHandle) {
        #[cfg(feature = "wifi")]
        handle.attach(crate::wifi::AutoWifi::default());
        #[cfg(feature = "usb-host")]
        handle.attach(crate::usb_host::AutoUsb::default());
        #[cfg(feature = "ble-host")]
        handle.attach(crate::ble_host::AutoBle);
    }
}
