use super::{FrameBuffer, FrameSink};

pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;

pub const CMD_DATA: u8 = 0x00;
pub const CMD_TXDELAY: u8 = 0x01;
pub const CMD_P: u8 = 0x02;
pub const CMD_SLOTTIME: u8 = 0x03;
pub const CMD_TXTAIL: u8 = 0x04;
pub const CMD_FULLDUPLEX: u8 = 0x05;
pub const CMD_SETHARDWARE: u8 = 0x06;
/// RNS writes `CMD_READY 0x01` at startup and the TNC echoes it to pace transmission.
pub const CMD_READY: u8 = 0x0F;

/// `0xFE` can never be produced by `byte & 0x0F`, so it never collides with a real command.
const CMD_UNKNOWN: u8 = 0xFE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    OutputTooSmall,
}

pub const fn max_encoded_len(payload_len: usize) -> usize {
    3 + 2 * payload_len
}

pub fn encode(input: &[u8], output: &mut [u8]) -> Result<usize, EncodeError> {
    encode_with_command(CMD_DATA, input, output)
}

pub fn encode_with_command(
    command: u8,
    input: &[u8],
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    if output.len() < 2 {
        return Err(EncodeError::OutputTooSmall);
    }
    output[0] = FEND;
    output[1] = command;
    let mut written = 2;

    for &byte in input {
        match byte {
            FEND => {
                if written + 2 > output.len() {
                    return Err(EncodeError::OutputTooSmall);
                }
                output[written] = FESC;
                output[written + 1] = TFEND;
                written += 2;
            }
            FESC => {
                if written + 2 > output.len() {
                    return Err(EncodeError::OutputTooSmall);
                }
                output[written] = FESC;
                output[written + 1] = TFESC;
                written += 2;
            }
            other => {
                if written + 1 > output.len() {
                    return Err(EncodeError::OutputTooSmall);
                }
                output[written] = other;
                written += 1;
            }
        }
    }

    if written >= output.len() {
        return Err(EncodeError::OutputTooSmall);
    }
    output[written] = FEND;
    written += 1;
    Ok(written)
}

/// The value byte is not escaped, matching RNS, whose config values are clamped below the delimiter bytes.
#[must_use]
pub fn command_frame(command: u8, value: u8) -> [u8; 4] {
    [FEND, command, value, FEND]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    FrameTooBig,
}

/// Non-data command frames are consumed and never yielded; bytes outside any frame are dropped. A payload past the sink's capacity is rejected with [`DecodeError::FrameTooBig`] and scanning realigns at the next `FEND`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KissScanner {
    in_frame: bool,
    command: u8,
    saw_escape: bool,
}

impl Default for KissScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl KissScanner {
    pub const fn new() -> Self {
        Self {
            in_frame: false,
            command: CMD_UNKNOWN,
            saw_escape: false,
        }
    }

    pub fn reset(&mut self) {
        self.in_frame = false;
        self.command = CMD_UNKNOWN;
        self.saw_escape = false;
    }

    pub fn next_frame_into(
        &mut self,
        input: &[u8],
        offset: &mut usize,
        sink: &mut dyn FrameSink,
    ) -> Result<Option<usize>, DecodeError> {
        while *offset < input.len() {
            let byte = input[*offset];
            *offset += 1;
            if self.feed_one_into(byte, sink)? {
                return Ok(Some(sink.frame_len()));
            }
        }
        Ok(None)
    }

    fn feed_one_into(&mut self, byte: u8, sink: &mut dyn FrameSink) -> Result<bool, DecodeError> {
        if self.in_frame && byte == FEND && self.command == CMD_DATA {
            self.in_frame = false;
            self.saw_escape = false;
            return Ok(true);
        }

        // Any other FEND opens or reopens a frame. Back-to-back frames share a FEND under the reference's `FEND data FEND FEND data FEND` layout; an empty `FEND FEND` reopens harmlessly because its command stays CMD_UNKNOWN and never yields.
        if byte == FEND {
            self.in_frame = true;
            self.command = CMD_UNKNOWN;
            self.saw_escape = false;
            sink.clear();
            return Ok(false);
        }

        if !self.in_frame {
            return Ok(false);
        }

        if self.command == CMD_UNKNOWN && sink.frame_len() == 0 {
            self.command = byte & 0x0F;
            return Ok(false);
        }

        if self.command != CMD_DATA {
            return Ok(false);
        }

        if byte == FESC {
            self.saw_escape = true;
            return Ok(false);
        }

        let payload_byte = if self.saw_escape {
            self.saw_escape = false;
            match byte {
                TFEND => FEND,
                TFESC => FESC,
                // A non-canonical escape is kept verbatim, matching the reference.
                other => other,
            }
        } else {
            byte
        };

        if sink.push(payload_byte).is_err() {
            sink.clear();
            self.reset();
            return Err(DecodeError::FrameTooBig);
        }
        Ok(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KissDecoder<const FRAME_CAP: usize> {
    scanner: KissScanner,
    buffer: FrameBuffer<FRAME_CAP>,
}

impl<const FRAME_CAP: usize> Default for KissDecoder<FRAME_CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const FRAME_CAP: usize> KissDecoder<FRAME_CAP> {
    pub const fn new() -> Self {
        Self {
            scanner: KissScanner::new(),
            buffer: FrameBuffer::new(),
        }
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.scanner.reset();
    }

    pub fn feed(&mut self, byte: u8) -> Result<Option<&[u8]>, DecodeError> {
        if self.scanner.feed_one_into(byte, &mut self.buffer)? {
            Ok(Some(self.buffer.as_slice()))
        } else {
            Ok(None)
        }
    }

    pub fn feed_slice(&mut self, input: &[u8], mut on_frame: impl FnMut(&[u8])) {
        let mut offset = 0;
        while offset < input.len() {
            match self.feed_slice_next(input, &mut offset) {
                Ok(Some(frame)) => on_frame(frame),
                Ok(None) => break,
                Err(DecodeError::FrameTooBig) => {}
            }
        }
    }

    pub fn feed_slice_next<'a>(
        &'a mut self,
        input: &[u8],
        offset: &mut usize,
    ) -> Result<Option<&'a [u8]>, DecodeError> {
        match self
            .scanner
            .next_frame_into(input, offset, &mut self.buffer)?
        {
            Some(_) => Ok(Some(self.buffer.as_slice())),
            None => Ok(None),
        }
    }
}

/// Keeps the whole command byte because RNode commands span `0x00..=0x90`. A closing `FEND` also opens the next frame, matching RNode's batched detect query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KissCommandDecoder<const FRAME_CAP: usize> {
    buffer: FrameBuffer<FRAME_CAP>,
    in_frame: bool,
    command: u8,
    saw_escape: bool,
    /// Defers clearing the reopened frame until the yielded payload borrow ends.
    yielded: bool,
}

impl<const FRAME_CAP: usize> Default for KissCommandDecoder<FRAME_CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const FRAME_CAP: usize> KissCommandDecoder<FRAME_CAP> {
    pub const fn new() -> Self {
        Self {
            buffer: FrameBuffer::new(),
            in_frame: false,
            command: CMD_UNKNOWN,
            saw_escape: false,
            yielded: false,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.in_frame = false;
        self.command = CMD_UNKNOWN;
        self.saw_escape = false;
        self.yielded = false;
    }

    pub fn feed(&mut self, byte: u8) -> Result<Option<(u8, &[u8])>, DecodeError> {
        if self.feed_one(byte)? {
            Ok(Some((self.command, self.buffer.as_slice())))
        } else {
            Ok(None)
        }
    }

    pub fn feed_slice(&mut self, input: &[u8], mut on_frame: impl FnMut(u8, &[u8])) {
        let mut offset = 0;
        while offset < input.len() {
            match self.feed_slice_next(input, &mut offset) {
                Ok(Some((command, payload))) => on_frame(command, payload),
                Ok(None) => break,
                Err(DecodeError::FrameTooBig) => {}
            }
        }
    }

    pub fn feed_slice_next<'a>(
        &'a mut self,
        input: &[u8],
        offset: &mut usize,
    ) -> Result<Option<(u8, &'a [u8])>, DecodeError> {
        while *offset < input.len() {
            let byte = input[*offset];
            *offset += 1;
            if self.feed_one(byte)? {
                return Ok(Some((self.command, self.buffer.as_slice())));
            }
        }
        Ok(None)
    }

    fn feed_one(&mut self, byte: u8) -> Result<bool, DecodeError> {
        // The previous call yielded a frame whose closing FEND reopened the next; now that the borrow is done, clear the buffer for it.
        if self.yielded {
            self.yielded = false;
            self.buffer.clear();
            self.command = CMD_UNKNOWN;
            self.saw_escape = false;
        }

        if byte == FEND {
            if self.in_frame && self.command != CMD_UNKNOWN {
                self.yielded = true;
                self.in_frame = true;
                return Ok(true);
            }
            self.in_frame = true;
            self.command = CMD_UNKNOWN;
            self.saw_escape = false;
            self.buffer.clear();
            return Ok(false);
        }

        if !self.in_frame {
            return Ok(false);
        }

        if self.command == CMD_UNKNOWN && self.buffer.frame_len() == 0 {
            self.command = byte;
            return Ok(false);
        }

        if byte == FESC {
            self.saw_escape = true;
            return Ok(false);
        }
        let payload_byte = if self.saw_escape {
            self.saw_escape = false;
            match byte {
                TFEND => FEND,
                TFESC => FESC,
                other => other,
            }
        } else {
            byte
        };
        if self.buffer.push(payload_byte).is_err() {
            self.reset();
            return Err(DecodeError::FrameTooBig);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const TEST_FRAME_CAP: usize = 1024;

    fn decode_all(bytes: &[u8]) -> std::vec::Vec<std::vec::Vec<u8>> {
        let mut decoder: KissDecoder<TEST_FRAME_CAP> = KissDecoder::new();
        let mut frames = std::vec::Vec::new();
        for &b in bytes {
            if let Ok(Some(frame)) = decoder.feed(b) {
                frames.push(frame.to_vec());
            }
        }
        frames
    }

    fn decode_all_slice(bytes: &[u8]) -> std::vec::Vec<std::vec::Vec<u8>> {
        let mut decoder: KissDecoder<TEST_FRAME_CAP> = KissDecoder::new();
        let mut frames = std::vec::Vec::new();
        decoder.feed_slice(bytes, |frame| frames.push(frame.to_vec()));
        frames
    }

    #[test]
    fn empty_payload_encodes_to_fend_data_fend() {
        let mut out = [0u8; 8];
        let n = encode(&[], &mut out).unwrap();
        assert_eq!(&out[..n], &[FEND, CMD_DATA, FEND]);
    }

    #[test]
    fn non_special_bytes_pass_through_unescaped() {
        let payload = [0x01, 0x02, 0x03, 0x55];
        let mut out = [0u8; 32];
        let n = encode(&payload, &mut out).unwrap();
        assert_eq!(&out[..n], &[FEND, CMD_DATA, 0x01, 0x02, 0x03, 0x55, FEND]);
    }

    #[test]
    fn fend_byte_in_payload_is_transpose_escaped() {
        let payload = [0x01, FEND, 0x02];
        let mut out = [0u8; 32];
        let n = encode(&payload, &mut out).unwrap();
        assert_eq!(&out[..n], &[FEND, CMD_DATA, 0x01, FESC, TFEND, 0x02, FEND]);
    }

    #[test]
    fn fesc_byte_in_payload_is_transpose_escaped() {
        let payload = [FESC];
        let mut out = [0u8; 32];
        let n = encode(&payload, &mut out).unwrap();
        assert_eq!(&out[..n], &[FEND, CMD_DATA, FESC, TFESC, FEND]);
    }

    #[test]
    fn encode_to_undersized_buffer_returns_output_too_small() {
        let payload = [0x01, 0x02, 0x03];
        let mut tiny = [0u8; 4];
        assert_eq!(
            encode(&payload, &mut tiny),
            Err(EncodeError::OutputTooSmall)
        );
    }

    #[test]
    fn max_encoded_len_bounds_the_worst_case() {
        let payload = [FEND; 10];
        let mut out = [0u8; max_encoded_len(10)];
        let n = encode(&payload, &mut out).unwrap();
        assert_eq!(n, max_encoded_len(10));
    }

    #[test]
    fn command_frame_wraps_a_single_value() {
        assert_eq!(
            command_frame(CMD_TXDELAY, 0x05),
            [FEND, CMD_TXDELAY, 0x05, FEND]
        );
    }

    #[test]
    fn decoder_yields_payload_when_the_closing_fend_arrives() {
        let bytes = [FEND, CMD_DATA, 0x01, 0x02, 0x03, FEND];
        assert_eq!(decode_all(&bytes), std::vec![std::vec![0x01, 0x02, 0x03]]);
    }

    #[test]
    fn decoder_unescapes_transposed_fend_and_fesc() {
        let bytes = [FEND, CMD_DATA, FESC, TFEND, FESC, TFESC, 0x55, FEND];
        assert_eq!(decode_all(&bytes), std::vec![std::vec![FEND, FESC, 0x55]]);
    }

    #[test]
    fn decoder_strips_the_port_nibble_from_the_command() {
        let bytes = [FEND, 0x10, 0xAB, 0xCD, FEND];
        assert_eq!(decode_all(&bytes), std::vec![std::vec![0xAB, 0xCD]]);
    }

    #[test]
    fn decoder_ignores_non_data_command_frames() {
        let bytes = [FEND, CMD_TXDELAY, 0x05, FEND];
        assert_eq!(decode_all(&bytes), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn decoder_handles_back_to_back_frames_with_a_shared_double_fend() {
        let bytes = [FEND, CMD_DATA, 0x01, FEND, FEND, CMD_DATA, 0x02, FEND];
        assert_eq!(
            decode_all(&bytes),
            std::vec![std::vec![0x01], std::vec![0x02]]
        );
    }

    #[test]
    fn stray_bytes_outside_a_frame_are_dropped_then_the_decoder_realigns() {
        let bytes = [0xAA, 0xBB, FEND, CMD_DATA, 0x01, FEND];
        assert_eq!(decode_all(&bytes), std::vec![std::vec![0x01]]);
    }

    #[test]
    fn empty_data_frame_yields_an_empty_payload() {
        let bytes = [FEND, CMD_DATA, FEND];
        assert_eq!(decode_all(&bytes), std::vec![Vec::<u8>::new()]);
    }

    #[test]
    fn frame_exceeding_cap_returns_frame_too_big_and_auto_resets() {
        let mut decoder: KissDecoder<2> = KissDecoder::new();
        assert_eq!(decoder.feed(FEND).unwrap(), None);
        assert_eq!(decoder.feed(CMD_DATA).unwrap(), None);
        assert_eq!(decoder.feed(0x01).unwrap(), None);
        assert_eq!(decoder.feed(0x02).unwrap(), None);
        assert_eq!(decoder.feed(0x03), Err(DecodeError::FrameTooBig));

        assert_eq!(decoder.feed(FEND).unwrap(), None);
        assert_eq!(decoder.feed(CMD_DATA).unwrap(), None);
        assert_eq!(decoder.feed(0xAB).unwrap(), None);
        let frame = decoder.feed(FEND).unwrap().unwrap();
        assert_eq!(frame, &[0xAB]);
    }

    const RAW_ANNOUNCE_HEX: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                    59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                    0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                    7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                    4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn bytes_from_hex(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    #[test]
    fn a_real_rns_announce_round_trips_through_encode_then_decode() {
        let raw = bytes_from_hex(RAW_ANNOUNCE_HEX);
        let mut framed = std::vec![0u8; max_encoded_len(raw.len())];
        let n = encode(&raw, &mut framed).unwrap();
        let framed = &framed[..n];

        assert_eq!(framed[0], FEND);
        assert_eq!(framed[1], CMD_DATA);
        assert_eq!(framed[framed.len() - 1], FEND);
        assert_eq!(decode_all(framed), std::vec![raw]);
    }

    proptest! {
        #[test]
        fn arbitrary_payloads_round_trip_through_encode_then_decode(
            payload in prop::collection::vec(any::<u8>(), 0..512),
        ) {
            let mut framed = std::vec![0u8; max_encoded_len(payload.len())];
            let n = encode(&payload, &mut framed).unwrap();
            prop_assert_eq!(decode_all(&framed[..n]), std::vec![payload]);
        }

        #[test]
        fn feed_slice_matches_feeding_byte_by_byte(
            bytes in proptest::collection::vec(any::<u8>(), 0..4096)
        ) {
            prop_assert_eq!(decode_all_slice(&bytes), decode_all(&bytes));
        }

        #[test]
        fn streaming_decoder_handles_arbitrary_chunk_boundaries(
            payload in prop::collection::vec(any::<u8>(), 0..512),
            chunk_size in 1usize..16,
        ) {
            let mut framed = std::vec![0u8; max_encoded_len(payload.len())];
            let n = encode(&payload, &mut framed).unwrap();
            let framed = &framed[..n];

            let mut decoder: KissDecoder<TEST_FRAME_CAP> = KissDecoder::new();
            let mut frames = std::vec::Vec::new();
            for chunk in framed.chunks(chunk_size) {
                for &b in chunk {
                    if let Ok(Some(frame)) = decoder.feed(b) {
                        frames.push(frame.to_vec());
                    }
                }
            }
            prop_assert_eq!(frames, std::vec![payload]);
        }
    }

    fn decode_all_commands(bytes: &[u8]) -> std::vec::Vec<(u8, std::vec::Vec<u8>)> {
        let mut decoder: KissCommandDecoder<TEST_FRAME_CAP> = KissCommandDecoder::new();
        let mut frames = std::vec::Vec::new();
        for &b in bytes {
            if let Ok(Some((command, payload))) = decoder.feed(b) {
                frames.push((command, payload.to_vec()));
            }
        }
        frames
    }

    #[test]
    fn encode_with_command_frames_an_arbitrary_command() {
        let mut out = [0u8; 16];
        let n = encode_with_command(0x01, &[0x01, 0x02, 0x03, 0x04], &mut out).unwrap();
        assert_eq!(&out[..n], &[FEND, 0x01, 0x01, 0x02, 0x03, 0x04, FEND]);
    }

    #[test]
    fn encode_with_command_escapes_special_bytes_in_the_payload() {
        let mut out = [0u8; 16];
        let n = encode_with_command(0x02, &[FEND, FESC], &mut out).unwrap();
        assert_eq!(&out[..n], &[FEND, 0x02, FESC, TFEND, FESC, TFESC, FEND]);
    }

    #[test]
    fn the_command_decoder_yields_the_whole_command_byte_unmasked() {
        assert_eq!(
            decode_all_commands(&[FEND, 0x90, 0xAB, FEND]),
            std::vec![(0x90u8, std::vec![0xAB])]
        );
        assert_eq!(
            decode_all_commands(&[FEND, 0x10, 0xAB, FEND]),
            std::vec![(0x10u8, std::vec![0xAB])]
        );
    }

    #[test]
    fn the_command_decoder_splits_single_fend_separated_frames() {
        let stream = [
            FEND, 0x08, 0x46, FEND, 0x50, 0x01, 0x34, FEND, 0x48, 0x80, FEND,
        ];
        assert_eq!(
            decode_all_commands(&stream),
            std::vec![
                (0x08u8, std::vec![0x46]),
                (0x50u8, std::vec![0x01, 0x34]),
                (0x48u8, std::vec![0x80]),
            ]
        );
    }

    #[test]
    fn the_command_decoder_handles_double_fend_separated_frames() {
        let stream = [FEND, 0x03, 0x25, FEND, FEND, 0x04, 0x0C, FEND];
        assert_eq!(
            decode_all_commands(&stream),
            std::vec![(0x03u8, std::vec![0x25]), (0x04u8, std::vec![0x0C])]
        );
    }

    #[test]
    fn the_command_decoder_unescapes_payloads() {
        let stream = [FEND, 0x01, FESC, TFEND, FESC, TFESC, 0x55, FEND];
        assert_eq!(
            decode_all_commands(&stream),
            std::vec![(0x01u8, std::vec![FEND, FESC, 0x55])]
        );
    }

    proptest! {
        #[test]
        fn a_command_frame_round_trips_through_encode_then_decode(
            command in 0x00u8..=0x90,
            payload in prop::collection::vec(any::<u8>(), 0..256),
        ) {
            let mut framed = std::vec![0u8; max_encoded_len(payload.len())];
            let n = encode_with_command(command, &payload, &mut framed).unwrap();
            prop_assert_eq!(decode_all_commands(&framed[..n]), std::vec![(command, payload)]);
        }
    }
}
