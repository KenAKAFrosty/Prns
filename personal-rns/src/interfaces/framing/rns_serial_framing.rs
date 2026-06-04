//! The reference implementation calls this HDLC framing, but the
//! behavior here is specifically HDLC-like octet-stuffed framing: frame
//! delimiters and escapes live at the byte level. It is not KISS
//! framing, and it is not bit-synchronous HDLC.
//!
//! Reference RNS serial framing:
//! <https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/SerialInterface.py#L38-L48>
//!
//! Reference escape handling:
//! <https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/SerialInterface.py#L180-L186>

use heapless::Vec;

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

pub const fn max_encoded_len(payload_len: usize) -> usize {
    2 + 2 * payload_len
}

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

/// The decoder can be plugged into an already-running byte stream: bytes
/// that arrive with no frame open are taken as the body of a frame whose
/// opening `FLAG` was missed, so they close at the next `FLAG` as one
/// (typically undecodable, discarded) frame and the decoder realigns from
/// there. This self-heal matters because RNS's `FLAG data FLAG FLAG data
/// FLAG` layout would otherwise let a mid-frame join lock the decoder a
/// half-frame out of phase permanently.
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
            // A payload byte with no frame open means we joined the stream
            // mid-frame: a reconnect landing in another frame's body, or a
            // half-frame an interrupted write left in the FIFO. Dropping these —
            // the obvious thing — is a trap against RNS's periodic
            // `FLAG data FLAG FLAG data FLAG` layout: it can lock the decoder a
            // half-frame out of phase *permanently*, with every real payload then
            // falling into the dropped gap. Instead, open a frame implicitly and
            // accumulate. Those bytes close at the next FLAG as one frame that
            // fails to decode and is discarded, and from there we are realigned.
            self.buffer.clear();
            self.in_frame = true;
            self.saw_escape = false;
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
    fn a_mid_frame_join_surfaces_one_discardable_frame_then_realigns() {
        let bytes = [0xAA, 0xBB, FLAG, 0x01, FLAG];
        let frames = decode_all(&bytes);
        assert_eq!(frames, std::vec![std::vec![0xAA, 0xBB], std::vec![0x01]]);
    }

    #[test]
    fn a_half_frame_does_not_permanently_desync_the_following_frames() {
        let announce = hx(RAW_ANNOUNCE_HEX);
        let mut clean = std::vec![0u8; max_encoded_len(announce.len())];
        let n = encode(&announce, &mut clean).unwrap();

        let mut stream = std::vec![FLAG, 0x03, 0xAA, 0xBB];
        stream.extend_from_slice(&clean[..n]);

        let frames = decode_all(&stream);
        assert!(
            frames.contains(&announce),
            "decoder failed to realign onto the clean frame after a half-frame"
        );
    }

    #[test]
    fn decoder_yields_two_back_to_back_frames_with_the_rns_double_flag_layout() {
        let bytes = [FLAG, 0x01, FLAG, FLAG, 0x02, FLAG];
        let frames = decode_all(&bytes);
        assert_eq!(frames, std::vec![std::vec![0x01], std::vec![0x02]]);
    }

    #[test]
    fn frame_exceeding_cap_returns_frame_too_big_and_auto_resets() {
        let mut decoder: RnsSerialDecoder<2> = RnsSerialDecoder::new();
        assert_eq!(decoder.feed(FLAG).unwrap(), None);
        assert_eq!(decoder.feed(0x01).unwrap(), None);
        assert_eq!(decoder.feed(0x02).unwrap(), None);
        assert_eq!(decoder.feed(0x03), Err(DecodeError::FrameTooBig));

        assert_eq!(decoder.feed(FLAG).unwrap(), None);
        assert_eq!(decoder.feed(0xAB).unwrap(), None);
        let frame = decoder.feed(FLAG).unwrap().unwrap();
        assert_eq!(frame, &[0xAB]);
    }

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

        assert_eq!(framed[0], FLAG);
        assert_eq!(framed[framed.len() - 1], FLAG);

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
