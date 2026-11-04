//! Portable signed contact cards for a single Reticulum service.
//!
//! The binary card, canonical token and draft URI are independent of cameras,
//! application frameworks, discovery transports and group membership. Every
//! default output uses caller-owned storage; no allocator or private-key type is needed.
//! Optional `alloc` exposes owned public claims, and `uniffi-bindings` exports
//! that same verification API to foreign consumers.
//! A verified card proves its public identity signed a service/name claim. It
//! does not establish a person's name, proximity, network reachability or group
//! admission. See the crate README for the version-one wire contract.

#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(feature = "uniffi-bindings")]
extern crate std;
#[cfg(feature = "uniffi-bindings")]
uniffi::setup_scaffolding!();
#[cfg(feature = "alloc")]
pub mod api;

use core::{fmt, str};

use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

#[cfg(feature = "prns-identity")]
mod identity;
mod token;
#[cfg(feature = "prns-identity")]
pub use identity::encode_identity;

pub use token::{decode_token, decode_uri, encode_token, encode_uri, TokenError};

/// Maximum UTF-8 display-name bytes. An empty name is valid.
pub const MAX_DISPLAY_NAME_BYTES: usize = 128;
/// Reticulum public identity: X25519 public key followed by Ed25519 public key.
pub const PUBLIC_IDENTITY_BYTES: usize = 64;
/// Reticulum truncated service name hash.
pub const SERVICE_NAME_HASH_BYTES: usize = 10;
/// Reticulum truncated identity and destination hash size.
pub const HASH_BYTES: usize = 16;
/// Ed25519 signature size.
pub const SIGNATURE_BYTES: usize = 64;
/// Version-one fixed header before display-name bytes.
pub const HEADER_BYTES: usize = 84;
/// Smallest card, with an empty display name.
pub const MIN_CARD_BYTES: usize = HEADER_BYTES + SIGNATURE_BYTES;
/// Largest complete signed card.
pub const MAX_CARD_BYTES: usize = MIN_CARD_BYTES + MAX_DISPLAY_NAME_BYTES;
/// Largest unpadded canonical base64url token.
pub const MAX_TOKEN_BYTES: usize = (MAX_CARD_BYTES * 4).div_ceil(3);
/// Experimental application-level URI prefix, not a Reticulum core protocol.
pub const URI_PREFIX: &str = "rns-contact://v1/";
/// Largest complete canonical URI.
pub const MAX_URI_BYTES: usize = URI_PREFIX.len() + MAX_TOKEN_BYTES;

const MAGIC: &[u8; 8] = b"PRNSCARD";
const VERSION: u8 = 1;
const SIGNATURE_DOMAIN: &[u8] = b"prns.contact-card.v1\0";
const MAX_SIGNING_INPUT_BYTES: usize =
    SIGNATURE_DOMAIN.len() + HEADER_BYTES + MAX_DISPLAY_NAME_BYTES;

/// Public fields supplied by the embedding application.
#[derive(Clone, Copy, Debug)]
pub struct CardFields<'a> {
    /// Public identity whose signing key must authorize this card.
    pub public_identity: &'a [u8; PUBLIC_IDENTITY_BYTES],
    /// Hash of the fully qualified Reticulum service name.
    pub service_name_hash: &'a [u8; SERVICE_NAME_HASH_BYTES],
    /// Self-asserted display hint, not a verified real-world name.
    pub display_name: &'a str,
}

/// An identity owner's signing operation; private keys stay with the caller.
pub trait ContactCardSigner {
    /// Caller-defined signing failure.
    type Error;

    /// Sign these exact domain-separated bytes using ordinary Ed25519.
    fn sign(&self, message: &[u8]) -> Result<[u8; SIGNATURE_BYTES], Self::Error>;
}

/// Structural or cryptographic card rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Error))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "ContactCardError"))]
pub enum CardError {
    /// Input cannot contain a complete card.
    Truncated,
    /// Input exceeds the absolute version-one bound.
    TooLarge,
    /// The fixed contact-card magic did not match.
    InvalidMagic,
    /// This codec does not interpret the encoded version.
    UnsupportedVersion,
    /// Declared and actual length differ, including any trailing bytes.
    InvalidLength,
    /// Name exceeds its byte bound, is not UTF-8, or includes control/line/bidi markers.
    InvalidName,
    /// Ed25519 public key is invalid, identity or outside the prime-order subgroup.
    InvalidPublicIdentity,
    /// The strict Ed25519 signature did not verify against the exact card.
    InvalidSignature,
}

impl core::error::Error for CardError {}

impl fmt::Display for CardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Failure producing a signed card. Caller output is unchanged on failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError<E> {
    /// Fields or signer output are not a valid card.
    InvalidCard(CardError),
    /// Caller-owned output is too small.
    OutputTooSmall { required: usize, available: usize },
    /// Identity owner could not sign the request.
    Signer(E),
}

/// A borrowed card whose canonical fields and signature have been checked.
#[derive(Clone, Copy, Debug)]
pub struct VerifiedContactCard<'a> {
    wire: &'a [u8],
    public_identity: &'a [u8; PUBLIC_IDENTITY_BYTES],
    service_name_hash: &'a [u8; SERVICE_NAME_HASH_BYTES],
    display_name: &'a str,
    identity_hash: [u8; HASH_BYTES],
    destination_hash: [u8; HASH_BYTES],
}

impl<'a> VerifiedContactCard<'a> {
    /// Original complete canonical card bytes.
    pub const fn wire(&self) -> &'a [u8] {
        self.wire
    }

    /// Public identity bound by the verified signature.
    pub const fn public_identity(&self) -> &'a [u8; PUBLIC_IDENTITY_BYTES] {
        self.public_identity
    }

    /// Service name hash bound by the verified signature.
    pub const fn service_name_hash(&self) -> &'a [u8; SERVICE_NAME_HASH_BYTES] {
        self.service_name_hash
    }

    /// Signed but self-asserted display name.
    pub const fn display_name(&self) -> &'a str {
        self.display_name
    }

    /// Identity hash derived from the complete public identity.
    pub const fn identity_hash(&self) -> &[u8; HASH_BYTES] {
        &self.identity_hash
    }

    /// Destination derived from the signed service and identity.
    pub const fn destination_hash(&self) -> &[u8; HASH_BYTES] {
        &self.destination_hash
    }
}

/// Derive Reticulum's service name hash from a fully qualified dotted name.
///
/// Naming/availability policy belongs to the application; no supported-service
/// registry is embedded in this crate.
#[must_use]
pub fn service_name_hash(dotted_name: &str) -> [u8; SERVICE_NAME_HASH_BYTES] {
    let hash = Sha256::digest(dotted_name.as_bytes());
    let mut result = [0_u8; SERVICE_NAME_HASH_BYTES];
    result.copy_from_slice(&hash[..SERVICE_NAME_HASH_BYTES]);
    result
}

/// Compose and verify a signed canonical card into caller-owned output.
///
/// A signer returning a signature for a different key is rejected. All input
/// checks, signing and verification occur before modifying `output`. Name bytes
/// are preserved exactly; no Unicode normalization is performed.
pub fn encode_signed<S: ContactCardSigner + ?Sized>(
    fields: CardFields<'_>,
    signer: &S,
    output: &mut [u8],
) -> Result<usize, EncodeError<S::Error>> {
    validate_display_name(fields.display_name).map_err(EncodeError::InvalidCard)?;
    let unsigned_len = HEADER_BYTES + fields.display_name.len();
    let required = unsigned_len + SIGNATURE_BYTES;
    if output.len() < required {
        return Err(EncodeError::OutputTooSmall {
            required,
            available: output.len(),
        });
    }
    let mut card = [0_u8; MAX_CARD_BYTES];
    card[..8].copy_from_slice(MAGIC);
    card[8] = VERSION;
    card[9] = fields.display_name.len() as u8;
    card[10..74].copy_from_slice(fields.public_identity);
    card[74..HEADER_BYTES].copy_from_slice(fields.service_name_hash);
    card[HEADER_BYTES..unsigned_len].copy_from_slice(fields.display_name.as_bytes());
    let mut signed = [0_u8; MAX_SIGNING_INPUT_BYTES];
    let signing_len = signature_input(&card[..unsigned_len], &mut signed);
    let signature = signer
        .sign(&signed[..signing_len])
        .map_err(EncodeError::Signer)?;
    card[unsigned_len..required].copy_from_slice(&signature);
    verify(&card[..required]).map_err(EncodeError::InvalidCard)?;
    output[..required].copy_from_slice(&card[..required]);
    Ok(required)
}

/// Parse and verify a complete canonical card with a strict Ed25519 signature.
///
/// Applications must still check whether they support the signed service. No
/// card, even a correctly signed one, grants authorization or establishes a
/// transport route. The X25519 half is bound by the signature and identity hash;
/// actual key agreement and live peer possession are checked by Reticulum.
pub fn verify(wire: &[u8]) -> Result<VerifiedContactCard<'_>, CardError> {
    if wire.len() < MIN_CARD_BYTES {
        return Err(CardError::Truncated);
    }
    if wire.len() > MAX_CARD_BYTES {
        return Err(CardError::TooLarge);
    }
    if &wire[..8] != MAGIC {
        return Err(CardError::InvalidMagic);
    }
    if wire[8] != VERSION {
        return Err(CardError::UnsupportedVersion);
    }
    let name_len = usize::from(wire[9]);
    if name_len > MAX_DISPLAY_NAME_BYTES {
        return Err(CardError::InvalidName);
    }
    let unsigned_len = HEADER_BYTES + name_len;
    if wire.len() != unsigned_len + SIGNATURE_BYTES {
        return Err(CardError::InvalidLength);
    }
    let display_name =
        str::from_utf8(&wire[HEADER_BYTES..unsigned_len]).map_err(|_| CardError::InvalidName)?;
    validate_display_name(display_name)?;
    let public_identity: &[u8; PUBLIC_IDENTITY_BYTES] = wire[10..74]
        .try_into()
        .map_err(|_| CardError::InvalidLength)?;
    let service_name_hash: &[u8; SERVICE_NAME_HASH_BYTES] = wire[74..HEADER_BYTES]
        .try_into()
        .map_err(|_| CardError::InvalidLength)?;
    let signing_key: &[u8; 32] = public_identity[32..]
        .try_into()
        .map_err(|_| CardError::InvalidPublicIdentity)?;
    if !prime_order_point(signing_key) {
        return Err(CardError::InvalidPublicIdentity);
    }
    let verifying_key =
        VerifyingKey::from_bytes(signing_key).map_err(|_| CardError::InvalidPublicIdentity)?;
    let signature_bytes: &[u8; SIGNATURE_BYTES] = wire[unsigned_len..]
        .try_into()
        .map_err(|_| CardError::InvalidSignature)?;
    let signature_point: &[u8; 32] = signature_bytes[..32]
        .try_into()
        .map_err(|_| CardError::InvalidSignature)?;
    if !prime_order_point(signature_point) {
        return Err(CardError::InvalidSignature);
    }
    let mut signed = [0_u8; MAX_SIGNING_INPUT_BYTES];
    let signing_len = signature_input(&wire[..unsigned_len], &mut signed);
    verifying_key
        .verify_strict(
            &signed[..signing_len],
            &Signature::from_bytes(signature_bytes),
        )
        .map_err(|_| CardError::InvalidSignature)?;
    let identity_digest = Sha256::digest(public_identity);
    let mut identity_hash = [0_u8; HASH_BYTES];
    identity_hash.copy_from_slice(&identity_digest[..HASH_BYTES]);
    let mut destination_hasher = Sha256::new();
    destination_hasher.update(service_name_hash);
    destination_hasher.update(identity_hash);
    let destination_digest = destination_hasher.finalize();
    let mut destination_hash = [0_u8; HASH_BYTES];
    destination_hash.copy_from_slice(&destination_digest[..HASH_BYTES]);
    Ok(VerifiedContactCard {
        wire,
        public_identity,
        service_name_hash,
        display_name,
        identity_hash,
        destination_hash,
    })
}

/// Check the codec's exact display-name bounds and prohibited characters.
/// Native owners use this before bounded admission; encoding repeats the same
/// validation before signing. No normalization or service policy is applied.
pub fn validate_display_name(name: &str) -> Result<(), CardError> {
    if name.len() > MAX_DISPLAY_NAME_BYTES
        || name.chars().any(|ch| {
            ch.is_control()
                || matches!(
                    ch,
                    '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
    {
        return Err(CardError::InvalidName);
    }
    Ok(())
}

fn signature_input(unsigned: &[u8], output: &mut [u8; MAX_SIGNING_INPUT_BYTES]) -> usize {
    let prefix = SIGNATURE_DOMAIN.len();
    output[..prefix].copy_from_slice(SIGNATURE_DOMAIN);
    output[prefix..prefix + unsigned.len()].copy_from_slice(unsigned);
    prefix + unsigned.len()
}

fn prime_order_point(bytes: &[u8; 32]) -> bool {
    CompressedEdwardsY(*bytes)
        .decompress()
        .is_some_and(|point| {
            point.compress().as_bytes() == bytes
                && !point.is_small_order()
                && point.is_torsion_free()
        })
}

#[cfg(test)]
mod tests;
