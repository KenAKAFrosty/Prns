//! Byte-level framing codecs shared by the byte-stream interfaces. Each codec turns the
//! RNS packet boundary into a self-delimiting frame on the wire and back:
//!
//! - [`rns_serial_framing`] — HDLC-like octet-stuffing (`0x7E` flag), what `SerialInterface`
//!   and a plain TCP interface speak.
//! - [`kiss_framing`] — KISS TNC framing (`0xC0` FEND), what `KISSInterface`,
//!   `AX25KISSInterface`, and an `RNodeInterface`'s host link speak.
//!
//! Both decoders accumulate a frame in the same fixed-capacity [`FrameBuffer`], which lives
//! here once rather than in each codec: it is the heap (`std`) or inline `heapless` (no_std)
//! backing a decoder's in-progress frame, capped at the interface's frame ceiling so a
//! runaway stream can never grow it without bound.

pub mod kiss_framing;
pub mod rns_serial_framing;

#[cfg(not(feature = "std"))]
use heapless::Vec as HeaplessVec;

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

    pub(crate) fn clear(&mut self) {
        self.bytes.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) const fn capacity(&self) -> usize {
        FRAME_CAP
    }

    pub(crate) fn extend_from_slice(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if bytes.len() > FRAME_CAP.saturating_sub(self.bytes.len()) {
            return Err(());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn push(&mut self, byte: u8) -> Result<(), ()> {
        if self.bytes.len() >= FRAME_CAP {
            return Err(());
        }
        self.bytes.push(byte);
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

    pub(crate) fn clear(&mut self) {
        self.bytes.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.bytes.capacity()
    }

    pub(crate) fn extend_from_slice(&mut self, bytes: &[u8]) -> Result<(), ()> {
        self.bytes.extend_from_slice(bytes)
    }

    pub(crate) fn push(&mut self, byte: u8) -> Result<(), ()> {
        self.bytes.push(byte).map_err(|_| ())
    }
}
