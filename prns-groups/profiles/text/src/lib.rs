//! Bounded UTF-8 text carried inside an authenticated generic group record.
//!
//! The outer record supplies identity, author, validity and delivery targets.
//! This profile adds neither encryption nor a second source for those facts.
#![no_std]
#![forbid(unsafe_code)]

use core::{fmt, str};
use prns_groups_core::{NamespaceId, SchemaId};

pub const NAMESPACE: NamespaceId = NamespaceId(0x7465_7874);
pub const SCHEMA: SchemaId = SchemaId(1);
pub const MAX_TEXT_BYTES: usize = 2048;
const HEADER_BYTES: usize = 6;
pub const MAX_ENCODED_BYTES: usize = HEADER_BYTES + MAX_TEXT_BYTES;
const MAGIC: [u8; 3] = *b"GTX";
const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Empty,
    TooLong,
    BufferTooSmall { required: usize },
    InvalidEncoding,
    UnsupportedVersion(u8),
    InvalidUtf8,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("text is empty"),
            Self::TooLong => formatter.write_str("text exceeds the profile byte limit"),
            Self::BufferTooSmall { required } => {
                write!(formatter, "text output needs {required} bytes")
            }
            Self::InvalidEncoding => formatter.write_str("invalid text profile encoding"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported text profile version {version}")
            }
            Self::InvalidUtf8 => formatter.write_str("text is not UTF-8"),
        }
    }
}
impl core::error::Error for Error {}

/// Validated text borrowing its caller's storage. Whitespace and Unicode are
/// preserved exactly; presentation and normalization are application decisions.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Text<'a>(&'a str);

impl<'a> Text<'a> {
    pub fn new(text: &'a str) -> Result<Self, Error> {
        if text.is_empty() {
            return Err(Error::Empty);
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(Error::TooLong);
        }
        Ok(Self(text))
    }

    pub const fn as_str(self) -> &'a str {
        self.0
    }

    pub const fn encoded_len(self) -> usize {
        HEADER_BYTES + self.0.len()
    }

    /// Validates capacity before modifying output. Returns only initialized bytes.
    pub fn encode(self, output: &mut [u8]) -> Result<&[u8], Error> {
        let required = self.encoded_len();
        let output = output
            .get_mut(..required)
            .ok_or(Error::BufferTooSmall { required })?;
        output[..3].copy_from_slice(&MAGIC);
        output[3] = VERSION;
        output[4..HEADER_BYTES].copy_from_slice(&(self.0.len() as u16).to_be_bytes());
        output[HEADER_BYTES..].copy_from_slice(self.0.as_bytes());
        Ok(output)
    }

    /// Exact framing rejects trailing bytes, truncation and unsupported versions.
    pub fn decode(encoded: &'a [u8]) -> Result<Self, Error> {
        if encoded.len() < HEADER_BYTES || encoded[..3] != MAGIC {
            return Err(Error::InvalidEncoding);
        }
        if encoded[3] != VERSION {
            return Err(Error::UnsupportedVersion(encoded[3]));
        }
        let length = usize::from(u16::from_be_bytes([encoded[4], encoded[5]]));
        if length > MAX_TEXT_BYTES {
            return Err(Error::TooLong);
        }
        if encoded.len() != HEADER_BYTES + length {
            return Err(Error::InvalidEncoding);
        }
        Self::new(str::from_utf8(&encoded[HEADER_BYTES..]).map_err(|_| Error::InvalidUtf8)?)
    }
}

impl fmt::Debug for Text<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Text")
            .field("bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn stable_fixture_and_borrowed_unicode_roundtrip() {
        let mut storage = [0; MAX_ENCODED_BYTES];
        let text = Text::new("a🌎\n").unwrap();
        let encoded = text.encode(&mut storage).unwrap();
        assert_eq!(encoded, b"GTX\x01\x00\x06a\xf0\x9f\x8c\x8e\n");
        let decoded = Text::decode(encoded).unwrap();
        assert_eq!(decoded, text);
        assert_eq!(decoded.as_str().as_ptr(), encoded[HEADER_BYTES..].as_ptr());
    }

    #[test]
    fn exact_byte_limit_and_preserved_whitespace() {
        let maximum = [b'a'; MAX_TEXT_BYTES];
        let text = Text::new(str::from_utf8(&maximum).unwrap()).unwrap();
        let mut encoded = [0; MAX_ENCODED_BYTES];
        assert_eq!(Text::decode(text.encode(&mut encoded).unwrap()), Ok(text));
        let oversized = [b'a'; MAX_TEXT_BYTES + 1];
        assert_eq!(
            Text::new(str::from_utf8(&oversized).unwrap()),
            Err(Error::TooLong)
        );
        assert_eq!(Text::new(""), Err(Error::Empty));
        assert_eq!(Text::new("  \n").unwrap().as_str(), "  \n");
    }

    #[test]
    fn insufficient_output_is_unchanged() {
        let mut output = [0xaa; HEADER_BYTES];
        assert_eq!(
            Text::new("a").unwrap().encode(&mut output),
            Err(Error::BufferTooSmall { required: 7 })
        );
        assert_eq!(output, [0xaa; HEADER_BYTES]);
    }

    #[test]
    fn rejects_ambiguous_or_invalid_inputs() {
        let valid = b"GTX\x01\x00\x01a";
        for end in 0..valid.len() {
            assert_eq!(Text::decode(&valid[..end]), Err(Error::InvalidEncoding));
        }
        assert_eq!(
            Text::decode(b"GTX\x01\x00\x01ab"),
            Err(Error::InvalidEncoding)
        );
        assert_eq!(
            Text::decode(b"GTX\x02\x00\x01a"),
            Err(Error::UnsupportedVersion(2))
        );
        assert_eq!(
            Text::decode(b"GTX\x01\x00\x01\xff"),
            Err(Error::InvalidUtf8)
        );
        assert_eq!(Text::decode(b"GTX\x01\x08\x01"), Err(Error::TooLong));
        assert_eq!(Text::decode(b"GTX\x01\x00\x00"), Err(Error::Empty));
    }
}
