use crate::{MAX_CARD_BYTES, MAX_TOKEN_BYTES, MAX_URI_BYTES, MIN_CARD_BYTES, URI_PREFIX};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Canonical transport-wrapper failure, before signature verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Error))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "ContactCardTokenError", flat_error)
)]
pub enum TokenError {
    /// Input does not have a possible bounded card length.
    InvalidLength,
    /// Input exceeds the fixed bound.
    TooLarge,
    /// Alphabet contains padding, whitespace or another disallowed byte.
    InvalidCharacter,
    /// Unused trailing bits are nonzero, creating an alternate spelling.
    NonCanonical,
    /// Caller-owned output cannot hold the result.
    OutputTooSmall { required: usize, available: usize },
    /// The URI scheme, version, or exact prefix was not recognized.
    InvalidUri,
}

impl core::fmt::Display for TokenError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl core::error::Error for TokenError {}

/// Encode bounded wire bytes as unpadded base64url, without signature checking.
///
/// Call [`crate::verify`] before treating decoded bytes as a contact. The output
/// is ASCII; callers may convert the successful prefix with `core::str::from_utf8`.
/// Output remains unchanged on failure.
pub fn encode_token(wire: &[u8], output: &mut [u8]) -> Result<usize, TokenError> {
    check_wire_length(wire.len())?;
    let required = (wire.len() * 4).div_ceil(3);
    if output.len() < required {
        return Err(TokenError::OutputTooSmall {
            required,
            available: output.len(),
        });
    }
    let mut cursor = 0;
    for chunk in wire.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output[cursor] = ALPHABET[usize::from(a >> 2)];
        output[cursor + 1] = ALPHABET[usize::from(((a & 3) << 4) | (b >> 4))];
        cursor += 2;
        if chunk.len() > 1 {
            output[cursor] = ALPHABET[usize::from(((b & 15) << 2) | (c >> 6))];
            cursor += 1;
        }
        if chunk.len() > 2 {
            output[cursor] = ALPHABET[usize::from(c & 63)];
            cursor += 1;
        }
    }
    Ok(cursor)
}

/// Decode one exact unpadded base64url token into caller-owned wire storage.
///
/// Padding, whitespace, alternate trailing bits, escaped characters and lengths
/// outside card bounds are rejected. This checks the carrier only; the returned
/// wire bytes still require [`crate::verify`]. Output is unchanged on failure.
pub fn decode_token(token: &str, output: &mut [u8]) -> Result<usize, TokenError> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(TokenError::TooLarge);
    }
    let remainder = token.len() % 4;
    if remainder == 1 {
        return Err(TokenError::InvalidLength);
    }
    let required = token.len() / 4 * 3 + remainder.saturating_sub(1);
    check_wire_length(required)?;
    if output.len() < required {
        return Err(TokenError::OutputTooSmall {
            required,
            available: output.len(),
        });
    }
    // Validate the whole token before touching caller-owned storage.
    let mut values = [0_u8; MAX_TOKEN_BYTES];
    for (index, byte) in token.bytes().enumerate() {
        values[index] = decode_digit(byte).ok_or(TokenError::InvalidCharacter)?;
    }
    if (remainder == 2 && values[token.len() - 1] & 15 != 0)
        || (remainder == 3 && values[token.len() - 1] & 3 != 0)
    {
        return Err(TokenError::NonCanonical);
    }
    let mut cursor = 0;
    for chunk in values[..token.len()].chunks(4) {
        output[cursor] = (chunk[0] << 2) | (chunk[1] >> 4);
        cursor += 1;
        if chunk.len() > 2 {
            output[cursor] = (chunk[1] << 4) | (chunk[2] >> 2);
            cursor += 1;
        }
        if chunk.len() > 3 {
            output[cursor] = (chunk[2] << 6) | chunk[3];
            cursor += 1;
        }
    }
    Ok(cursor)
}

/// Encode the portable draft `rns-contact://v1/<token>` URI.
///
/// This does not select or register an application, host a website, carry private
/// keys, grant membership, or change Reticulum's wire protocol. No query or
/// fragment is part of this format. Output remains unchanged on failure.
pub fn encode_uri(wire: &[u8], output: &mut [u8]) -> Result<usize, TokenError> {
    check_wire_length(wire.len())?;
    let required = URI_PREFIX.len() + (wire.len() * 4).div_ceil(3);
    if output.len() < required {
        return Err(TokenError::OutputTooSmall {
            required,
            available: output.len(),
        });
    }
    encode_token(wire, &mut output[URI_PREFIX.len()..])?;
    output[..URI_PREFIX.len()].copy_from_slice(URI_PREFIX.as_bytes());
    Ok(required)
}

/// Decode the exact portable draft URI, without verifying its card signature.
///
/// Unknown versions, alternate casing, query/fragment suffixes and percent
/// encodings are rejected. Generic URL parsers must not normalize the input
/// before this call. Output remains unchanged on failure.
pub fn decode_uri(uri: &str, output: &mut [u8]) -> Result<usize, TokenError> {
    if uri.len() > MAX_URI_BYTES {
        return Err(TokenError::TooLarge);
    }
    let token = uri.strip_prefix(URI_PREFIX).ok_or(TokenError::InvalidUri)?;
    decode_token(token, output)
}

fn check_wire_length(len: usize) -> Result<(), TokenError> {
    if len > MAX_CARD_BYTES {
        Err(TokenError::TooLarge)
    } else if len < MIN_CARD_BYTES {
        Err(TokenError::InvalidLength)
    } else {
        Ok(())
    }
}

fn decode_digit(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}
