use super::{channel_mdu, MessageType};

/// RNS `SystemMessageTypes.SMT_STREAM_DATA`
pub const STREAM_DATA_TYPE: MessageType = MessageType(0xff00);
/// RNS `StreamDataMessage.STREAM_ID_MAX`
pub const STREAM_ID_MAX: u16 = 0x3fff;
const EOF_BIT: u16 = 0x8000;
const COMPRESSED_BIT: u16 = 0x4000;
const STREAM_ID_MASK: u16 = 0x3fff;
pub const HEADER_LEN: usize = 2;

/// RNS `RawChannelWriter.MAX_CHUNK_LEN` (16 KiB): the most input a single stream-data message carries, so a compressed message inflates to no more than this.
///
/// A writer packs up to this much into one message when it compresses to fit; a reader refuses a compressed chunk that would run past it.
pub const MAX_STREAM_CHUNK_LEN: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct StreamId(u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamIdError {
    TooLarge,
}

impl StreamId {
    pub const fn new(id: u16) -> Result<Self, StreamIdError> {
        if id <= STREAM_ID_MAX {
            Ok(Self(id))
        } else {
            Err(StreamIdError::TooLarge)
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamDataHeader {
    pub stream_id: StreamId,
    pub eof: bool,
    pub compressed: bool,
}

impl StreamDataHeader {
    pub fn to_bytes(self) -> [u8; HEADER_LEN] {
        let mut value = self.stream_id.0 & STREAM_ID_MASK;
        if self.eof {
            value |= EOF_BIT;
        }
        if self.compressed {
            value |= COMPRESSED_BIT;
        }
        value.to_be_bytes()
    }

    pub fn parse(bytes: [u8; HEADER_LEN]) -> Self {
        let value = u16::from_be_bytes(bytes);
        Self {
            stream_id: StreamId(value & STREAM_ID_MASK),
            eof: value & EOF_BIT != 0,
            compressed: value & COMPRESSED_BIT != 0,
        }
    }
}

pub const fn max_stream_payload(mtu: usize) -> usize {
    channel_mdu(mtu).saturating_sub(HEADER_LEN)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamDataFrame<'a> {
    pub header: StreamDataHeader,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDataParseError {
    TooShort,
}

pub fn parse(body: &[u8]) -> Result<StreamDataFrame<'_>, StreamDataParseError> {
    if body.len() < HEADER_LEN {
        return Err(StreamDataParseError::TooShort);
    }
    let header = StreamDataHeader::parse([body[0], body[1]]);
    Ok(StreamDataFrame {
        header,
        payload: &body[HEADER_LEN..],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDataWriteError {
    BufferTooShort,
}

pub fn write_frame(
    header: StreamDataHeader,
    payload: &[u8],
    buf: &mut [u8],
) -> Result<usize, StreamDataWriteError> {
    let end = HEADER_LEN + payload.len();
    if buf.len() < end {
        return Err(StreamDataWriteError::BufferTooShort);
    }
    buf[..HEADER_LEN].copy_from_slice(&header.to_bytes());
    buf[HEADER_LEN..end].copy_from_slice(payload);
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::BROADCAST_MTU;

    fn header(id: u16, eof: bool, compressed: bool) -> StreamDataHeader {
        StreamDataHeader {
            stream_id: StreamId::new(id).unwrap(),
            eof,
            compressed,
        }
    }

    #[test]
    fn header_packs_byte_identical_to_the_rns_stream_data_header() {
        assert_eq!(header(1, false, false).to_bytes(), [0x00, 0x01]);
        assert_eq!(header(2, true, false).to_bytes(), [0x80, 0x02]);
        assert_eq!(header(0, false, true).to_bytes(), [0x40, 0x00]);
        assert_eq!(header(STREAM_ID_MAX, true, true).to_bytes(), [0xff, 0xff]);
    }

    #[test]
    fn header_round_trips_every_flag_combination_and_id() {
        for (eof, compressed) in [(false, false), (true, false), (false, true), (true, true)] {
            for id in [0u16, 1, 0x1234, STREAM_ID_MAX] {
                let packed = header(id, eof, compressed);
                assert_eq!(StreamDataHeader::parse(packed.to_bytes()), packed);
            }
        }
    }

    #[test]
    fn stream_id_rejects_values_past_the_14_bit_ceiling() {
        assert!(StreamId::new(STREAM_ID_MAX).is_ok());
        assert!(StreamId::new(STREAM_ID_MAX + 1).is_err());
        assert!(StreamId::new(0x8000).is_err());
    }

    #[test]
    fn a_frame_round_trips_through_a_channel_body() {
        let head = header(7, true, false);
        let payload = b"the quick brown fox";
        let mut buf = [0u8; 64];
        let len = write_frame(head, payload, &mut buf).unwrap();
        let frame = parse(&buf[..len]).unwrap();
        assert_eq!(frame.header, head);
        assert_eq!(frame.payload, payload.as_slice());
    }

    #[test]
    fn parse_rejects_a_body_too_short_for_a_header() {
        assert_eq!(parse(&[0x00]), Err(StreamDataParseError::TooShort));
        assert_eq!(parse(&[]), Err(StreamDataParseError::TooShort));
    }

    #[test]
    fn write_frame_reports_a_buffer_too_short_for_header_plus_payload() {
        let mut buf = [0u8; 3];
        assert_eq!(
            write_frame(header(0, false, false), b"xxxx", &mut buf),
            Err(StreamDataWriteError::BufferTooShort)
        );
    }

    #[test]
    fn max_payload_is_the_channel_body_less_the_header() {
        assert_eq!(
            max_stream_payload(BROADCAST_MTU),
            channel_mdu(BROADCAST_MTU) - HEADER_LEN
        );
    }
}
