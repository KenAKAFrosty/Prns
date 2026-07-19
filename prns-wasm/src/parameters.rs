use core::convert::TryFrom;

use personal_rns::identity::IDENTITY_SECRET_KEY_LEN;
use personal_rns::interfaces::websocket::core as websocket_core;
use personal_rns::interfaces::{BitrateBps, INTERFACE_ID_LEN};
use personal_rns::wire::TRUNCATED_HASH_BYTE_LEN;
use wasm_bindgen::prelude::*;

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
    bitrate_bps_u32(websocket_core::WEBSOCKET_BITRATE_ESTIMATE)
}

#[wasm_bindgen(js_name = websocketHardwareMtu)]
pub fn websocket_hardware_mtu() -> usize {
    websocket_core::WEBSOCKET_HW_MTU_CAP
}

pub(crate) fn bitrate_bps_u32(bitrate: BitrateBps) -> u32 {
    u32::try_from(bitrate.get()).unwrap_or(u32::MAX)
}
