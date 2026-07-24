use napi::bindgen_prelude::Buffer;
use personal_rns::identity::IdentityHash;
use personal_rns::interfaces::bluetooth_auto::BleIdentity;
use personal_rns::interfaces::InterfaceId;
use personal_rns::routing::links::request::RequestId;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::wire::{DestinationHash, TransportId};
use personal_rns::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

use crate::errors::{code_err, CodeResult, ErrorCode};

fn fixed<const N: usize>(buffer: &[u8], what: &str) -> CodeResult<[u8; N]> {
    buffer.try_into().map_err(|_| {
        code_err(
            ErrorCode::InvalidArgument,
            format!("{what} must be {N} bytes, got {}", buffer.len()),
        )
    })
}

pub fn destination_hash(buffer: &[u8]) -> CodeResult<DestinationHash> {
    Ok(DestinationHash::new(fixed(buffer, "destination hash")?))
}

pub fn interface_id(buffer: &[u8]) -> CodeResult<InterfaceId> {
    Ok(InterfaceId::new(fixed(buffer, "interface id")?))
}

pub fn identity_hash(buffer: &[u8]) -> CodeResult<IdentityHash> {
    Ok(IdentityHash::new(fixed(buffer, "identity hash")?))
}

pub fn link_id(buffer: &[u8]) -> CodeResult<LinkId> {
    Ok(LinkId::new(fixed(buffer, "link id")?))
}

pub fn request_id(buffer: &[u8]) -> CodeResult<RequestId> {
    Ok(RequestId(fixed(buffer, "request id")?))
}

pub fn request_path_hash(buffer: &[u8]) -> CodeResult<RequestPathHash> {
    Ok(RequestPathHash::new(fixed(buffer, "request path hash")?))
}

pub fn transport_id(buffer: &[u8]) -> CodeResult<TransportId> {
    Ok(TransportId::new(fixed(buffer, "transport id")?))
}

pub fn ble_identity(buffer: &[u8]) -> CodeResult<BleIdentity> {
    Ok(BleIdentity::new(fixed(buffer, "ble identity")?))
}

pub fn identity_secret(buffer: &[u8]) -> CodeResult<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>> {
    Ok(Zeroizing::new(fixed(buffer, "identity secret")?))
}

pub fn to_buffer(bytes: &[u8]) -> Buffer {
    Buffer::from(bytes.to_vec())
}

pub fn unwrap_packed_binary(packed: &[u8]) -> Option<&[u8]> {
    match packed {
        [0xc4, len, rest @ ..] if rest.len() == *len as usize => Some(rest),
        [0xc5, a, b, rest @ ..] if rest.len() == u16::from_be_bytes([*a, *b]) as usize => {
            Some(rest)
        }
        [0xc6, a, b, c, d, rest @ ..]
            if u32::try_from(rest.len()) == Ok(u32::from_be_bytes([*a, *b, *c, *d])) =>
        {
            Some(rest)
        }
        _ => None,
    }
}
