#![forbid(unsafe_code)]

mod bluetooth;
mod input;
mod js_translation;
mod parameters;
mod runtime;
mod usb_auto;

pub use bluetooth::{
    bluetooth_bitrate_bps, bluetooth_control_uuid, bluetooth_data_fragments, bluetooth_data_uuid,
    bluetooth_decode_control, bluetooth_dialer_hello, bluetooth_hardware_mtu,
    bluetooth_service_uuid, BluetoothReassembler,
};
pub use parameters::{
    destination_hash_length, identity_secret_key_length, interface_id_length,
    websocket_bitrate_bps, websocket_hardware_mtu,
};
pub use runtime::PrnsRuntime;
pub use usb_auto::{
    usb_auto_data_frame, usb_auto_host_bitrate_bps, usb_auto_host_hardware_mtu,
    usb_auto_host_hello_ack_frame, usb_auto_host_hello_frame, usb_auto_node_tag_for,
    usb_auto_web_usb_product_id, usb_auto_web_usb_vendor_id, UsbAutoDecoder,
};
