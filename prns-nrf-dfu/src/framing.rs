use alloc::vec::Vec;

use thiserror::Error;

use crate::firmware_crc;

const HEADER_BYTES: usize = 4;
const CRC_BYTES: usize = 2;
const MAX_PAYLOAD_BYTES: usize = 0x0fff;
const PACKET_TYPE_ACKNOWLEDGEMENT: u8 = 0;
const PACKET_TYPE_VENDOR_SPECIFIC: u8 = 14;
const DATA_INTEGRITY_PRESENT: u8 = 1 << 6;
const RELIABLE_PACKET: u8 = 1 << 7;
const SLIP_END: u8 = 0xc0;
const SLIP_ESCAPE: u8 = 0xdb;
const SLIP_ESCAPED_END: u8 = 0xdc;
const SLIP_ESCAPED_ESCAPE: u8 = 0xdd;
const SEQUENCE_MODULUS: u8 = 8;
const INITIAL_SEQUENCE_NUMBER: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Acknowledgement(u8);

impl Acknowledgement {
    pub const fn number(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameEncodeError {
    #[error("DFU frame payload is {actual} bytes; the maximum is {maximum}")]
    PayloadTooLong { actual: usize, maximum: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedFrame {
    sequence_number: u8,
    expected_acknowledgement: Acknowledgement,
    bytes: Vec<u8>,
}

impl EncodedFrame {
    pub const fn sequence_number(&self) -> u8 {
        self.sequence_number
    }

    pub const fn expected_acknowledgement(&self) -> Acknowledgement {
        self.expected_acknowledgement
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug)]
pub struct ReliableFrameEncoder {
    next_sequence_number: u8,
}

impl ReliableFrameEncoder {
    pub const fn new() -> Self {
        Self {
            next_sequence_number: INITIAL_SEQUENCE_NUMBER,
        }
    }

    pub fn encode(&mut self, payload: &[u8]) -> Result<EncodedFrame, FrameEncodeError> {
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(FrameEncodeError::PayloadTooLong {
                actual: payload.len(),
                maximum: MAX_PAYLOAD_BYTES,
            });
        }

        let sequence_number = self.next_sequence_number;
        let expected_acknowledgement = (sequence_number + 1) % SEQUENCE_MODULUS;
        self.next_sequence_number = expected_acknowledgement;

        let payload_length = payload.len() as u16;
        let mut unescaped = Vec::with_capacity(HEADER_BYTES + payload.len() + CRC_BYTES);
        unescaped.push(
            sequence_number
                | (expected_acknowledgement << 3)
                | DATA_INTEGRITY_PRESENT
                | RELIABLE_PACKET,
        );
        unescaped.push(PACKET_TYPE_VENDOR_SPECIFIC | ((payload_length as u8 & 0x0f) << 4));
        unescaped.push((payload_length >> 4) as u8);
        let header_checksum = 0_u8.wrapping_sub(
            unescaped[0]
                .wrapping_add(unescaped[1])
                .wrapping_add(unescaped[2]),
        );
        unescaped.push(header_checksum);
        unescaped.extend_from_slice(payload);
        unescaped.extend_from_slice(&firmware_crc(&unescaped).get().to_le_bytes());

        let mut bytes = Vec::with_capacity(unescaped.len() + 2);
        bytes.push(SLIP_END);
        for byte in unescaped {
            match byte {
                SLIP_END => bytes.extend_from_slice(&[SLIP_ESCAPE, SLIP_ESCAPED_END]),
                SLIP_ESCAPE => bytes.extend_from_slice(&[SLIP_ESCAPE, SLIP_ESCAPED_ESCAPE]),
                byte => bytes.push(byte),
            }
        }
        bytes.push(SLIP_END);

        Ok(EncodedFrame {
            sequence_number,
            expected_acknowledgement: Acknowledgement(expected_acknowledgement),
            bytes,
        })
    }
}

impl Default for ReliableFrameEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AcknowledgementError {
    #[error("SLIP escape byte was followed by invalid value 0x{0:02x}")]
    InvalidEscape(u8),
    #[error("SLIP acknowledgement ends with an incomplete escape sequence")]
    IncompleteEscape,
    #[error("acknowledgement frame has {actual} bytes; expected {expected}")]
    InvalidLength { actual: usize, expected: usize },
    #[error("acknowledgement header checksum is invalid")]
    HeaderChecksum,
    #[error("received packet type {0} instead of an acknowledgement")]
    PacketType(u8),
    #[error("acknowledgement frame declares unsupported flags 0x{0:02x}")]
    UnsupportedFlags(u8),
    #[error("acknowledgement frame declares a non-empty payload")]
    NonEmptyPayload,
}

#[derive(Debug)]
pub struct AcknowledgementDecoder {
    frame_started: bool,
    escape_pending: bool,
    decoded: Vec<u8>,
}

impl AcknowledgementDecoder {
    pub const fn new() -> Self {
        Self {
            frame_started: false,
            escape_pending: false,
            decoded: Vec::new(),
        }
    }

    pub fn push(&mut self, byte: u8) -> Result<Option<Acknowledgement>, AcknowledgementError> {
        if byte == SLIP_END {
            if self.escape_pending {
                self.escape_pending = false;
                self.frame_started = false;
                self.decoded.clear();
                return Err(AcknowledgementError::IncompleteEscape);
            }
            if !self.frame_started {
                self.frame_started = true;
                self.decoded.clear();
                return Ok(None);
            }
            if self.decoded.is_empty() {
                return Ok(None);
            }

            let result = decode_acknowledgement(&self.decoded);
            self.decoded.clear();
            return result.map(Some);
        }
        if !self.frame_started {
            return Ok(None);
        }
        if self.escape_pending {
            self.escape_pending = false;
            if self.decoded.len() == HEADER_BYTES {
                self.decoded.clear();
                self.frame_started = false;
                return Err(AcknowledgementError::InvalidLength {
                    actual: HEADER_BYTES + 1,
                    expected: HEADER_BYTES,
                });
            }
            match byte {
                SLIP_ESCAPED_END => self.decoded.push(SLIP_END),
                SLIP_ESCAPED_ESCAPE => self.decoded.push(SLIP_ESCAPE),
                byte => {
                    self.decoded.clear();
                    self.frame_started = false;
                    return Err(AcknowledgementError::InvalidEscape(byte));
                }
            }
            return Ok(None);
        }
        if byte == SLIP_ESCAPE {
            self.escape_pending = true;
        } else {
            if self.decoded.len() == HEADER_BYTES {
                self.decoded.clear();
                self.frame_started = false;
                return Err(AcknowledgementError::InvalidLength {
                    actual: HEADER_BYTES + 1,
                    expected: HEADER_BYTES,
                });
            }
            self.decoded.push(byte);
        }
        Ok(None)
    }
}

impl Default for AcknowledgementDecoder {
    fn default() -> Self {
        Self::new()
    }
}

fn decode_acknowledgement(bytes: &[u8]) -> Result<Acknowledgement, AcknowledgementError> {
    if bytes.len() != HEADER_BYTES {
        return Err(AcknowledgementError::InvalidLength {
            actual: bytes.len(),
            expected: HEADER_BYTES,
        });
    }
    if bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte)) != 0 {
        return Err(AcknowledgementError::HeaderChecksum);
    }
    let packet_type = bytes[1] & 0x0f;
    if packet_type != PACKET_TYPE_ACKNOWLEDGEMENT {
        return Err(AcknowledgementError::PacketType(packet_type));
    }
    let flags = bytes[0] & (DATA_INTEGRITY_PRESENT | RELIABLE_PACKET);
    if flags != 0 {
        return Err(AcknowledgementError::UnsupportedFlags(flags));
    }
    if bytes[1] >> 4 != 0 || bytes[2] != 0 {
        return Err(AcknowledgementError::NonEmptyPayload);
    }
    Ok(Acknowledgement((bytes[0] >> 3) & 0x07))
}

#[cfg(test)]
mod tests {
    use super::{
        Acknowledgement, AcknowledgementDecoder, AcknowledgementError, FrameEncodeError,
        ReliableFrameEncoder,
    };

    #[test]
    fn reliable_frame_matches_adafruit_nrfutil_reference() -> Result<(), FrameEncodeError> {
        let mut encoder = ReliableFrameEncoder::new();
        let frame = encoder.encode(&[3, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0])?;
        assert_eq!(
            frame.bytes(),
            &[
                0xc0, 0xd1, 0x0e, 0x01, 0x20, 0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0xab, 0xe8, 0xc0,
            ]
        );
        assert_eq!(frame.sequence_number(), 1);
        assert_eq!(frame.expected_acknowledgement(), Acknowledgement(2));
        Ok(())
    }

    #[test]
    fn reliable_frame_escapes_slip_control_bytes() -> Result<(), FrameEncodeError> {
        let mut encoder = ReliableFrameEncoder::new();
        let frame = encoder.encode(&[0xc0, 0xdb])?;
        assert_eq!(
            frame.bytes(),
            &[0xc0, 0xd1, 0x2e, 0x00, 0x01, 0xdb, 0xdc, 0xdb, 0xdd, 0x6a, 0x73, 0xc0]
        );
        Ok(())
    }

    #[test]
    fn acknowledgement_decoder_accepts_fragmented_reference_frame(
    ) -> Result<(), AcknowledgementError> {
        let mut decoder = AcknowledgementDecoder::new();
        let mut decoded = None;
        for byte in [0xc0, 0x10, 0x00, 0x00, 0xf0, 0xc0] {
            let current = decoder.push(byte)?;
            if current.is_some() {
                decoded = current;
            }
        }
        assert_eq!(decoded, Some(Acknowledgement(2)));
        Ok(())
    }

    #[test]
    fn acknowledgement_decoder_rejects_corrupt_headers() {
        let mut decoder = AcknowledgementDecoder::new();
        for byte in [0xc0, 0x10, 0x00, 0x00, 0x00] {
            assert!(decoder.push(byte).is_ok());
        }
        assert_eq!(
            decoder.push(0xc0),
            Err(AcknowledgementError::HeaderChecksum)
        );
    }

    #[test]
    fn acknowledgement_decoder_rejects_incomplete_escapes() {
        let mut decoder = AcknowledgementDecoder::new();
        for byte in [0xc0, 0x10, 0x00, 0x00, 0xf0, 0xdb] {
            assert!(decoder.push(byte).is_ok());
        }
        assert_eq!(
            decoder.push(0xc0),
            Err(AcknowledgementError::IncompleteEscape)
        );
    }

    #[test]
    fn acknowledgement_decoder_bounds_unterminated_frames() {
        let mut decoder = AcknowledgementDecoder::new();
        for byte in [0xc0, 0x10, 0x00, 0x00, 0xf0] {
            assert!(decoder.push(byte).is_ok());
        }
        assert_eq!(
            decoder.push(0x00),
            Err(AcknowledgementError::InvalidLength {
                actual: 5,
                expected: 4,
            })
        );
    }

    #[test]
    fn acknowledgement_decoder_bounds_escaped_unterminated_frames() {
        let mut decoder = AcknowledgementDecoder::new();
        for byte in [0xc0, 0x10, 0x00, 0x00, 0xf0, 0xdb] {
            assert!(decoder.push(byte).is_ok());
        }
        assert_eq!(
            decoder.push(0xdc),
            Err(AcknowledgementError::InvalidLength {
                actual: 5,
                expected: 4,
            })
        );
    }
}
