//! RNS 1.3.1 `Channel`: reliable, in-order, deduplicated message delivery over
//! a link (the [`LinkId`](super::LinkId) session). This module is the wire
//! foundation — the envelope header the engine writes ahead of every message
//! body, riding as the plaintext of a
//! [`WireContext::Channel`](crate::wire::WireContext::Channel) link data packet.
//!
//! The engine treats the body as opaque: a [`MessageType`] tag and raw bytes
//! the app interprets. Typed-message multiplexing (an app handing us an enum)
//! is a higher consumer-API layer, not the engine's concern. `Buffer` rides on
//! top as one consumer that claims the reserved system type `0xff00`.

pub mod columns;
pub mod impls;
pub mod receive;
pub mod send;

use crate::routing::links::data::link_mdu;

/// RNS 1.3.1 `Channel` `MSGTYPE`: the 16-bit tag that lets one channel
/// multiplex several message kinds over a link. Opaque to the engine; values
/// `>= 0xf000` are reserved for system types (the `Buffer` stream is `0xff00`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct MessageType(pub u16);

/// RNS 1.3.1 `Channel` sequence number: 16-bit, counting modulo
/// [`SEQ_MODULUS`], the ordering key for reliable in-order delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct ChannelSequence(pub u16);

/// RNS 1.3.1 `Channel.SEQ_MODULUS` (`SEQ_MAX + 1`): sequence numbers count
/// modulo this, wrapping from `0xFFFF` back to `0`.
pub const SEQ_MODULUS: u32 = 0x1_0000;

impl ChannelSequence {
    /// The next sequence in line, wrapping `0xFFFF -> 0`.
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// RNS 1.3.1 `Channel.Envelope` header: the 6 bytes —
/// `struct.pack(">HHH", msgtype, sequence, length)` — ahead of the message
/// body.
pub const ENVELOPE_HEADER_LEN: usize = 6;

/// RNS 1.3.1 `Channel.mdu`: the most one message body can carry — the link MDU
/// (see [`link_mdu`]) less the envelope header, capped at the length field's
/// `u16::MAX` ceiling.
pub const fn channel_mdu(mtu: usize) -> usize {
    let body = link_mdu(mtu).saturating_sub(ENVELOPE_HEADER_LEN);
    if body > u16::MAX as usize {
        u16::MAX as usize
    } else {
        body
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeError {
    PayloadTooLong,
    BufferTooShort,
    TruncatedHeader,
    LengthMismatch,
}

/// Frame `payload` behind its envelope header into `buf`, returning the written
/// length. The body rides verbatim and the length field is set to the body
/// length, matching RNS `Channel.Envelope.pack`.
pub fn write_envelope(
    message_type: MessageType,
    sequence: ChannelSequence,
    payload: &[u8],
    buf: &mut [u8],
) -> Result<usize, EnvelopeError> {
    if payload.len() > u16::MAX as usize {
        return Err(EnvelopeError::PayloadTooLong);
    }
    let end = ENVELOPE_HEADER_LEN + payload.len();
    if buf.len() < end {
        return Err(EnvelopeError::BufferTooShort);
    }
    buf[0..2].copy_from_slice(&message_type.0.to_be_bytes());
    buf[2..4].copy_from_slice(&sequence.0.to_be_bytes());
    buf[4..6].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    buf[ENVELOPE_HEADER_LEN..end].copy_from_slice(payload);
    Ok(end)
}

/// A parsed envelope: the [`MessageType`] tag, the [`ChannelSequence`], and the
/// borrowed message body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Envelope<'a> {
    pub message_type: MessageType,
    pub sequence: ChannelSequence,
    pub payload: &'a [u8],
}

/// Parse an envelope from the opened plaintext of a channel link packet. The
/// length field must equal the actual body length — RNS senders always agree,
/// so a disagreement is a malformed frame.
pub fn parse_envelope(bytes: &[u8]) -> Result<Envelope<'_>, EnvelopeError> {
    if bytes.len() < ENVELOPE_HEADER_LEN {
        return Err(EnvelopeError::TruncatedHeader);
    }
    let message_type = MessageType(u16::from_be_bytes([bytes[0], bytes[1]]));
    let sequence = ChannelSequence(u16::from_be_bytes([bytes[2], bytes[3]]));
    let length = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    let payload = &bytes[ENVELOPE_HEADER_LEN..];
    if payload.len() != length {
        return Err(EnvelopeError::LengthMismatch);
    }
    Ok(Envelope {
        message_type,
        sequence,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::BROADCAST_MTU;

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    #[test]
    fn write_envelope_matches_the_reference_pack() {
        let mut buf = [0u8; 64];
        let n = write_envelope(
            MessageType(0x0007),
            ChannelSequence(0x0003),
            b"hello channel",
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], &hx("00070003000d68656c6c6f206368616e6e656c")[..]);
    }

    #[test]
    fn the_system_stream_type_near_the_wrap_round_trips() {
        let mut buf = [0u8; 32];
        let n = write_envelope(
            MessageType(0xff00),
            ChannelSequence(0xfffe),
            &[0, 1, 2, 3],
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], &hx("ff00fffe000400010203")[..]);

        let envelope = parse_envelope(&buf[..n]).unwrap();
        assert_eq!(envelope.message_type, MessageType(0xff00));
        assert_eq!(envelope.sequence, ChannelSequence(0xfffe));
        assert_eq!(envelope.payload, &[0, 1, 2, 3]);
    }

    #[test]
    fn an_empty_body_packs_a_zero_length_field() {
        let mut buf = [0u8; 16];
        let n =
            write_envelope(MessageType(0x0001), ChannelSequence(0x0000), b"", &mut buf).unwrap();
        assert_eq!(&buf[..n], &hx("000100000000")[..]);
        assert_eq!(parse_envelope(&buf[..n]).unwrap().payload, b"");
    }

    #[test]
    fn parse_rejects_a_truncated_header() {
        assert_eq!(
            parse_envelope(&[0x00, 0x07, 0x00]),
            Err(EnvelopeError::TruncatedHeader),
        );
    }

    #[test]
    fn parse_rejects_a_length_field_that_disagrees_with_the_body() {
        // Header claims a 5-byte body, but only two bytes follow.
        assert_eq!(
            parse_envelope(&hx("0007000300056865")),
            Err(EnvelopeError::LengthMismatch),
        );
    }

    #[test]
    fn write_rejects_a_body_that_overflows_the_buffer() {
        let mut buf = [0u8; 8];
        assert_eq!(
            write_envelope(
                MessageType(0x0001),
                ChannelSequence(0x0000),
                b"toolong",
                &mut buf
            ),
            Err(EnvelopeError::BufferTooShort),
        );
    }

    #[test]
    fn the_sequence_wraps_past_the_modulus() {
        assert_eq!(ChannelSequence(0xFFFF).next(), ChannelSequence(0x0000));
        assert_eq!(SEQ_MODULUS, 0x1_0000);
    }

    #[test]
    fn the_channel_mdu_is_the_link_mdu_less_the_header_capped_at_u16() {
        assert_eq!(channel_mdu(BROADCAST_MTU), 425);
        assert_eq!(channel_mdu(1_000_000), u16::MAX as usize);
    }
}
