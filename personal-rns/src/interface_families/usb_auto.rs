#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::usb_auto::{
    AutoUsb, UsbAutoHost, DEFAULT_USB_AUTO_ID, DEFAULT_USB_BAUD,
};

#[cfg(feature = "embassy-host")]
pub use prns_interfaces_embassy::usb_auto::{
    UsbAutoDevice, UsbAutoDeviceInput, WebUsbAutoClass, WebUsbAutoError, WebUsbAutoRx,
    WebUsbAutoState, WebUsbAutoTx, WEBUSB_AUTO_CONTROL_BUFFER_BYTES,
    WEBUSB_AUTO_MSOS_DESCRIPTOR_BYTES, WEBUSB_AUTO_PACKET_SIZE,
};
