use std::collections::BTreeMap;

use crate::reference::ReferenceValue;

pub(super) fn global_bool(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
    default: bool,
) -> bool {
    match globals.get(key).and_then(ReferenceValue::as_scalar) {
        Some(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => true,
            "false" | "no" | "off" | "0" => false,
            _ => default,
        },
        None => default,
    }
}

pub(super) fn global_u16(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<u16> {
    global_number(globals, key)
}

pub(super) fn global_u64(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<u64> {
    global_number(globals, key)
}

pub(super) fn global_string(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Option<String> {
    globals
        .get(key)
        .and_then(ReferenceValue::as_scalar)
        .map(str::to_string)
}

pub(super) fn decode_hex(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let text = core::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}

pub(super) fn global_i64(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<i64> {
    global_number(globals, key)
}

pub(super) fn global_f64(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<f64> {
    global_number(globals, key)
}

fn global_number<T>(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<T>
where
    T: core::str::FromStr,
{
    globals
        .get(key)
        .and_then(ReferenceValue::as_scalar)
        .and_then(|text| crate::reference::cleaned_number(text.trim()))
        .and_then(|text| text.parse().ok())
}
