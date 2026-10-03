#![forbid(unsafe_code)]

mod bluetooth_auto;
mod browser_work;
mod command_settlement;
mod event_projection;
mod inline_work;
mod input;
mod js_translation;
mod outbound_batch;
mod packed_snapshot;
mod parameters;
mod portable_crypto;
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
pub use portable_crypto::{portable_ed25519_verify, portable_link_proof_verify};
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
        let sealed_len = personal_rns::crypto::token_seal(&key, &iv, &plaintext, &mut sealed)
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
    let sealed_len = personal_rns::crypto::token_seal(&key, &[0x3c; 16], &plaintext, &mut sealed)
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
    let sealed_len = personal_rns::crypto::token_seal(&key, &[0x3c; 16], &plaintext, &mut sealed)
        .map_err(|_| JsValue::from_str("token preparation failed"))?;
    let mut opened = vec![0; sealed_len];
    let mut checksum = 0u32;
    for _ in 0..iterations {
        let opened_len = personal_rns::crypto::token_open(&key, &sealed[..sealed_len], &mut opened)
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

#[wasm_bindgen(js_name = profileEd25519Vector)]
pub fn profile_ed25519_vector() -> Vec<u8> {
    let secret = personal_rns::crypto::Ed25519SecretKey::new([0x11; 32]);
    personal_rns::crypto::ed25519_sign(&secret, b"sign-this")
        .0
        .to_vec()
}

#[wasm_bindgen(js_name = profileX25519Vector)]
pub fn profile_x25519_vector() -> Vec<u8> {
    let secret = personal_rns::crypto::X25519SecretKey::new([0x22; 32]);
    let peer = personal_rns::crypto::X25519PublicKey([
        0x7b, 0x0d, 0x47, 0xd9, 0x34, 0x27, 0xf8, 0x31, 0x11, 0x60, 0x78, 0x1c, 0x7c, 0x73, 0x3f,
        0xd8, 0x9f, 0x88, 0x97, 0x0a, 0xef, 0x49, 0x0d, 0x8a, 0xa0, 0xee, 0x19, 0xa4, 0xcb, 0x8a,
        0x1b, 0x14,
    ]);
    personal_rns::crypto::x25519_diffie_hellman(&secret, &peer)
        .as_bytes()
        .to_vec()
}

#[wasm_bindgen(js_name = profileHkdfSha256Vector)]
pub fn profile_hkdf_sha256_vector() -> Vec<u8> {
    personal_rns::crypto::hkdf_sha256::<64>(&[0x42; 32], &[0x01; 16], b"context").to_vec()
}

#[wasm_bindgen(js_name = profileEd25519Sign)]
pub fn profile_ed25519_sign(iterations: u32) -> Result<u32, JsValue> {
    if iterations == 0 {
        return Err(JsValue::from_str("iterations must be positive"));
    }
    let secret = personal_rns::crypto::Ed25519SecretKey::new([0x11; 32]);
    let mut checksum = 0u32;
    for _ in 0..iterations {
        let signature = personal_rns::crypto::ed25519_sign(&secret, b"sign-this");
        checksum = checksum.wrapping_add(u32::from(signature.0[0]));
    }
    Ok(checksum)
}

#[wasm_bindgen(js_name = profileEd25519Verify)]
pub fn profile_ed25519_verify(iterations: u32) -> Result<u32, JsValue> {
    if iterations == 0 {
        return Err(JsValue::from_str("iterations must be positive"));
    }
    let secret = personal_rns::crypto::Ed25519SecretKey::new([0x11; 32]);
    let public = personal_rns::crypto::ed25519_public_key(&secret);
    let signature = personal_rns::crypto::ed25519_sign(&secret, b"sign-this");
    let verifier = personal_rns::crypto::Ed25519Verifier::new(&public)
        .map_err(|_| JsValue::from_str("Ed25519 verifier setup failed"))?;
    let mut checksum = 0u32;
    for _ in 0..iterations {
        verifier
            .verify(b"sign-this", &signature)
            .map_err(|_| JsValue::from_str("Ed25519 verification failed"))?;
        checksum = checksum.wrapping_add(1);
    }
    Ok(checksum)
}

#[wasm_bindgen(js_name = profileX25519)]
pub fn profile_x25519(iterations: u32) -> Result<u32, JsValue> {
    if iterations == 0 {
        return Err(JsValue::from_str("iterations must be positive"));
    }
    let secret = personal_rns::crypto::X25519SecretKey::new([0x22; 32]);
    let peer = personal_rns::crypto::X25519PublicKey([
        0x7b, 0x0d, 0x47, 0xd9, 0x34, 0x27, 0xf8, 0x31, 0x11, 0x60, 0x78, 0x1c, 0x7c, 0x73, 0x3f,
        0xd8, 0x9f, 0x88, 0x97, 0x0a, 0xef, 0x49, 0x0d, 0x8a, 0xa0, 0xee, 0x19, 0xa4, 0xcb, 0x8a,
        0x1b, 0x14,
    ]);
    let mut checksum = 0u32;
    for _ in 0..iterations {
        let shared = personal_rns::crypto::x25519_diffie_hellman(&secret, &peer);
        checksum = checksum.wrapping_add(u32::from(shared.as_bytes()[0]));
    }
    Ok(checksum)
}

#[wasm_bindgen(js_name = profileHkdfSha256)]
pub fn profile_hkdf_sha256(iterations: u32) -> Result<u32, JsValue> {
    if iterations == 0 {
        return Err(JsValue::from_str("iterations must be positive"));
    }
    let mut checksum = 0u32;
    for _ in 0..iterations {
        let output = personal_rns::crypto::hkdf_sha256::<32>(&[0x42; 32], &[0x01; 16], b"context");
        checksum = checksum.wrapping_add(u32::from(output[0]));
    }
    Ok(checksum)
}
