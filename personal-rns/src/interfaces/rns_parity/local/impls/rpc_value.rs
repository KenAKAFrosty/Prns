//! A tiny typed msgpack encoder for the shared-instance control-RPC replies. RNS 1.3.5 frames the
//! RPC in msgpack (`mp.unpackb`), so a real reply is a structured value — a map of interface stats, a
//! list of path-table rows. Rather than hand-roll the tag-and-length bytes per reply (where the bugs
//! hide) or pull a serde stack down into the no_std engine, each reply is built as a typed [`Value`]
//! tree and encoded in one place that owns every fixmap/fixstr/array tag. It covers exactly the
//! shapes RNS RPC replies take: maps and arrays of integers, strings, bytes, bools and nil. Host-side
//! only (the `local` feature); the engine returns plain structs and the shim maps them to a `Value`.

use std::string::String;
use std::vec::Vec;

/// A msgpack value in the small subset the control RPC speaks.
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Str(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Map(Vec<(String, Value)>),
}

impl Value {
    /// Encode to msgpack bytes a stock RNS client reads back with `mp.unpackb`.
    #[must_use]
    pub fn to_msgpack(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }

    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Value::Nil => out.push(0xc0),
            Value::Bool(false) => out.push(0xc2),
            Value::Bool(true) => out.push(0xc3),
            Value::Int(n) => encode_int(*n, out),
            Value::Str(s) => encode_str(s.as_bytes(), out),
            Value::Bytes(b) => encode_bin(b, out),
            Value::Array(items) => {
                encode_seq_len(items.len(), 0x90, 0xdc, 0xdd, out);
                for item in items {
                    item.encode(out);
                }
            }
            Value::Map(pairs) => {
                encode_seq_len(pairs.len(), 0x80, 0xde, 0xdf, out);
                for (key, value) in pairs {
                    encode_str(key.as_bytes(), out);
                    value.encode(out);
                }
            }
        }
    }
}

/// msgpack integer: positive/negative fixint where it fits, else the smallest uint/int width.
fn encode_int(n: i64, out: &mut Vec<u8>) {
    if (0..=0x7f).contains(&n) {
        out.push(n as u8);
    } else if (-32..0).contains(&n) {
        out.push(n as i8 as u8);
    } else if n >= 0 {
        if n <= 0xff {
            out.extend_from_slice(&[0xcc, n as u8]);
        } else if n <= 0xffff {
            out.push(0xcd);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        } else if n <= 0xffff_ffff {
            out.push(0xce);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        } else {
            out.push(0xcf);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    } else if n >= i64::from(i8::MIN) {
        out.extend_from_slice(&[0xd0, n as i8 as u8]);
    } else if n >= i64::from(i16::MIN) {
        out.push(0xd1);
        out.extend_from_slice(&(n as i16).to_be_bytes());
    } else if n >= i64::from(i32::MIN) {
        out.push(0xd2);
        out.extend_from_slice(&(n as i32).to_be_bytes());
    } else {
        out.push(0xd3);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

/// msgpack string (UTF-8): fixstr / str8 / str16 / str32 by length.
fn encode_str(bytes: &[u8], out: &mut Vec<u8>) {
    let len = bytes.len();
    if len <= 31 {
        out.push(0xa0 | len as u8);
    } else if len <= 0xff {
        out.extend_from_slice(&[0xd9, len as u8]);
    } else if len <= 0xffff {
        out.push(0xda);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xdb);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(bytes);
}

/// msgpack binary: bin8 / bin16 / bin32 by length.
fn encode_bin(bytes: &[u8], out: &mut Vec<u8>) {
    let len = bytes.len();
    if len <= 0xff {
        out.extend_from_slice(&[0xc4, len as u8]);
    } else if len <= 0xffff {
        out.push(0xc5);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xc6);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(bytes);
}

/// The length header for an array or a map: the caller passes the fix-tag base (`0x90` array,
/// `0x80` map) and the 16-/32-element opcodes; the element count picks the width.
fn encode_seq_len(len: usize, fix_base: u8, op16: u8, op32: u8, out: &mut Vec<u8>) {
    if len <= 15 {
        out.push(fix_base | len as u8);
    } else if len <= 0xffff {
        out.push(op16);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(op32);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::Value;
    use std::vec;

    #[test]
    fn scalars_match_the_msgpack_spec() {
        assert_eq!(Value::Nil.to_msgpack(), [0xc0]);
        assert_eq!(Value::Bool(true).to_msgpack(), [0xc3]);
        assert_eq!(Value::Bool(false).to_msgpack(), [0xc2]);
        assert_eq!(Value::Int(0).to_msgpack(), [0x00]);
        assert_eq!(Value::Int(6).to_msgpack(), [0x06]);
        assert_eq!(Value::Int(127).to_msgpack(), [0x7f]);
        assert_eq!(Value::Int(300).to_msgpack(), [0xcd, 0x01, 0x2c]);
        assert_eq!(
            Value::Int(65_536).to_msgpack(),
            [0xce, 0x00, 0x01, 0x00, 0x00]
        );
        assert_eq!(Value::Int(-1).to_msgpack(), [0xff]);
        assert_eq!(Value::Int(-33).to_msgpack(), [0xd0, 0xdf]);
        assert_eq!(Value::Int(-129).to_msgpack(), [0xd1, 0xff, 0x7f]);
        assert_eq!(
            Value::Int(-32_769).to_msgpack(),
            [0xd2, 0xff, 0xff, 0x7f, 0xff]
        );
        assert_eq!(Value::Str("get".into()).to_msgpack(), b"\xa3get");
    }

    #[test]
    fn string_widths_follow_the_msgpack_boundaries() {
        let str8 = Value::Str("x".repeat(32)).to_msgpack();
        assert_eq!(&str8[..2], &[0xd9, 32]);
        assert_eq!(str8.len(), 34);

        let str16 = Value::Str("x".repeat(256)).to_msgpack();
        assert_eq!(&str16[..3], &[0xda, 0x01, 0x00]);
        assert_eq!(str16.len(), 259);
    }

    #[test]
    fn binary_widths_follow_the_msgpack_boundaries() {
        let bin16 = Value::Bytes(vec![0; 256]).to_msgpack();
        assert_eq!(&bin16[..3], &[0xc5, 0x01, 0x00]);
        assert_eq!(bin16.len(), 259);
    }

    #[test]
    fn sequence_widths_follow_the_msgpack_boundaries() {
        let array = Value::Array((0..16).map(Value::Int).collect()).to_msgpack();
        assert_eq!(&array[..3], &[0xdc, 0x00, 0x10]);

        let map = Value::Map(
            (0..16)
                .map(|index| (format!("k{index}"), Value::Nil))
                .collect(),
        )
        .to_msgpack();
        assert_eq!(&map[..3], &[0xde, 0x00, 0x10]);
    }

    #[test]
    fn the_interface_stats_empty_map_matches_the_hand_rolled_bytes() {
        let value = Value::Map(vec![("interfaces".into(), Value::Array(vec![]))]);
        assert_eq!(value.to_msgpack(), b"\x81\xaainterfaces\x90");
    }

    #[test]
    fn a_nested_row_list_encodes_array_of_maps() {
        let rows = Value::Array(vec![Value::Map(vec![
            ("hops".into(), Value::Int(2)),
            ("via".into(), Value::Bytes(vec![0xab; 4])),
        ])]);
        assert_eq!(
            rows.to_msgpack(),
            b"\x91\x82\xa4hops\x02\xa3via\xc4\x04\xab\xab\xab\xab",
        );
    }
}
