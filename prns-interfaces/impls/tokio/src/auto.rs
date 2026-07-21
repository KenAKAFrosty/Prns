//! The recipe intent for "every zero-config interface this build carries": each enabled
//! auto family attaches with its defaults, so a fresh node hears its surroundings without
//! naming a single wire. `Auto` only exists when at least one auto family is enabled.

#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "ble"))]
use prns_runtime::runtime::{AttachIntent, PrnsNodeHandle};

#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "ble"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Auto;

#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "ble"))]
impl AttachIntent for Auto {
    fn attach(self, handle: &PrnsNodeHandle) {
        #[cfg(feature = "wifi-auto")]
        handle.attach(crate::wifi_auto::AutoWifi::default());
        #[cfg(feature = "usb")]
        handle.attach(crate::usb_auto::AutoUsb::default());
        #[cfg(feature = "ble")]
        handle.attach(crate::ble_host::AutoBle);
    }
}
