//! Substrate-neutral RX ingest: decode RNS serial frames and push each
//! completed frame into a zero-copy SPSC sink.
//!
//! This is the shared heart of an `InterfaceWorker`'s RX side in the host-runtime
//! model. It touches no hardware and knows nothing about how it's driven — a
//! sync poll loop and an async task call `ingest_bytes` identically. The only
//! substrate knob is the channel's `RawMutex` (cooperative-single-core vs
//! cross-core/threaded), which is a type parameter here, not a code change.
//!
//! It uses the **zero-copy** sink: a completed frame is written directly into
//! the channel's own slot storage (one copy, out of the decoder), and the
//! Manifold later reads that same slot in place — no owned message is moved
//! across the channel. Pushes are non-blocking (`try_send`); if the Manifold
//! has fallen behind and the queue is full, the frame is dropped rather than
//! stalling the drain (a worker's job is to keep the hardware FIFO empty).

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::zerocopy_channel::Sender;
use heapless::Vec as HeaplessVec;

use personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder;
use personal_rns::wire::MTU;

/// One RNS packet body, sized at the MTU. This is the zero-copy channel's slot
/// type: the storage lives in the channel, and producer/consumer borrow it in
/// turn rather than moving a copy through.
pub type PacketBytes = HeaplessVec<u8, MTU>;

/// Streaming RNS-serial frame reassembly feeding a zero-copy sink.
pub struct RnsFrameIngest {
    decoder: RnsSerialDecoder<MTU>,
}

impl RnsFrameIngest {
    pub fn new() -> Self {
        Self {
            decoder: RnsSerialDecoder::new(),
        }
    }

    /// Feed freshly-read bytes; publish each completed RNS frame into `sink`.
    /// Returns how many frames were published. Drops a frame (does not block) if
    /// the sink is full. Identical under every substrate.
    pub fn ingest_bytes<M: RawMutex>(
        &mut self,
        bytes: &[u8],
        sink: &mut Sender<'_, M, PacketBytes>,
    ) -> usize {
        let mut published = 0;
        for &byte in bytes {
            // A decode error (oversized frame) just resets the stream by being
            // ignored here; partial/empty frames carry no payload to publish.
            if let Ok(Some(frame)) = self.decoder.feed(byte) {
                if frame.is_empty() {
                    continue;
                }
                if let Some(slot) = sink.try_send() {
                    slot.clear();
                    let filled = slot.extend_from_slice(frame).is_ok();
                    if filled {
                        sink.send_done();
                        published += 1;
                    }
                }
            }
        }
        published
    }
}

impl Default for RnsFrameIngest {
    fn default() -> Self {
        Self::new()
    }
}
