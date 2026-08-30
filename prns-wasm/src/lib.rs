#![forbid(unsafe_code)]

mod bluetooth_auto;
mod command_settlement;
mod event_projection;
mod input;
mod js_translation;
mod outbound_batch;
mod packed_snapshot;
mod parameters;
mod runtime;
mod usb_auto;
mod websocket;

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
pub use websocket::WebSocketFramingCodec;

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

#[wasm_bindgen(js_name = profileTokenSeal)]
pub fn profile_token_seal(bytes: u32, iterations: u32) -> Result<u32, JsValue> {
    if bytes == 0 || iterations == 0 {
        return Err(JsValue::from_str("bytes and iterations must be positive"));
    }
    let bytes = bytes as usize;
    let key_bytes = [0x5a; 64];
    let key = personal_rns::crypto::TokenKey::from_aes256(&key_bytes);
    let plaintext = vec![0xa5; bytes];
    let mut sealed = vec![0; personal_rns::crypto::sealed_len(bytes)];
    let mut checksum = 0u32;
    for iteration in 0..iterations {
        let iv = [iteration as u8; 16];
        let sealed_len = personal_rns::crypto::token_seal(
            &key,
            &iv,
            &plaintext,
            &mut sealed,
        )
        .map_err(|_| JsValue::from_str("token seal failed"))?;
        checksum = checksum.wrapping_add(u32::from(sealed[sealed_len - 1]));
    }
    Ok(checksum)
}

#[wasm_bindgen(js_name = profileTokenVector)]
pub fn profile_token_vector(bytes: u32) -> Result<Vec<u8>, JsValue> {
    if bytes == 0 {
        return Err(JsValue::from_str("bytes must be positive"));
    }
    let bytes = bytes as usize;
    let key_bytes = [0x5a; 64];
    let key = personal_rns::crypto::TokenKey::from_aes256(&key_bytes);
    let plaintext = vec![0xa5; bytes];
    let mut sealed = vec![0; personal_rns::crypto::sealed_len(bytes)];
    let sealed_len = personal_rns::crypto::token_seal(
        &key,
        &[0x3c; 16],
        &plaintext,
        &mut sealed,
    )
    .map_err(|_| JsValue::from_str("token vector seal failed"))?;
    sealed.truncate(sealed_len);
    Ok(sealed)
}

#[wasm_bindgen(js_name = profileTokenOpen)]
pub fn profile_token_open(bytes: u32, iterations: u32) -> Result<u32, JsValue> {
    if bytes == 0 || iterations == 0 {
        return Err(JsValue::from_str("bytes and iterations must be positive"));
    }
    let bytes = bytes as usize;
    let key_bytes = [0x5a; 64];
    let key = personal_rns::crypto::TokenKey::from_aes256(&key_bytes);
    let plaintext = vec![0xa5; bytes];
    let mut sealed = vec![0; personal_rns::crypto::sealed_len(bytes)];
    let sealed_len = personal_rns::crypto::token_seal(
        &key,
        &[0x3c; 16],
        &plaintext,
        &mut sealed,
    )
    .map_err(|_| JsValue::from_str("token preparation failed"))?;
    let mut opened = vec![0; sealed_len];
    let mut checksum = 0u32;
    for _ in 0..iterations {
        let opened_len = personal_rns::crypto::token_open(
            &key,
            &sealed[..sealed_len],
            &mut opened,
        )
        .map_err(|_| JsValue::from_str("token open failed"))?;
        checksum = checksum.wrapping_add(u32::from(opened[opened_len - 1]));
    }
    Ok(checksum)
}

#[wasm_bindgen(js_name = profileSha256)]
pub fn profile_sha256(bytes: u32, iterations: u32) -> Result<u32, JsValue> {
    if bytes == 0 || iterations == 0 {
        return Err(JsValue::from_str("bytes and iterations must be positive"));
    }
    let payload = vec![0xa5; bytes as usize];
    let mut checksum = 0u32;
    for _ in 0..iterations {
        checksum = checksum.wrapping_add(u32::from(personal_rns::crypto::sha256(&payload)[0]));
    }
    Ok(checksum)
}
