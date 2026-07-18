use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use super::RnsRpcReplyKind;

const NONE: &[u8] = b"N.";
const FALSE: &[u8] = b"I00\n.";
const TRUE: &[u8] = b"I01\n.";
const EMPTY_LIST: &[u8] = b"].";
const EMPTY_MAP: &[u8] = b"}.";
const EMPTY_INTERFACE_STATS: &[u8] = &[
    0x80, 0x02, 0x7d, 0x71, 0x00, 0x58, 0x0a, 0x00, 0x00, 0x00, b'i', b'n', b't', b'e', b'r', b'f',
    b'a', b'c', b'e', b's', 0x71, 0x01, 0x5d, 0x71, 0x02, 0x73, 0x2e,
];

pub(super) fn encode(reply: &RnsRpcReplyKind) -> Vec<u8> {
    match reply {
        RnsRpcReplyKind::None => NONE.to_vec(),
        RnsRpcReplyKind::Boolean(false) => FALSE.to_vec(),
        RnsRpcReplyKind::Boolean(true) => TRUE.to_vec(),
        RnsRpcReplyKind::Integer(value) => pickle_number(b'I', &value.to_string()),
        RnsRpcReplyKind::Float(value) => pickle_number(b'F', &format!("{value:?}")),
        RnsRpcReplyKind::NextHop(None) => NONE.to_vec(),
        RnsRpcReplyKind::NextHop(Some(value)) => pickle_binary(value),
        RnsRpcReplyKind::NextHopInterfaceName(value) => pickle_string(value),
        RnsRpcReplyKind::PathTable(_) | RnsRpcReplyKind::AnnounceRateTable(_) => {
            EMPTY_LIST.to_vec()
        }
        RnsRpcReplyKind::InterfaceStats(_) => EMPTY_INTERFACE_STATS.to_vec(),
        RnsRpcReplyKind::BlackholeTable(_) => EMPTY_MAP.to_vec(),
    }
}

fn pickle_number(opcode: u8, value: &str) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(value.len() + 3);
    encoded.push(opcode);
    encoded.extend_from_slice(value.as_bytes());
    encoded.push(b'\n');
    encoded.push(b'.');
    encoded
}

fn pickle_binary(value: &[u8; 16]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(21);
    encoded.extend_from_slice(&[0x80, 0x03, b'C', 16]);
    encoded.extend_from_slice(value);
    encoded.push(b'.');
    encoded
}

fn pickle_string(value: &str) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(value.len() + 3);
    encoded.push(b'V');
    encoded.extend_from_slice(value.as_bytes());
    encoded.push(b'\n');
    encoded.push(b'.');
    encoded
}
