use std::collections::BTreeMap;

use crate::reference::{parse_bool, ReferenceValue};

pub(super) fn global_bool(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
    default: bool,
) -> bool {
    globals
        .get(key)
        .and_then(ReferenceValue::as_scalar)
        .and_then(parse_bool)
        .unwrap_or(default)
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
