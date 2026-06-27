#![no_main]

use libfuzzer_sys::fuzz_target;
use personal_rns::interfaces::rns_parity::local::impls::rpc_value::Value;

fuzz_target!(|data: &[u8]| {
    let mut at = 0;
    let value = build_value(data, &mut at, 3);
    let encoded = value.to_msgpack();

    assert!(!encoded.is_empty());
    assert_eq!(encoded, value.to_msgpack());
});

fn build_value(data: &[u8], at: &mut usize, depth: u8) -> Value {
    let tag = next(data, at) % if depth == 0 { 5 } else { 7 };
    match tag {
        0 => Value::Nil,
        1 => Value::Bool(next(data, at) & 1 == 1),
        2 => Value::Int(read_i64(data, at)),
        3 => Value::Str(String::from_utf8_lossy(&take_vec(data, at, 32)).into_owned()),
        4 => Value::Bytes(take_vec(data, at, 48)),
        5 => {
            let len = (next(data, at) % 4) as usize;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(build_value(data, at, depth - 1));
            }
            Value::Array(items)
        }
        _ => {
            let len = (next(data, at) % 4) as usize;
            let mut pairs = Vec::with_capacity(len);
            for _ in 0..len {
                let key = String::from_utf8_lossy(&take_vec(data, at, 24)).into_owned();
                pairs.push((key, build_value(data, at, depth - 1)));
            }
            Value::Map(pairs)
        }
    }
}

fn read_i64(data: &[u8], at: &mut usize) -> i64 {
    let mut bytes = [0u8; 8];
    for byte in &mut bytes {
        *byte = next(data, at);
    }
    i64::from_be_bytes(bytes)
}

fn take_vec(data: &[u8], at: &mut usize, max_len: usize) -> Vec<u8> {
    let len = (next(data, at) as usize) % (max_len + 1);
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(next(data, at));
    }
    out
}

fn next(data: &[u8], at: &mut usize) -> u8 {
    if data.is_empty() {
        return 0;
    }
    let byte = data[*at % data.len()];
    *at = (*at).wrapping_add(1);
    byte
}
