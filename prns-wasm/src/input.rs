use js_sys::{Array, Reflect, Uint8Array};
use personal_rns::identity::IDENTITY_SECRET_KEY_LEN;
use personal_rns::interfaces::{InterfaceId, InterfaceKind, INTERFACE_ID_LEN};
use personal_rns::routing::links::LinkId;
use personal_rns::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use zeroize::Zeroizing;

fn required_value(object: &JsValue, key: &str) -> Result<JsValue, JsValue> {
    let value = Reflect::get(object, &JsValue::from_str(key))
        .map_err(|_| JsValue::from_str(&format!("failed to read {key}")))?;
    if value.is_undefined() || value.is_null() {
        return Err(JsValue::from_str(&format!("missing required option {key}")));
    }
    Ok(value)
}

fn optional_value(object: &JsValue, key: &str) -> Result<Option<JsValue>, JsValue> {
    let value = Reflect::get(object, &JsValue::from_str(key))
        .map_err(|_| JsValue::from_str(&format!("failed to read {key}")))?;
    if value.is_undefined() || value.is_null() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

pub(crate) fn required_string(object: &JsValue, key: &str) -> Result<String, JsValue> {
    required_value(object, key)?
        .as_string()
        .ok_or_else(|| JsValue::from_str(&format!("{key} must be a string")))
}

pub(crate) fn required_array(object: &JsValue, key: &str) -> Result<Array, JsValue> {
    let value = required_value(object, key)?;
    if !Array::is_array(&value) {
        return Err(JsValue::from_str(&format!("{key} must be an array")));
    }
    Ok(Array::from(&value))
}

pub(crate) fn required_bytes(object: &JsValue, key: &str) -> Result<Vec<u8>, JsValue> {
    bytes_from_value(required_value(object, key)?, key)
}

pub(crate) fn optional_bytes(object: &JsValue, key: &str) -> Result<Option<Vec<u8>>, JsValue> {
    optional_value(object, key)?
        .map(|value| bytes_from_value(value, key))
        .transpose()
}

fn bytes_from_value(value: JsValue, key: &str) -> Result<Vec<u8>, JsValue> {
    let Some(array) = value.dyn_ref::<Uint8Array>() else {
        return Err(JsValue::from_str(&format!("{key} must be a Uint8Array")));
    };
    Ok(array.to_vec())
}

pub(crate) fn required_u64(object: &JsValue, key: &str) -> Result<u64, JsValue> {
    let value = required_value(object, key)?;
    let number = value
        .as_f64()
        .ok_or_else(|| JsValue::from_str(&format!("{key} must be a number")))?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        return Err(JsValue::from_str(&format!(
            "{key} must be a non-negative integer"
        )));
    }
    if number > u64::MAX as f64 {
        return Err(JsValue::from_str(&format!("{key} is too large")));
    }
    Ok(number as u64)
}

pub(crate) fn optional_u32(object: &JsValue, key: &str) -> Result<Option<u32>, JsValue> {
    let Some(value) = optional_value(object, key)? else {
        return Ok(None);
    };
    let number = value
        .as_f64()
        .ok_or_else(|| JsValue::from_str(&format!("{key} must be a number")))?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        return Err(JsValue::from_str(&format!(
            "{key} must be a non-negative integer"
        )));
    }
    if number > u32::MAX as f64 {
        return Err(JsValue::from_str(&format!("{key} is too large")));
    }
    Ok(Some(number as u32))
}

pub(crate) fn array_to_strings(values: &Array) -> Result<Vec<String>, JsValue> {
    let mut out = Vec::new();
    for value in values.iter() {
        let Some(value) = value.as_string() else {
            return Err(JsValue::from_str("aspects must be strings"));
        };
        out.push(value);
    }
    Ok(out)
}

pub(crate) fn secret_key_from_vec(
    bytes: Vec<u8>,
) -> Result<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>, JsValue> {
    if bytes.len() != IDENTITY_SECRET_KEY_LEN {
        return Err(JsValue::from_str("identity secret key must be 64 bytes"));
    }
    let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret.copy_from_slice(&bytes);
    Ok(secret)
}

pub(crate) fn destination_hash_from_vec(bytes: Vec<u8>) -> Result<DestinationHash, JsValue> {
    if bytes.len() != TRUNCATED_HASH_BYTE_LEN {
        return Err(JsValue::from_str("destination hash must be 16 bytes"));
    }
    let mut hash = [0u8; TRUNCATED_HASH_BYTE_LEN];
    hash.copy_from_slice(&bytes);
    Ok(DestinationHash::new(hash))
}

pub(crate) fn interface_id_from_vec(bytes: Vec<u8>) -> Result<InterfaceId, JsValue> {
    if bytes.len() != INTERFACE_ID_LEN {
        return Err(JsValue::from_str("interface id must be 8 bytes"));
    }
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(&bytes);
    Ok(InterfaceId::new(id))
}

pub(crate) fn link_id_from_vec(bytes: Vec<u8>) -> Result<LinkId, JsValue> {
    if bytes.len() != TRUNCATED_HASH_BYTE_LEN {
        return Err(JsValue::from_str("link id must be 16 bytes"));
    }
    let mut id = [0u8; TRUNCATED_HASH_BYTE_LEN];
    id.copy_from_slice(&bytes);
    Ok(LinkId::new(id))
}

pub(crate) fn parse_interface_kind(kind: &str) -> Result<InterfaceKind, JsValue> {
    match kind {
        "auto-usb-host" | "usb-auto-host" | "AutoUSB" => Ok(InterfaceKind::UsbAutoHost),
        "auto-usb-device" | "usb-auto-device" => Ok(InterfaceKind::UsbAutoDevice),
        "rnode" | "RNode" => Ok(InterfaceKind::Rnode),
        "bluetooth-auto" | "ble-auto" => Ok(InterfaceKind::BluetoothAuto),
        "bluetooth-peer" | "ble-peer" => Ok(InterfaceKind::BluetoothPeer),
        "auto-wifi" => Ok(InterfaceKind::AutoWifi),
        "websocket-client" | "websocket" => Ok(InterfaceKind::WebSocketClient),
        "websocket-server" => Ok(InterfaceKind::WebSocketServer),
        "websocket-server-peer" => Ok(InterfaceKind::WebSocketServerPeer),
        "serial" => Ok(InterfaceKind::Serial),
        "kiss" => Ok(InterfaceKind::Kiss),
        "pipe" => Ok(InterfaceKind::Pipe),
        _ => Err(JsValue::from_str("unsupported interface kind")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_wifi_crosses_the_wasm_interface_kind_boundary() {
        assert!(matches!(
            parse_interface_kind("auto-wifi"),
            Ok(InterfaceKind::AutoWifi)
        ));
    }
}
