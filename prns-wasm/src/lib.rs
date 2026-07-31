#![forbid(unsafe_code)]

mod bluetooth_auto;
mod input;
mod js_translation;
mod parameters;
mod runtime;
mod usb_auto;

pub use bluetooth_auto::{
    bluetooth_bitrate_bps, bluetooth_control_uuid, bluetooth_data_fragments, bluetooth_data_uuid,
    bluetooth_decode_control, bluetooth_dialer_hello, bluetooth_hardware_mtu,
    bluetooth_service_uuid, BluetoothReassembler,
};
pub use parameters::{
    browser_persistence_version, destination_hash_length, host_contract_abi, host_schema_version,
    identity_secret_key_length, interface_id_length, product_version, websocket_bitrate_bps,
    websocket_frame_cap, websocket_hardware_mtu,
};
pub use runtime::PrnsRuntime;
pub use usb_auto::{
    usb_auto_data_frame, usb_auto_host_bitrate_bps, usb_auto_host_hardware_mtu,
    usb_auto_host_hello_ack_frame, usb_auto_host_hello_frame, usb_auto_node_tag_for,
    usb_auto_web_usb_product_id, usb_auto_web_usb_vendor_id, UsbAutoDecoder,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = compressResourceCandidate)]
pub fn compress_resource_candidate(options: JsValue) -> Result<Option<Vec<u8>>, JsValue> {
    let payload = input::required_bytes(&options, "payload")?;
    let packed_metadata = input::optional_bytes(&options, "packedMetadata")?;
    Ok(
        prns_runtime::resource_compression::compress_resource_candidate(
            &payload,
            packed_metadata.as_deref(),
        ),
    )
}
