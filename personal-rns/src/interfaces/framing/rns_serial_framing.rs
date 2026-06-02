//! Canonical RNS serial byte framing.
//!
//! This module intentionally uses Reticulum's reference
//! `SerialInterface` framing rather than inventing a local transport
//! wrapper. That keeps USB serial, RS-232, and similar byte-stream
//! hosts wire-compatible with a stock RNS daemon.
//!
//! The reference implementation calls this HDLC framing, but the
//! behavior here is specifically HDLC-like octet-stuffed framing: frame
//! delimiters and escapes live at the byte level. It is not KISS
//! framing, and it is not bit-synchronous HDLC.
//!
//! - [`FLAG`] (`0x7E`) is the frame delimiter — one before each frame
//!   and one after.
//! - [`ESC`] (`0x7D`) is the escape byte. Any `FLAG` or `ESC` byte that
//!   would appear in the payload is replaced by `ESC` followed by the
//!   raw byte XOR-ed with [`ESC_MASK`] (`0x20`).
//!
//! That gives `FLAG <escaped bytes> FLAG` on the wire. Empty frames
//! (`FLAG FLAG` with nothing between them) are valid keepalives and the
//! streaming decoder yields them; callers that don't care filter them
//! out.
//!
//! Worst-case framed length is `2 + 2 * payload.len()` (every byte
//! escaped) and best case is `2 + payload.len()`.
//!
//! This module owns the octet-stuffed frame format only. Concrete
//! interfaces own read-loop policy such as idle timeout, frame cap, and
//! oversize recovery.
//!
//! Reference RNS serial framing:
//! <https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/SerialInterface.py#L38-L48>
//!
//! Reference escape handling:
//! <https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/SerialInterface.py#L180-L186>

use heapless::Vec;

/// RNS serial frame delimiter.
pub const FLAG: u8 = 0x7E;
pub const ESC: u8 = 0x7D;
/// XOR mask applied after [`ESC`] to recover the original byte (and
/// applied at encode time to produce the escaped byte). Chosen so the
/// escaped form of `FLAG` (`0x5E`) and `ESC` (`0x5D`) can never collide
/// with another `FLAG` or `ESC` in the byte stream.
pub const ESC_MASK: u8 = 0x20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    OutputTooSmall,
}

/// Largest possible framed output size for an input of `payload_len`
/// bytes: the two delimiters plus an escape per byte in the worst
/// case (every byte is `FLAG` or `ESC`).
pub const fn max_encoded_len(payload_len: usize) -> usize {
    2 + 2 * payload_len
}

/// Encode `input` as a single RNS serial frame into `output`. Returns the
/// number of bytes written, including the leading and trailing
/// delimiters.
pub fn encode(input: &[u8], output: &mut [u8]) -> Result<usize, EncodeError> {
    let mut written = 0usize;
    let mut put = |byte: u8, written: &mut usize| -> Result<(), EncodeError> {
        if *written >= output.len() {
            return Err(EncodeError::OutputTooSmall);
        }
        output[*written] = byte;
        *written += 1;
        Ok(())
    };

    put(FLAG, &mut written)?;
    for &byte in input {
        match byte {
            FLAG => {
                put(ESC, &mut written)?;
                put(FLAG ^ ESC_MASK, &mut written)?;
            }
            ESC => {
                put(ESC, &mut written)?;
                put(ESC ^ ESC_MASK, &mut written)?;
            }
            other => put(other, &mut written)?,
        }
    }
    put(FLAG, &mut written)?;
    Ok(written)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    FrameTooBig,
}

/// Streaming RNS serial framing decoder. Feed bytes one at a time as
/// they arrive from the byte transport; on each [`FLAG`] that closes a
/// frame, [`feed`] returns the decoded payload, borrowing from the
/// decoder's internal buffer.
///
/// `FRAME_CAP` is the largest in-progress frame the decoder will
/// accept. Serial-style RNS interfaces should size this at least at
/// the engine MTU; sizing larger costs only stack bytes.
///
/// Bytes received before the first `FLAG` are silently ignored — that
/// matches RNS's reference behavior so the decoder can be plugged into
/// an already-running byte stream without manual resync.
///
/// [`feed`]: RnsSerialDecoder::feed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsSerialDecoder<const FRAME_CAP: usize> {
    buffer: Vec<u8, FRAME_CAP>,
    in_frame: bool,
    saw_escape: bool,
}

impl<const FRAME_CAP: usize> Default for RnsSerialDecoder<FRAME_CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const FRAME_CAP: usize> RnsSerialDecoder<FRAME_CAP> {
    pub const fn new() -> Self {
        Self {
            buffer: Vec::new(),
            in_frame: false,
            saw_escape: false,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.in_frame = false;
        self.saw_escape = false;
    }

    /// Feed one byte into the decoder.
    ///
    /// - `Ok(None)` — keep feeding; no frame boundary yet.
    /// - `Ok(Some(frame))` — a frame just closed; the slice borrows
    ///   from `self` and stays valid until the next call to
    ///   [`feed`](Self::feed) or [`reset`](Self::reset). Empty frames
    ///   (`FLAG FLAG`) are valid keepalives and surface here as
    ///   `Some(&[])`.
    /// - `Err(FrameTooBig)` — the in-progress frame exceeded
    ///   `FRAME_CAP`; the decoder auto-resets and the next `FLAG`
    ///   starts a fresh frame.
    pub fn feed(&mut self, byte: u8) -> Result<Option<&[u8]>, DecodeError> {
        if byte == FLAG {
            if self.in_frame {
                self.in_frame = false;
                self.saw_escape = false;
                return Ok(Some(&self.buffer));
            }
            self.buffer.clear();
            self.in_frame = true;
            self.saw_escape = false;
            return Ok(None);
        }

        if !self.in_frame {
            // Pre-frame noise (anything before the first FLAG). RNS
            // ignores these; we do the same so a decoder can plug into
            // an already-running stream.
            return Ok(None);
        }

        if byte == ESC {
            self.saw_escape = true;
            return Ok(None);
        }

        let payload_byte = if self.saw_escape {
            self.saw_escape = false;
            match byte {
                escaped_flag if escaped_flag == (FLAG ^ ESC_MASK) => FLAG,
                escaped_esc if escaped_esc == (ESC ^ ESC_MASK) => ESC,
                other => other,
            }
        } else {
            byte
        };

        if self.buffer.push(payload_byte).is_err() {
            self.reset();
            return Err(DecodeError::FrameTooBig);
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Default decoder cap for the tests — comfortably larger than any
    /// Reticulum packet so the FrameTooBig branch is exercised on a
    /// dedicated tighter-cap decoder rather than accidentally here.
    const TEST_FRAME_CAP: usize = 1024;

    fn decode_all(bytes: &[u8]) -> std::vec::Vec<std::vec::Vec<u8>> {
        let mut decoder: RnsSerialDecoder<TEST_FRAME_CAP> = RnsSerialDecoder::new();
        let mut frames = std::vec::Vec::new();
        for &b in bytes {
            if let Some(frame) = decoder.feed(b).unwrap() {
                frames.push(frame.to_vec());
            }
        }
        frames
    }

    #[test]
    fn empty_payload_encodes_to_flag_flag() {
        let mut out = [0u8; 4];
        let n = encode(&[], &mut out).unwrap();
        assert_eq!(&out[..n], &[FLAG, FLAG]);
    }

    #[test]
    fn non_special_bytes_pass_through_unescaped() {
        let payload = [0x01, 0x02, 0x03, 0x55];
        let mut out = [0u8; 32];
        let n = encode(&payload, &mut out).unwrap();
        assert_eq!(&out[..n], &[FLAG, 0x01, 0x02, 0x03, 0x55, FLAG]);
    }

    #[test]
    fn flag_byte_in_payload_is_escaped() {
        let payload = [0x01, FLAG, 0x02];
        let mut out = [0u8; 32];
        let n = encode(&payload, &mut out).unwrap();
        assert_eq!(&out[..n], &[FLAG, 0x01, ESC, FLAG ^ ESC_MASK, 0x02, FLAG],);
    }

    #[test]
    fn esc_byte_in_payload_is_escaped() {
        let payload = [ESC];
        let mut out = [0u8; 32];
        let n = encode(&payload, &mut out).unwrap();
        assert_eq!(&out[..n], &[FLAG, ESC, ESC ^ ESC_MASK, FLAG]);
    }

    #[test]
    fn encode_to_undersized_buffer_returns_output_too_small() {
        let payload = [0x01, 0x02, 0x03];
        let mut tiny = [0u8; 3];
        assert_eq!(
            encode(&payload, &mut tiny),
            Err(EncodeError::OutputTooSmall)
        );
    }

    #[test]
    fn max_encoded_len_bounds_the_worst_case() {
        let payload = [FLAG; 10];
        let mut out = [0u8; max_encoded_len(10)];
        let n = encode(&payload, &mut out).unwrap();
        assert_eq!(n, max_encoded_len(10));
    }

    #[test]
    fn decoder_yields_payload_when_the_closing_flag_arrives() {
        let bytes = [FLAG, 0x01, 0x02, 0x03, FLAG];
        let frames = decode_all(&bytes);
        assert_eq!(frames, std::vec![std::vec![0x01, 0x02, 0x03]]);
    }

    #[test]
    fn decoder_yields_empty_frames_as_keepalives() {
        // FLAG FLAG → empty frame. Two empty frames back to back: FLAG
        // FLAG FLAG → open, close (empty), open. The third FLAG isn't
        // a frame yet, but the middle one yields Some(&[]).
        let bytes = [FLAG, FLAG, FLAG];
        let frames = decode_all(&bytes);
        assert_eq!(frames, std::vec![std::vec::Vec::<u8>::new()]);
    }

    #[test]
    fn decoder_unescapes_flag_and_esc_back_to_their_raw_forms() {
        let bytes = [FLAG, ESC, FLAG ^ ESC_MASK, ESC, ESC ^ ESC_MASK, 0x55, FLAG];
        let frames = decode_all(&bytes);
        assert_eq!(frames, std::vec![std::vec![FLAG, ESC, 0x55]]);
    }

    #[test]
    fn decoder_preserves_noncanonical_escaped_bytes_like_rns() {
        let noncanonical_escaped_byte = 0x61;
        let bytes = [FLAG, ESC, noncanonical_escaped_byte, FLAG];
        let frames = decode_all(&bytes);
        assert_eq!(frames, std::vec![std::vec![noncanonical_escaped_byte]]);
    }

    #[test]
    fn decoder_ignores_bytes_before_the_first_flag() {
        let bytes = [0xAA, 0xBB, FLAG, 0x01, FLAG];
        let frames = decode_all(&bytes);
        assert_eq!(frames, std::vec![std::vec![0x01]]);
    }

    #[test]
    fn decoder_yields_two_back_to_back_frames_with_the_rns_double_flag_layout() {
        // RNS wraps each frame as `FLAG <data> FLAG`, so two frames
        // back to back put TWO FLAGs between them. The closing FLAG of
        // frame 1 exits the in-frame state; the opening FLAG of frame
        // 2 re-enters it.
        let bytes = [FLAG, 0x01, FLAG, FLAG, 0x02, FLAG];
        let frames = decode_all(&bytes);
        assert_eq!(frames, std::vec![std::vec![0x01], std::vec![0x02]]);
    }

    #[test]
    fn frame_exceeding_cap_returns_frame_too_big_and_auto_resets() {
        let mut decoder: RnsSerialDecoder<2> = RnsSerialDecoder::new();
        // Open a frame and push past the cap.
        assert_eq!(decoder.feed(FLAG).unwrap(), None);
        assert_eq!(decoder.feed(0x01).unwrap(), None);
        assert_eq!(decoder.feed(0x02).unwrap(), None);
        assert_eq!(decoder.feed(0x03), Err(DecodeError::FrameTooBig));

        // Auto-reset: the next FLAG opens a fresh frame.
        assert_eq!(decoder.feed(FLAG).unwrap(), None);
        assert_eq!(decoder.feed(0xAB).unwrap(), None);
        let frame = decoder.feed(FLAG).unwrap().unwrap();
        assert_eq!(frame, &[0xAB]);
    }

    /// A genuine RNS 1.3.1 announce — the same vector the engine
    /// module uses. Round-tripping it through encode → decode proves
    /// the framing layer is byte-transparent for the real workload.
    const RAW_ANNOUNCE_HEX: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                    59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                    0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                    7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                    4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    #[test]
    fn a_real_rns_announce_round_trips_through_encode_then_decode() {
        let raw = hx(RAW_ANNOUNCE_HEX);

        let mut framed = std::vec![0u8; max_encoded_len(raw.len())];
        let n = encode(&raw, &mut framed).unwrap();
        let framed = &framed[..n];

        // Sanity: opens + closes with FLAG.
        assert_eq!(framed[0], FLAG);
        assert_eq!(framed[framed.len() - 1], FLAG);

        // And the decoder reconstructs the original byte-for-byte.
        let frames = decode_all(framed);
        assert_eq!(frames, std::vec![raw]);
    }

    proptest! {
        #[test]
        fn arbitrary_payloads_round_trip_through_encode_then_decode(
            payload in prop::collection::vec(any::<u8>(), 0..256),
        ) {
            let mut framed = std::vec![0u8; max_encoded_len(payload.len())];
            let n = encode(&payload, &mut framed).unwrap();
            let frames = decode_all(&framed[..n]);
            prop_assert_eq!(frames, std::vec![payload]);
        }

        #[test]
        fn streaming_decoder_handles_arbitrary_chunk_boundaries(
            payload in prop::collection::vec(any::<u8>(), 0..256),
            chunk_size in 1usize..16,
        ) {
            // Encode once, then feed the resulting byte stream in
            // chunks of `chunk_size` to mirror how a real serial read
            // would deliver bytes — frame boundaries do not align with
            // read calls.
            let mut framed = std::vec![0u8; max_encoded_len(payload.len())];
            let n = encode(&payload, &mut framed).unwrap();
            let framed = &framed[..n];

            let mut decoder: RnsSerialDecoder<TEST_FRAME_CAP> = RnsSerialDecoder::new();
            let mut frames = std::vec::Vec::new();
            for chunk in framed.chunks(chunk_size) {
                for &b in chunk {
                    if let Some(frame) = decoder.feed(b).unwrap() {
                        frames.push(frame.to_vec());
                    }
                }
            }
            prop_assert_eq!(frames, std::vec![payload]);
        }
    }
}
