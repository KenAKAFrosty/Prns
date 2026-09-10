use core::fmt;

const TRANSCRIPT_CAPACITY: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum EventKind {
    Scenario = 1,
    SpiWrite = 2,
    SpiRead = 3,
    Wait = 4,
    Reset = 5,
    Delay = 6,
    Result = 7,
    Poll = 8,
    Recovery = 9,
    Complete = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptError {
    PayloadTooLong,
    CapacityExceeded,
}

pub struct Transcript {
    bytes: [u8; TRANSCRIPT_CAPACITY],
    len: usize,
    overflowed: bool,
}

impl Transcript {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            bytes: [0; TRANSCRIPT_CAPACITY],
            len: 0,
            overflowed: false,
        }
    }

    pub(crate) fn record(
        &mut self,
        kind: EventKind,
        payload: &[u8],
    ) -> Result<(), TranscriptError> {
        let Ok(payload_len) = u16::try_from(payload.len()) else {
            self.overflowed = true;
            return Err(TranscriptError::PayloadTooLong);
        };
        let Some(end) = self.len.checked_add(3 + payload.len()) else {
            self.overflowed = true;
            return Err(TranscriptError::CapacityExceeded);
        };
        if end > self.bytes.len() {
            self.overflowed = true;
            return Err(TranscriptError::CapacityExceeded);
        }
        let length = payload_len.to_be_bytes();
        self.bytes[self.len] = kind as u8;
        self.bytes[self.len + 1..self.len + 3].copy_from_slice(&length);
        self.bytes[self.len + 3..end].copy_from_slice(payload);
        self.len = end;
        Ok(())
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8] {
        self.bytes.split_at(self.len).0
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub(crate) const fn overflowed(&self) -> bool {
        self.overflowed
    }
}

pub(crate) struct HexBytes<'a>(pub &'a [u8]);

impl fmt::Display for HexBytes<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}
