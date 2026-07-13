//! Byte-level framing codecs shared by the byte-stream interfaces. Each codec turns the RNS
//! packet boundary into a self-delimiting frame on the wire and back:
//!
//! - [`rns_serial_framing`]: HDLC-like octet-stuffing (`0x7E` flag), what `SerialInterface`
//!   and a plain TCP interface speak.
//! - [`kiss_framing`]: KISS TNC framing (`0xC0` FEND), what `KISSInterface`,
//!   `AX25KISSInterface`, and an `RNodeInterface`'s host link speak.

pub mod kiss_framing;
pub mod rns_serial_framing;

#[cfg(not(feature = "std"))]
use heapless::Vec as HeaplessVec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSinkError {
    Full,
}

/// Where a streaming deframer writes the frame it is decoding, so the bytes land once in the
/// storage that will carry them onward — an interface seam's granted slot on a host, a
/// `FrameBuffer` inside the self-buffered decoders.
/// Writes are all-or-nothing: a `push` or `extend_from_slice` that would exceed the sink's
/// capacity appends nothing and returns [`FrameSinkError::Full`], so a rejected frame never
/// leaves a partial tail behind.
pub trait FrameSink {
    fn clear(&mut self);
    fn frame_len(&self) -> usize;
    fn free_capacity(&self) -> usize;
    fn push(&mut self, byte: u8) -> Result<(), FrameSinkError>;
    fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError>;
}

/// Uncapped sink for tests and adapters that re-frame in their own scratch before forwarding.
#[cfg(feature = "std")]
impl FrameSink for std::vec::Vec<u8> {
    fn clear(&mut self) {
        std::vec::Vec::clear(self);
    }

    fn frame_len(&self) -> usize {
        self.len()
    }

    fn free_capacity(&self) -> usize {
        usize::MAX - self.len()
    }

    fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
        std::vec::Vec::push(self, byte);
        Ok(())
    }

    fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
        std::vec::Vec::extend_from_slice(self, run);
        Ok(())
    }
}

/// The fixed-capacity buffer a streaming decoder fills with one frame's payload. `std` hosts
/// keep a heap `Vec` (capped on push/extend) for its optimized bulk copy; no_std targets keep
/// an inline `heapless::Vec`. The `FRAME_CAP` const is the hard ceiling either way — a frame
/// that would exceed it is rejected by the caller, never silently grown.
#[cfg(feature = "std")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameBuffer<const FRAME_CAP: usize> {
    bytes: std::vec::Vec<u8>,
}

#[cfg(feature = "std")]
impl<const FRAME_CAP: usize> FrameBuffer<FRAME_CAP> {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: std::vec::Vec::new(),
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(feature = "std")]
impl<const FRAME_CAP: usize> FrameSink for FrameBuffer<FRAME_CAP> {
    fn clear(&mut self) {
        self.bytes.clear();
    }

    fn frame_len(&self) -> usize {
        self.bytes.len()
    }

    fn free_capacity(&self) -> usize {
        FRAME_CAP.saturating_sub(self.bytes.len())
    }

    fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
        if self.bytes.len() >= FRAME_CAP {
            return Err(FrameSinkError::Full);
        }
        self.bytes.push(byte);
        Ok(())
    }

    fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
        if run.len() > FRAME_CAP.saturating_sub(self.bytes.len()) {
            return Err(FrameSinkError::Full);
        }
        self.bytes.extend_from_slice(run);
        Ok(())
    }
}

#[cfg(not(feature = "std"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameBuffer<const FRAME_CAP: usize> {
    bytes: HeaplessVec<u8, FRAME_CAP>,
}

#[cfg(not(feature = "std"))]
impl<const FRAME_CAP: usize> FrameBuffer<FRAME_CAP> {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: HeaplessVec::new(),
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(not(feature = "std"))]
impl<const FRAME_CAP: usize> FrameSink for FrameBuffer<FRAME_CAP> {
    fn clear(&mut self) {
        self.bytes.clear();
    }

    fn frame_len(&self) -> usize {
        self.bytes.len()
    }

    fn free_capacity(&self) -> usize {
        FRAME_CAP.saturating_sub(self.bytes.len())
    }

    fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
        self.bytes.push(byte).map_err(|_| FrameSinkError::Full)
    }

    fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
        self.bytes
            .extend_from_slice(run)
            .map_err(|_| FrameSinkError::Full)
    }
}
