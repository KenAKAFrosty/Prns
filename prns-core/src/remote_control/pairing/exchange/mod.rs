use crate::crypto::{Ed25519Signature, SHA256_OUTPUT_LEN};
use crate::identity::IDENTITY_PUBLIC_KEY_LEN;
use crate::remote_control::RemoteControlRequestKind;
use crate::units::DurationMillis;

use super::MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER;

mod invitation;
mod transcript;
mod wire;

pub use invitation::*;
pub use transcript::*;
pub use wire::*;

pub const REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID: &str = "/pair";
pub const MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT: DurationMillis =
    MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER;

const PAIRING_MESSAGE_HEADER_ENCODED_LEN: usize = 2;
const PAIRING_IDENTITY_ENCODED_LEN: usize = IDENTITY_PUBLIC_KEY_LEN;
const PAIRING_REQUEST_SET_COUNT_ENCODED_LEN: usize = 1;
const PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN: usize = 4;
const PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN: usize = SHA256_OUTPUT_LEN;
const PAIRING_SIGNATURE_ENCODED_LEN: usize = Ed25519Signature::LEN;
const PAIRING_BEGIN_ENCODED_LEN: usize = PAIRING_MESSAGE_HEADER_ENCODED_LEN
    .saturating_add(PAIRING_IDENTITY_ENCODED_LEN)
    .saturating_add(PAIRING_INVITATION_PROOF_ENCODED_LEN);
const PAIRING_COMMIT_ENCODED_LEN: usize =
    PAIRING_MESSAGE_HEADER_ENCODED_LEN.saturating_add(PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN);
const PAIRING_OFFER_MAX_ENCODED_LEN: usize = PAIRING_MESSAGE_HEADER_ENCODED_LEN
    .saturating_add(PAIRING_IDENTITY_ENCODED_LEN)
    .saturating_add(PAIRING_REQUEST_SET_COUNT_ENCODED_LEN)
    .saturating_add(RemoteControlRequestKind::ALL.len())
    .saturating_add(PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN)
    .saturating_add(PAIRING_SIGNATURE_ENCODED_LEN);
const PAIRING_COMPLETED_ENCODED_LEN: usize = PAIRING_MESSAGE_HEADER_ENCODED_LEN
    .saturating_add(PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN)
    .saturating_add(PAIRING_SIGNATURE_ENCODED_LEN);
const PAIRING_TRANSCRIPT_PERMISSION_BYTES_LEN: usize =
    PAIRING_REQUEST_SET_COUNT_ENCODED_LEN.saturating_add(RemoteControlRequestKind::ALL.len());
const PAIRING_CONFIRMATION_CODE_MODULUS: u32 = 1_000_000;
const PAIRING_TRANSCRIPT_DOMAIN: &[u8] = b"reticulum.remote.control.pairing.transcript.v1";
const PAIRING_COMPLETION_DOMAIN: &[u8] = b"reticulum.remote.control.pairing.completed.v1";

const _: () = assert!(RemoteControlRequestKind::ALL.len() <= u8::MAX as usize);
const _: () = assert!(MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT.0 <= u32::MAX as u64);

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlPairingProtocolVersion {
        V2 = 2,
    }
}

impl RemoteControlPairingProtocolVersion {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    const fn from_wire(value: u8) -> Option<Self> {
        match value {
            2 => Some(Self::V2),
            _ => None,
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlPairingMessageKind {
        Begin = 1,
        Offer = 2,
        Commit = 3,
        Completed = 4,
    }
}

impl RemoteControlPairingMessageKind {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    const fn from_wire(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Begin),
            2 => Some(Self::Offer),
            3 => Some(Self::Commit),
            4 => Some(Self::Completed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingMessageDirection {
    Request,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingIdentityRole {
    Controller,
    Target,
}

#[cfg(test)]
mod tests;
