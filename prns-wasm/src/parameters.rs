use core::convert::TryFrom;

use personal_rns::identity::IDENTITY_SECRET_KEY_LEN;
use personal_rns::interfaces::websocket;
use personal_rns::interfaces::{BitrateBps, INTERFACE_ID_LEN};
use personal_rns::wire::TRUNCATED_HASH_BYTE_LEN;
use wasm_bindgen::prelude::*;

pub const BROWSER_PERSISTENCE_VERSION: u32 = 1;

#[wasm_bindgen(js_name = hostContractAbi)]
pub fn host_contract_abi() -> u32 {
    prns_host::HOST_CONTRACT_ABI
}

#[wasm_bindgen(js_name = hostSchemaVersion)]
pub fn host_schema_version() -> u32 {
    prns_host::HOST_SCHEMA_VERSION
}

#[wasm_bindgen(js_name = browserPersistenceVersion)]
pub fn browser_persistence_version() -> u32 {
    BROWSER_PERSISTENCE_VERSION
}

#[wasm_bindgen(js_name = productVersion)]
pub fn product_version() -> String {
    prns_host::HOST_CONTRACT.product_version.to_string()
}

#[wasm_bindgen(js_name = identitySecretKeyLength)]
pub fn identity_secret_key_length() -> usize {
    IDENTITY_SECRET_KEY_LEN
}

#[wasm_bindgen(js_name = interfaceIdLength)]
pub fn interface_id_length() -> usize {
    INTERFACE_ID_LEN
}

#[wasm_bindgen(js_name = destinationHashLength)]
pub fn destination_hash_length() -> usize {
    TRUNCATED_HASH_BYTE_LEN
}

#[wasm_bindgen(js_name = websocketBitrateBps)]
pub fn websocket_bitrate_bps() -> u32 {
    bitrate_bps_u32(websocket::WEBSOCKET_BITRATE_ESTIMATE)
}

#[wasm_bindgen(js_name = websocketHardwareMtu)]
pub fn websocket_hardware_mtu() -> usize {
    websocket::WEBSOCKET_HW_MTU_CAP
}

#[wasm_bindgen(js_name = websocketFrameCap)]
pub fn websocket_frame_cap() -> usize {
    websocket::FRAME_CAP
}

pub(crate) fn bitrate_bps_u32(bitrate: BitrateBps) -> u32 {
    u32::try_from(bitrate.get()).unwrap_or(u32::MAX)
}
