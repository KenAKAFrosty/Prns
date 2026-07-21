#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto"))]
use prns_runtime::runtime::{AttachIntent, PrnsNodeHandle};

#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DefaultAutoInterfaces;

#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto"))]
impl AttachIntent for DefaultAutoInterfaces {
    fn attach(self, handle: &PrnsNodeHandle) {
        #[cfg(feature = "wifi-auto")]
        handle.attach(crate::wifi_auto::AutoWifi::default());
        #[cfg(feature = "usb")]
        handle.attach(crate::usb_auto::AutoUsb::default());
        #[cfg(feature = "bluetooth-auto")]
        handle.attach(crate::bluetooth_auto::AutoBle);
    }
}
