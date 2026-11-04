//! Allocation-backed convenience over the canonical bounded public-card codecs.
//! The wire format, validation and trust semantics remain in `public_card`.
//! These signatures are also the source for generated native foreign functions.

use super::{
    RemoteControlControllerCard, RemoteControlPublicCardError, RemoteControlTargetAccessCard,
};
use crate::identity::IdentityHash;
use crate::remote_control::{
    RemoteControlControllerIdentity, RemoteControlPairingAttemptId, RemoteControlTargetIdentity,
};
use alloc::string::{String, ToString};

/// Encode public controller identity. This grants no access.
pub fn encode_remote_control_controller_card(
    card: RemoteControlControllerCard,
) -> Result<String, RemoteControlPublicCardError> {
    let mut output = [0; RemoteControlControllerCard::MAX_TEXT_LEN];
    let length = card.encode_text(&mut output)?;
    encoded_text(&output, length)
}

/// Decode public identity only; the caller must independently trust its source.
pub fn decode_remote_control_controller_card(
    text: String,
) -> Result<RemoteControlControllerCard, RemoteControlPublicCardError> {
    RemoteControlControllerCard::decode_text(&text)
}

/// Encode recipient-bound access information, not an authorization receipt.
pub fn encode_remote_control_target_access_card(
    card: RemoteControlTargetAccessCard,
) -> Result<String, RemoteControlPublicCardError> {
    let mut output = [0; RemoteControlTargetAccessCard::MAX_TEXT_LEN];
    let length = card.encode_text(&mut output)?;
    encoded_text(&output, length)
}

/// Decode and validate the existing card format without authorizing or storing it.
pub fn decode_remote_control_target_access_card(
    text: String,
) -> Result<RemoteControlTargetAccessCard, RemoteControlPublicCardError> {
    RemoteControlTargetAccessCard::decode_text(&text)
}

/// Format the exact transcript-bound six-digit comparison code, including zeros.
/// Display equality is a user verification step, not automatic authorization.
#[must_use]
pub fn remote_control_pairing_confirmation_code(attempt: RemoteControlPairingAttemptId) -> String {
    attempt.confirmation_code().to_string()
}

/// Derive the canonical public controller fingerprint for source review.
#[must_use]
pub fn remote_control_controller_identity_hash(
    identity: RemoteControlControllerIdentity,
) -> IdentityHash {
    identity.identity_hash()
}

/// Derive the canonical target fingerprint used by saved-target lookup.
#[must_use]
pub fn remote_control_target_identity_hash(identity: RemoteControlTargetIdentity) -> IdentityHash {
    identity.identity_hash()
}

fn encoded_text(output: &[u8], length: usize) -> Result<String, RemoteControlPublicCardError> {
    let bytes = output
        .get(..length)
        .ok_or(RemoteControlPublicCardError::InvalidLength)?;
    let text = core::str::from_utf8(bytes)
        .map_err(|_| RemoteControlPublicCardError::InvalidTextEnvelope)?;
    Ok(text.to_string())
}
