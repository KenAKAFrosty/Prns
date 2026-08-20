#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto"))]
use prns_runtime::runtime::{AttachIntent, PrnsNodeHandle};

#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "bluetooth-auto"), derive(Default))]
pub struct DefaultAutoInterfaces {
    #[cfg(feature = "bluetooth-auto")]
    ble_identity: prns_core::interfaces::bluetooth_auto::BleIdentity,
}

#[cfg(feature = "bluetooth-auto")]
impl DefaultAutoInterfaces {
    pub const fn new(ble_identity: prns_core::interfaces::bluetooth_auto::BleIdentity) -> Self {
        Self { ble_identity }
    }
}

#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto"))]
impl AttachIntent for DefaultAutoInterfaces {
    fn attach(self, handle: &PrnsNodeHandle) {
        #[cfg(feature = "wifi-auto")]
        handle.attach(crate::wifi_auto::AutoWifi::default());
        #[cfg(all(
            feature = "usb",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        handle.attach(crate::usb_auto::AutoUsb::default());
        #[cfg(feature = "bluetooth-auto")]
        handle.attach(crate::bluetooth_auto::AutoBle::new(self.ble_identity));
    }
}
