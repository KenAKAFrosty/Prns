//! Bounded public-data interchange for independent RemoteControl controllers.
//!
//! Version 1 binary cards begin with ASCII `RCP`, version byte `1`, and role byte
//! `1` (controller) or `2` (target access). A controller card then contains its
//! 64-byte public key: X25519 encryption key followed by Ed25519 signing key.
//! A target-access card contains, in order, the recipient controller's 64-byte
//! public key, the target's 64-byte public key, one authority byte, one request
//! count byte, and that many strictly increasing request-kind wire bytes.
//! Authority and request values are the existing RemoteControl wire values.
//! No omitted, duplicate, unknown or trailing fields are accepted. The request
//! set is the explicitly recorded set, not its authority-derived expansion.
//!
//! The text carrier is ASCII `prns-rc:` followed by hex of the entire binary card.
//! Encoders emit lowercase hex; decoders also accept uppercase hex digits. No
//! whitespace, app URI, labels, private keys, signatures or trust assertions are
//! carried. These cards neither authorize a controller nor prove a durable grant.
//! Callers must verify the source through their chosen trust channel and record
//! verification provenance separately. Before storing imported target access,
//! use `RemoteControlTargetAccessCard::into_access_for` with the local controller.
//!
//! All storage is inline or caller-provided. Each type exposes its current exact
//! maximum binary/text buffer size. Unknown versions, authority and request kinds
//! are rejected rather than downgraded or silently dropped.

#[cfg(feature = "alloc")]
mod owned;
#[cfg(feature = "alloc")]
pub use owned::*;

use crate::crypto::{x25519_diffie_hellman, X25519SecretKey};
use crate::identity::{IdentityPublicKeys, PublicIdentityMaterial, IDENTITY_PUBLIC_KEY_LEN};

use super::{
    RemoteControlControllerAuthority, RemoteControlControllerIdentity, RemoteControlRequestKind,
    RemoteControlRequestSet, RemoteControlTargetAccess, RemoteControlTargetAccessError,
    RemoteControlTargetIdentity,
};

const MAGIC: &[u8; 3] = b"RCP";
const VERSION: u8 = 1;
const CONTROLLER_ROLE: u8 = 1;
const TARGET_ACCESS_ROLE: u8 = 2;
const HEADER_LEN: usize = 5;
const TEXT_PREFIX: &[u8; 8] = b"prns-rc:";

/// The identity field whose key failed public-card validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPublicCardIdentityRole {
    Controller,
    Recipient,
    Target,
}

/// Malformed or cryptographically unusable public-key component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPublicCardKeyComponent {
    Encryption,
    Signing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPublicCardError {
    BufferTooShort,
    InvalidLength,
    InvalidMagic,
    UnsupportedVersion {
        found: u8,
    },
    UnexpectedRole {
        found: u8,
    },
    InvalidTextEnvelope,
    InvalidHex,
    InvalidPublicKey {
        role: RemoteControlPublicCardIdentityRole,
        component: RemoteControlPublicCardKeyComponent,
    },
    UnknownAuthority {
        found: u8,
    },
    UnknownRequest {
        found: u8,
    },
    DuplicateRequest {
        request: RemoteControlRequestKind,
    },
    NonCanonicalRequestOrder,
    InvalidTargetAccess(RemoteControlTargetAccessError),
    RecipientMismatch,
}

impl core::fmt::Display for RemoteControlPublicCardError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BufferTooShort => formatter.write_str("public card output buffer is too short"),
            Self::InvalidLength => formatter.write_str("public card length is invalid"),
            Self::InvalidMagic => formatter.write_str("public card header is invalid"),
            Self::UnsupportedVersion { found } => {
                write!(formatter, "unsupported public card version {found}")
            }
            Self::UnexpectedRole { found } => {
                write!(formatter, "unexpected public card role {found}")
            }
            Self::InvalidTextEnvelope => formatter.write_str("public card text prefix is invalid"),
            Self::InvalidHex => formatter.write_str("public card contains invalid hex"),
            Self::InvalidPublicKey { role, component } => {
                write!(formatter, "invalid {component:?} public key for {role:?}")
            }
            Self::UnknownAuthority { found } => {
                write!(formatter, "unknown public card authority {found}")
            }
            Self::UnknownRequest { found } => {
                write!(formatter, "unknown public card request {found}")
            }
            Self::DuplicateRequest { request } => {
                write!(formatter, "duplicate public card request {request:?}")
            }
            Self::NonCanonicalRequestOrder => {
                formatter.write_str("public card requests are not in increasing order")
            }
            Self::InvalidTargetAccess(error) => {
                write!(formatter, "invalid public card access: {error:?}")
            }
            Self::RecipientMismatch => {
                formatter.write_str("public card belongs to a different controller")
            }
        }
    }
}

impl core::error::Error for RemoteControlPublicCardError {}

/// Public identity export. Possession of this card grants no access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerCard {
    controller: RemoteControlControllerIdentity,
}

impl RemoteControlControllerCard {
    pub const MAX_ENCODED_LEN: usize = HEADER_LEN.saturating_add(IDENTITY_PUBLIC_KEY_LEN);
    pub const MAX_TEXT_LEN: usize = TEXT_PREFIX
        .len()
        .saturating_add(Self::MAX_ENCODED_LEN.saturating_mul(2));

    pub fn new(
        controller: RemoteControlControllerIdentity,
    ) -> Result<Self, RemoteControlPublicCardError> {
        validate_keys(
            controller.public_keys(),
            RemoteControlPublicCardIdentityRole::Controller,
        )?;
        Ok(Self { controller })
    }

    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }

    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        Self::MAX_ENCODED_LEN
    }

    #[must_use]
    pub const fn text_len(&self) -> usize {
        Self::MAX_TEXT_LEN
    }

    pub fn encode(&self, output: &mut [u8]) -> Result<usize, RemoteControlPublicCardError> {
        let mut writer = Writer::new(output, self.encoded_len())?;
        writer.header(CONTROLLER_ROLE)?;
        writer.write(&self.controller.public_keys().public_key_bytes())?;
        Ok(self.encoded_len())
    }

    pub fn decode(input: &[u8]) -> Result<Self, RemoteControlPublicCardError> {
        if input.len() != Self::MAX_ENCODED_LEN {
            return Err(RemoteControlPublicCardError::InvalidLength);
        }
        let mut reader = Reader::new(input);
        reader.header(CONTROLLER_ROLE)?;
        let controller = RemoteControlControllerIdentity::new(reader.keys()?);
        reader.finish()?;
        Self::new(controller)
    }

    pub fn encode_text(&self, output: &mut [u8]) -> Result<usize, RemoteControlPublicCardError> {
        let mut binary = [0; Self::MAX_ENCODED_LEN];
        let len = self.encode(&mut binary)?;
        encode_text(
            binary
                .get(..len)
                .ok_or(RemoteControlPublicCardError::InvalidLength)?,
            output,
        )
    }

    pub fn decode_text(input: &str) -> Result<Self, RemoteControlPublicCardError> {
        let mut binary = [0; Self::MAX_ENCODED_LEN];
        let len = decode_text(input, &mut binary)?;
        Self::decode(
            binary
                .get(..len)
                .ok_or(RemoteControlPublicCardError::InvalidLength)?,
        )
    }
}

/// Recipient-bound access information, not a grant receipt or proof of authority.
#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlTargetAccessCard {
    recipient: RemoteControlControllerIdentity,
    access: RemoteControlTargetAccess,
}

impl RemoteControlTargetAccessCard {
    const FIXED_ENCODED_LEN: usize = HEADER_LEN
        .saturating_add(IDENTITY_PUBLIC_KEY_LEN.saturating_mul(2))
        .saturating_add(2);
    pub const MAX_ENCODED_LEN: usize =
        Self::FIXED_ENCODED_LEN.saturating_add(RemoteControlRequestKind::ALL.len());
    pub const MAX_TEXT_LEN: usize = TEXT_PREFIX
        .len()
        .saturating_add(Self::MAX_ENCODED_LEN.saturating_mul(2));

    pub fn new(
        recipient: RemoteControlControllerIdentity,
        access: RemoteControlTargetAccess,
    ) -> Result<Self, RemoteControlPublicCardError> {
        validate_keys(
            recipient.public_keys(),
            RemoteControlPublicCardIdentityRole::Recipient,
        )?;
        validate_keys(
            access.target().public_keys(),
            RemoteControlPublicCardIdentityRole::Target,
        )?;
        Ok(Self { recipient, access })
    }

    #[must_use]
    pub const fn recipient(&self) -> &RemoteControlControllerIdentity {
        &self.recipient
    }

    #[must_use]
    pub const fn access(&self) -> &RemoteControlTargetAccess {
        &self.access
    }

    /// Consume the public card without copying its move-only access value.
    #[must_use]
    pub fn into_parts(self) -> (RemoteControlControllerIdentity, RemoteControlTargetAccess) {
        (self.recipient, self.access)
    }

    /// Checks the entire recipient public key before releasing access for storage.
    /// The caller must separately establish the card's trusted source.
    pub fn into_access_for(
        self,
        controller: &RemoteControlControllerIdentity,
    ) -> Result<RemoteControlTargetAccess, RemoteControlPublicCardError> {
        if &self.recipient != controller {
            return Err(RemoteControlPublicCardError::RecipientMismatch);
        }
        Ok(self.access)
    }

    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        Self::FIXED_ENCODED_LEN.saturating_add(self.access.permitted_requests().len())
    }

    #[must_use]
    pub const fn text_len(&self) -> usize {
        TEXT_PREFIX
            .len()
            .saturating_add(self.encoded_len().saturating_mul(2))
    }

    pub fn encode(&self, output: &mut [u8]) -> Result<usize, RemoteControlPublicCardError> {
        let mut writer = Writer::new(output, self.encoded_len())?;
        writer.header(TARGET_ACCESS_ROLE)?;
        writer.write(&self.recipient.public_keys().public_key_bytes())?;
        writer.write(&self.access.target().public_keys().public_key_bytes())?;
        writer.write(&[
            self.access.authority().wire_value(),
            self.access.permitted_requests().wire_count(),
        ])?;
        for request in self.access.permitted_requests().iter() {
            writer.write(&[request.wire_value()])?;
        }
        Ok(self.encoded_len())
    }

    pub fn decode(input: &[u8]) -> Result<Self, RemoteControlPublicCardError> {
        if input.len() > Self::MAX_ENCODED_LEN {
            return Err(RemoteControlPublicCardError::InvalidLength);
        }
        let mut reader = Reader::new(input);
        reader.header(TARGET_ACCESS_ROLE)?;
        let recipient = RemoteControlControllerIdentity::new(reader.keys()?);
        let target = RemoteControlTargetIdentity::new(reader.keys()?);
        let authority = reader.byte()?;
        let authority = RemoteControlControllerAuthority::from_wire(authority)
            .ok_or(RemoteControlPublicCardError::UnknownAuthority { found: authority })?;
        let count = reader.byte()?;
        if usize::from(count) != reader.remaining.len() {
            return Err(RemoteControlPublicCardError::InvalidLength);
        }
        let mut requests = RemoteControlRequestSet::empty();
        let mut previous = 0;
        for &value in reader.remaining {
            let request = RemoteControlRequestKind::from_wire(value)
                .ok_or(RemoteControlPublicCardError::UnknownRequest { found: value })?;
            if !requests.insert(request) {
                return Err(RemoteControlPublicCardError::DuplicateRequest { request });
            }
            if value <= previous {
                return Err(RemoteControlPublicCardError::NonCanonicalRequestOrder);
            }
            previous = value;
        }
        let access = RemoteControlTargetAccess::new(target, authority, requests)
            .map_err(RemoteControlPublicCardError::InvalidTargetAccess)?;
        Self::new(recipient, access)
    }

    pub fn encode_text(&self, output: &mut [u8]) -> Result<usize, RemoteControlPublicCardError> {
        let mut binary = [0; Self::MAX_ENCODED_LEN];
        let len = self.encode(&mut binary)?;
        encode_text(
            binary
                .get(..len)
                .ok_or(RemoteControlPublicCardError::InvalidLength)?,
            output,
        )
    }

    pub fn decode_text(input: &str) -> Result<Self, RemoteControlPublicCardError> {
        let mut binary = [0; Self::MAX_ENCODED_LEN];
        let len = decode_text(input, &mut binary)?;
        Self::decode(
            binary
                .get(..len)
                .ok_or(RemoteControlPublicCardError::InvalidLength)?,
        )
    }
}

fn validate_keys(
    keys: &IdentityPublicKeys,
    role: RemoteControlPublicCardIdentityRole,
) -> Result<(), RemoteControlPublicCardError> {
    let signing_error = RemoteControlPublicCardError::InvalidPublicKey {
        role,
        component: RemoteControlPublicCardKeyComponent::Signing,
    };
    let signing = ed25519_dalek::VerifyingKey::from_bytes(&keys.signing.as_ed25519().0)
        .map_err(|_| signing_error)?;
    if signing.is_weak() {
        return Err(signing_error);
    }
    // Public fixed scalar: this only rejects non-contributory X25519 points. It
    // is not a session key or proof that the card owner holds a private key.
    let probe = x25519_diffie_hellman(&X25519SecretKey::new([0; 32]), keys.encryption.as_x25519());
    if probe.as_bytes().iter().all(|byte| *byte == 0) {
        return Err(RemoteControlPublicCardError::InvalidPublicKey {
            role,
            component: RemoteControlPublicCardKeyComponent::Encryption,
        });
    }
    Ok(())
}

struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self { remaining: input }
    }

    fn take<const N: usize>(&mut self) -> Result<&'a [u8; N], RemoteControlPublicCardError> {
        let (value, rest) = self
            .remaining
            .split_at_checked(N)
            .ok_or(RemoteControlPublicCardError::InvalidLength)?;
        self.remaining = rest;
        value
            .try_into()
            .map_err(|_| RemoteControlPublicCardError::InvalidLength)
    }

    fn byte(&mut self) -> Result<u8, RemoteControlPublicCardError> {
        let &[value] = self.take()?;
        Ok(value)
    }

    fn header(&mut self, role: u8) -> Result<(), RemoteControlPublicCardError> {
        if self.take::<3>()? != MAGIC {
            return Err(RemoteControlPublicCardError::InvalidMagic);
        }
        let version = self.byte()?;
        if version != VERSION {
            return Err(RemoteControlPublicCardError::UnsupportedVersion { found: version });
        }
        let found = self.byte()?;
        if found != role {
            return Err(RemoteControlPublicCardError::UnexpectedRole { found });
        }
        Ok(())
    }

    fn keys(&mut self) -> Result<IdentityPublicKeys, RemoteControlPublicCardError> {
        Ok(PublicIdentityMaterial::from_bytes(*self.take()?).public_keys())
    }

    fn finish(&self) -> Result<(), RemoteControlPublicCardError> {
        if !self.remaining.is_empty() {
            return Err(RemoteControlPublicCardError::InvalidLength);
        }
        Ok(())
    }
}

struct Writer<'a> {
    output: &'a mut [u8],
    written: usize,
}

impl<'a> Writer<'a> {
    fn new(output: &'a mut [u8], len: usize) -> Result<Self, RemoteControlPublicCardError> {
        let output = output
            .get_mut(..len)
            .ok_or(RemoteControlPublicCardError::BufferTooShort)?;
        Ok(Self { output, written: 0 })
    }

    fn header(&mut self, role: u8) -> Result<(), RemoteControlPublicCardError> {
        self.write(MAGIC)?;
        self.write(&[VERSION, role])
    }

    fn write(&mut self, input: &[u8]) -> Result<(), RemoteControlPublicCardError> {
        let end = self
            .written
            .checked_add(input.len())
            .ok_or(RemoteControlPublicCardError::BufferTooShort)?;
        let output = self
            .output
            .get_mut(self.written..end)
            .ok_or(RemoteControlPublicCardError::BufferTooShort)?;
        output.copy_from_slice(input);
        self.written = end;
        Ok(())
    }
}

fn encode_text(binary: &[u8], output: &mut [u8]) -> Result<usize, RemoteControlPublicCardError> {
    let len = TEXT_PREFIX
        .len()
        .saturating_add(binary.len().saturating_mul(2));
    let mut writer = Writer::new(output, len)?;
    writer.write(TEXT_PREFIX)?;
    for &byte in binary {
        writer.write(&[hex_digit(byte >> 4), hex_digit(byte & 0x0f)])?;
    }
    Ok(len)
}

fn hex_digit(value: u8) -> u8 {
    if value < 10 {
        b'0'.saturating_add(value)
    } else {
        b'a'.saturating_add(value.saturating_sub(10))
    }
}

fn decode_text(input: &str, output: &mut [u8]) -> Result<usize, RemoteControlPublicCardError> {
    let hex = input
        .as_bytes()
        .strip_prefix(TEXT_PREFIX)
        .ok_or(RemoteControlPublicCardError::InvalidTextEnvelope)?;
    if hex.len() % 2 != 0 {
        return Err(RemoteControlPublicCardError::InvalidLength);
    }
    let len = hex.len() / 2;
    let output = output
        .get_mut(..len)
        .ok_or(RemoteControlPublicCardError::InvalidLength)?;
    for (&[high, low], byte) in hex.as_chunks::<2>().0.iter().zip(output) {
        *byte = (hex_value(high)? << 4) | hex_value(low)?;
    }
    Ok(len)
}

fn hex_value(value: u8) -> Result<u8, RemoteControlPublicCardError> {
    match value {
        b'0'..=b'9' => Ok(value.saturating_sub(b'0')),
        b'a'..=b'f' => Ok(value.saturating_sub(b'a').saturating_add(10)),
        b'A'..=b'F' => Ok(value.saturating_sub(b'A').saturating_add(10)),
        _ => Err(RemoteControlPublicCardError::InvalidHex),
    }
}

#[cfg(test)]
mod tests;
