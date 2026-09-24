//! Allocation-free Python-compatible composition of basic LXMF messages.

use sha2::{Digest, Sha256};

use crate::{
    encoded_basic_lxmf_payload_len, validate_messagepack_value, Canonicality, MessagePackKind,
    WireLimits, DESTINATION_HASH_LENGTH, LINK_PACKET_MAX_CONTENT, WIRE_HEADER_LENGTH,
};

const PAYLOAD_ARRAY_4: u8 = 0x94;
const MSGPACK_F64: u8 = 0xcb;
const EMPTY_FIELDS: [u8; 1] = [0x80];
const COMPLETE_WIRE_PREFIX_BYTES: usize = WIRE_HEADER_LENGTH;
const SIGNATURE_INPUT_SUFFIX_BYTES: usize = 32;

/// Largest complete normalized LXMF wire carried by one direct Link DATA packet.
pub const MAX_BASIC_LXMF_WIRE_BYTES: usize = 431;

/// Largest whole-millisecond timestamp whose adjacent values remain distinct
/// after Python LXMF converts them to binary64 seconds.
pub const MAX_LXMF_TIMESTAMP_UNIX_MS: u64 = (1_u64 << 43) * 1_000 - 1;

/// Product identity operation required to sign one LXMF message.
///
/// The signer is deliberately independent of Reticulum engines and secret-key
/// representations. Firmware adapts its ordinary PRNS identity signer here;
/// host tests can use any Ed25519 implementation.
pub trait BasicLxmfSigner {
    /// Sign exact Python LXMF input bytes and return the 64-byte Ed25519 wire form.
    fn sign_lxmf(&self, input: &[u8]) -> [u8; 64];
}

/// Successful composition of one complete basic LXMF message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedBasicLxmf {
    wire_len: u16,
    message_id: [u8; 32],
}

impl PreparedBasicLxmf {
    /// Bytes written to the caller's complete-wire buffer.
    pub const fn wire_len(self) -> u16 {
        self.wire_len
    }

    /// Python-compatible LXMF message identifier.
    pub const fn message_id(self) -> [u8; 32] {
        self.message_id
    }
}

/// Why a basic LXMF message was not composed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasicLxmfComposeError {
    /// The timestamp exceeds the injective binary64-millisecond subset.
    TimestampTooLarge { actual: u64, maximum: u64 },
    /// The supplied fields value is not one canonical MessagePack map.
    InvalidFields,
    /// Length arithmetic or the fixed internal product bound was exceeded.
    MessageTooLarge,
    /// Python LXMF selects a Resource because the complete direct wire exceeds 431 bytes.
    NeedsResource { wire_bytes: usize },
    /// Caller-owned output cannot hold the complete signed wire message.
    OutputTooSmall { required: usize, available: usize },
}

/// Compose one unstamped Python LXMF direct-packet message into caller-owned storage.
///
/// The complete output is `destination || source || signature || payload`.
/// A successful result always fits one direct Link DATA payload. The carrier is
/// the complete output, including its destination hash. No PRNS type, route
/// state, receipt, or proof participates in composition.
///
/// `fields` must be one canonical encoded MessagePack map. Pass `None` for the
/// canonical empty map. The output remains unchanged on every error.
#[allow(clippy::too_many_arguments)]
pub fn compose_basic_direct_lxmf<S: BasicLxmfSigner + ?Sized>(
    destination: [u8; DESTINATION_HASH_LENGTH],
    source: [u8; DESTINATION_HASH_LENGTH],
    timestamp_unix_ms: u64,
    title: &[u8],
    content: &[u8],
    fields: Option<&[u8]>,
    signer: &S,
    output: &mut [u8],
) -> Result<PreparedBasicLxmf, BasicLxmfComposeError> {
    if timestamp_unix_ms > MAX_LXMF_TIMESTAMP_UNIX_MS {
        return Err(BasicLxmfComposeError::TimestampTooLarge {
            actual: timestamp_unix_ms,
            maximum: MAX_LXMF_TIMESTAMP_UNIX_MS,
        });
    }
    let fields = fields.unwrap_or(&EMPTY_FIELDS);
    validate_fields(fields)?;
    let payload_len = encoded_basic_lxmf_payload_len(title.len(), content.len(), fields.len())
        .ok_or(BasicLxmfComposeError::MessageTooLarge)?;
    let wire_len = COMPLETE_WIRE_PREFIX_BYTES
        .checked_add(payload_len)
        .ok_or(BasicLxmfComposeError::MessageTooLarge)?;
    let selection_content_bytes = payload_len.saturating_sub(16);
    if selection_content_bytes > LINK_PACKET_MAX_CONTENT || wire_len > MAX_BASIC_LXMF_WIRE_BYTES {
        return Err(BasicLxmfComposeError::NeedsResource {
            wire_bytes: wire_len,
        });
    }

    let mut scratch = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let mut payload_cursor = COMPLETE_WIRE_PREFIX_BYTES;
    push_byte(&mut scratch, &mut payload_cursor, PAYLOAD_ARRAY_4)?;
    push_byte(&mut scratch, &mut payload_cursor, MSGPACK_F64)?;
    push_bytes(
        &mut scratch,
        &mut payload_cursor,
        &(timestamp_unix_ms as f64 / 1_000.0).to_bits().to_be_bytes(),
    )?;
    push_binary(&mut scratch, &mut payload_cursor, title)?;
    push_binary(&mut scratch, &mut payload_cursor, content)?;
    push_bytes(&mut scratch, &mut payload_cursor, fields)?;

    debug_assert_eq!(payload_cursor, wire_len);
    if output.len() < wire_len {
        return Err(BasicLxmfComposeError::OutputTooSmall {
            required: wire_len,
            available: output.len(),
        });
    }

    scratch[..DESTINATION_HASH_LENGTH].copy_from_slice(&destination);
    scratch[DESTINATION_HASH_LENGTH..DESTINATION_HASH_LENGTH * 2].copy_from_slice(&source);
    let payload = &scratch[COMPLETE_WIRE_PREFIX_BYTES..wire_len];
    let mut message_hasher = Sha256::new();
    message_hasher.update(destination);
    message_hasher.update(source);
    message_hasher.update(payload);
    let message_id: [u8; 32] = message_hasher.finalize().into();

    let mut signature_input = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let mut signature_cursor = 0;
    push_bytes(&mut signature_input, &mut signature_cursor, &destination)?;
    push_bytes(&mut signature_input, &mut signature_cursor, &source)?;
    push_bytes(&mut signature_input, &mut signature_cursor, payload)?;
    push_bytes(&mut signature_input, &mut signature_cursor, &message_id)?;
    debug_assert_eq!(
        signature_cursor,
        32 + payload.len() + SIGNATURE_INPUT_SUFFIX_BYTES
    );
    let signature = signer.sign_lxmf(&signature_input[..signature_cursor]);
    scratch[DESTINATION_HASH_LENGTH * 2..COMPLETE_WIRE_PREFIX_BYTES].copy_from_slice(&signature);
    output[..wire_len].copy_from_slice(&scratch[..wire_len]);

    Ok(PreparedBasicLxmf {
        wire_len: u16::try_from(wire_len).map_err(|_| BasicLxmfComposeError::MessageTooLarge)?,
        message_id,
    })
}

fn validate_fields(fields: &[u8]) -> Result<(), BasicLxmfComposeError> {
    let limits = WireLimits::new(
        fields.len(),
        fields.len(),
        fields.len(),
        fields.len().saturating_add(1),
        fields.len().saturating_mul(16).max(32),
        16,
    );
    let value = validate_messagepack_value(fields, limits)
        .map_err(|_| BasicLxmfComposeError::InvalidFields)?;
    if value.kind() != MessagePackKind::Map || value.canonicality() != Canonicality::Canonical {
        return Err(BasicLxmfComposeError::InvalidFields);
    }
    Ok(())
}

fn push_binary(
    output: &mut [u8],
    cursor: &mut usize,
    value: &[u8],
) -> Result<(), BasicLxmfComposeError> {
    if value.len() <= u8::MAX as usize {
        push_byte(output, cursor, 0xc4)?;
        push_byte(output, cursor, value.len() as u8)?;
    } else if value.len() <= u16::MAX as usize {
        push_byte(output, cursor, 0xc5)?;
        push_bytes(output, cursor, &(value.len() as u16).to_be_bytes())?;
    } else if value.len() <= u32::MAX as usize {
        push_byte(output, cursor, 0xc6)?;
        push_bytes(output, cursor, &(value.len() as u32).to_be_bytes())?;
    } else {
        return Err(BasicLxmfComposeError::MessageTooLarge);
    }
    push_bytes(output, cursor, value)
}

fn push_byte(
    output: &mut [u8],
    cursor: &mut usize,
    value: u8,
) -> Result<(), BasicLxmfComposeError> {
    push_bytes(output, cursor, &[value])
}

fn push_bytes(
    output: &mut [u8],
    cursor: &mut usize,
    value: &[u8],
) -> Result<(), BasicLxmfComposeError> {
    let end = cursor
        .checked_add(value.len())
        .ok_or(BasicLxmfComposeError::MessageTooLarge)?;
    let destination = output
        .get_mut(*cursor..end)
        .ok_or(BasicLxmfComposeError::MessageTooLarge)?;
    destination.copy_from_slice(value);
    *cursor = end;
    Ok(())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    struct FixtureSigner(SigningKey);

    impl BasicLxmfSigner for FixtureSigner {
        fn sign_lxmf(&self, input: &[u8]) -> [u8; 64] {
            self.0.sign(input).to_bytes()
        }
    }

    fn decode_hex<const N: usize>(encoded: &str) -> [u8; N] {
        let bytes = hex::decode(encoded).expect("fixture is hexadecimal");
        bytes.try_into().expect("fixture has exact length")
    }

    #[test]
    fn basic_message_matches_pinned_python_lxmf_exactly() {
        let destination = decode_hex("021e68345db8a80c29d0c2f193baa5f4");
        let source = decode_hex("20f7e44b55b06cff39719106f2bd1fd2");
        let signer = FixtureSigner(SigningKey::from_bytes(&[0x06; 32]));
        let expected = hex::decode(
            "021e68345db8a80c29d0c2f193baa5f4\
             20f7e44b55b06cff39719106f2bd1fd2\
             cfeaf89e57248baad43791a115345482f6b54b6e90aa0d02b5d8eddad1dc6a6\
             a323ec74921c618ae95e69153e9645db6f223d5d387db37ae23f58ef1f0560700\
             94cb41d954fc40000000c4094772656574696e6773\
             c41648656c6c6f2066726f6d20507974686f6e204c584d4680",
        )
        .expect("Python vector is hexadecimal");
        let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = compose_basic_direct_lxmf(
            destination,
            source,
            1_700_000_000_000,
            b"Greetings",
            b"Hello from Python LXMF",
            None,
            &signer,
            &mut output,
        )
        .expect("basic message composes");

        assert_eq!(&output[..usize::from(prepared.wire_len())], expected);
        assert_eq!(
            prepared.message_id(),
            decode_hex("c00af1f9ba72e66d4b9a41fbe76a55d6bbb1c8dfb9271f0cf660ed101e174c96")
        );
    }

    #[test]
    fn direct_selector_accepts_319_and_reports_320_without_mutating_output() {
        let signer = FixtureSigner(SigningKey::from_bytes(&[0x06; 32]));
        let mut accepted = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = compose_basic_direct_lxmf(
            [0x11; 16],
            [0x22; 16],
            1_700_000_000_000,
            b"",
            &[0x42; 319],
            None,
            &signer,
            &mut accepted,
        )
        .expect("319-byte selector content uses Link DATA");
        assert_eq!(usize::from(prepared.wire_len()), 431);

        let mut output = [0xa5_u8; MAX_BASIC_LXMF_WIRE_BYTES];
        let before = output;
        let error = compose_basic_direct_lxmf(
            [0x11; 16],
            [0x22; 16],
            1_700_000_000_000,
            b"",
            &[0x42; 320],
            None,
            &signer,
            &mut output,
        )
        .unwrap_err();
        assert_eq!(
            error,
            BasicLxmfComposeError::NeedsResource { wire_bytes: 432 }
        );
        assert_eq!(output, before);
    }

    #[test]
    fn rejects_noncanonical_or_non_map_fields() {
        let signer = FixtureSigner(SigningKey::from_bytes(&[0x06; 32]));
        let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
        for fields in [&[0x90][..], &[0xde, 0x00, 0x00][..]] {
            assert_eq!(
                compose_basic_direct_lxmf(
                    [0x11; 16],
                    [0x22; 16],
                    1,
                    b"",
                    b"",
                    Some(fields),
                    &signer,
                    &mut output,
                ),
                Err(BasicLxmfComposeError::InvalidFields)
            );
        }
    }
}
