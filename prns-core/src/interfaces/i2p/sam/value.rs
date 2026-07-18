use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::{alphabet, Engine as _};
use data_encoding::BASE32_NOPAD;
use zeroize::{Zeroize, Zeroizing};

pub const I2PLIB_PRIVATE_DESTINATION_MIN_DECODED_BYTES: usize = 387;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2pDestinationKind {
    Public,
    Private,
}

impl fmt::Display for I2pDestinationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Public => formatter.write_str("public"),
            Self::Private => formatter.write_str("private"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamValueError {
    Empty,
    UnsafeCharacter(char),
    DestinationTooShort {
        kind: I2pDestinationKind,
        minimum: usize,
        actual: usize,
    },
    InvalidDestinationCharacter {
        kind: I2pDestinationKind,
        character: char,
    },
    InvalidDestinationLength {
        kind: I2pDestinationKind,
        length: usize,
    },
    InvalidDestinationPadding {
        kind: I2pDestinationKind,
    },
    InvalidDestinationEncoding {
        kind: I2pDestinationKind,
    },
    DestinationCertificateTruncated {
        certificate_length: usize,
        decoded_length: usize,
    },
    InvalidBase32Address,
}

impl fmt::Display for SamValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("SAM value is empty"),
            Self::UnsafeCharacter(character) => {
                write!(
                    formatter,
                    "SAM value contains unsafe character {character:?}"
                )
            }
            Self::DestinationTooShort {
                kind,
                minimum,
                actual,
            } => write!(
                formatter,
                "I2P {kind} destination is {actual} bytes; expected at least {minimum}"
            ),
            Self::InvalidDestinationCharacter { kind, character } => write!(
                formatter,
                "I2P {kind} destination contains invalid base64 character {character:?}"
            ),
            Self::InvalidDestinationLength { kind, length } => write!(
                formatter,
                "I2P {kind} destination has invalid base64 length {length}"
            ),
            Self::InvalidDestinationPadding { kind } => {
                write!(
                    formatter,
                    "I2P {kind} destination has invalid base64 padding"
                )
            }
            Self::InvalidDestinationEncoding { kind } => {
                write!(formatter, "I2P {kind} destination has invalid base64 encoding")
            }
            Self::DestinationCertificateTruncated {
                certificate_length,
                decoded_length,
            } => write!(
                formatter,
                "I2P private destination declares a {certificate_length}-byte certificate but contains only {decoded_length} decoded bytes"
            ),
            Self::InvalidBase32Address => formatter.write_str(
                "I2P base32 address must be 52 lowercase base32 characters followed by .b32.i2p",
            ),
        }
    }
}

impl core::error::Error for SamValueError {}

macro_rules! sam_token {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, SamValueError> {
                let value = value.into();
                validate_sam_value(&value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = SamValueError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
    };
}

sam_token!(SamSessionId);
sam_token!(I2pAddress);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct I2pPublicDestination(String);

impl I2pPublicDestination {
    pub fn new(value: impl Into<String>) -> Result<Self, SamValueError> {
        let value = value.into();
        validate_i2p_destination(&value, I2pDestinationKind::Public, None)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn base32_address(&self) -> Result<I2pBase32Address, SamValueError> {
        let decoded = decode_destination(&self.0, I2pDestinationKind::Public)?;
        let digest = crate::crypto::sha256(&decoded);
        let label = BASE32_NOPAD.encode(&digest).to_ascii_lowercase();
        I2pBase32Address::new(format!("{label}.b32.i2p"))
    }
}

impl TryFrom<String> for I2pPublicDestination {
    type Error = SamValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct I2pPrivateDestination(String);

impl I2pPrivateDestination {
    pub fn new(value: impl Into<String>) -> Result<Self, SamValueError> {
        let value = value.into();
        validate_i2p_destination(
            &value,
            I2pDestinationKind::Private,
            Some(I2PLIB_PRIVATE_DESTINATION_MIN_DECODED_BYTES),
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn public_destination(&self) -> Result<I2pPublicDestination, SamValueError> {
        let decoded = Zeroizing::new(decode_destination(&self.0, I2pDestinationKind::Private)?);
        let certificate_length = usize::from(u16::from_be_bytes([decoded[385], decoded[386]]));
        let public_length = 387usize.saturating_add(certificate_length);
        if public_length > decoded.len() {
            return Err(SamValueError::DestinationCertificateTruncated {
                certificate_length,
                decoded_length: decoded.len(),
            });
        }
        I2pPublicDestination::new(encode_destination(&decoded[..public_length]))
    }
}

impl TryFrom<String> for I2pPrivateDestination {
    type Error = SamValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Debug for I2pPrivateDestination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("I2pPrivateDestination")
            .field(&"[REDACTED]")
            .finish()
    }
}

impl Drop for I2pPrivateDestination {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct I2pBase32Address(String);

impl I2pBase32Address {
    pub fn new(value: impl Into<String>) -> Result<Self, SamValueError> {
        let value = value.into();
        let Some(label) = value.strip_suffix(".b32.i2p") else {
            return Err(SamValueError::InvalidBase32Address);
        };
        if label.len() != 52
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || matches!(byte, b'2'..=b'7'))
        {
            return Err(SamValueError::InvalidBase32Address);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for I2pBase32Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn validate_sam_value(value: &str) -> Result<(), SamValueError> {
    if value.is_empty() {
        return Err(SamValueError::Empty);
    }
    if let Some(character) = value.chars().find(|character| {
        character.is_whitespace() || character.is_control() || matches!(character, '"' | '\\')
    }) {
        return Err(SamValueError::UnsafeCharacter(character));
    }
    Ok(())
}

fn validate_i2p_destination(
    value: &str,
    kind: I2pDestinationKind,
    minimum_decoded_bytes: Option<usize>,
) -> Result<(), SamValueError> {
    validate_sam_value(value)?;
    if let Some(character) = value.chars().find(|character| {
        !character.is_ascii_alphanumeric() && !matches!(character, '+' | '/' | '-' | '~' | '=')
    }) {
        return Err(SamValueError::InvalidDestinationCharacter { kind, character });
    }
    if !value.len().is_multiple_of(4) {
        return Err(SamValueError::InvalidDestinationLength {
            kind,
            length: value.len(),
        });
    }
    let padding = value.bytes().rev().take_while(|byte| *byte == b'=').count();
    if padding > 2 || value.as_bytes()[..value.len() - padding].contains(&b'=') {
        return Err(SamValueError::InvalidDestinationPadding { kind });
    }
    let decoded_bytes = value.len() / 4 * 3 - padding;
    if let Some(minimum) = minimum_decoded_bytes {
        if decoded_bytes < minimum {
            return Err(SamValueError::DestinationTooShort {
                kind,
                minimum,
                actual: decoded_bytes,
            });
        }
    }
    Ok(())
}

fn decode_destination(value: &str, kind: I2pDestinationKind) -> Result<Vec<u8>, SamValueError> {
    let standard = value.replace('-', "+").replace('~', "/");
    GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
    )
    .decode(standard)
    .map_err(|_| SamValueError::InvalidDestinationEncoding { kind })
}

fn encode_destination(value: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(value)
        .replace('+', "-")
        .replace('/', "~")
}
