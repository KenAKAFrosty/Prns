use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};

pub struct FrameSlot<const SLOT: usize> {
    /// The interface this frame belongs to — its source on an inbound lane, its target on an egress
    /// lane. A lane shared across a supervisor's fleet uses this tag to demux: the reactor reads it
    /// as the ingested frame's `source_interface`, and a supervisor reads it to pick the peer to
    /// send to. A 1:1 lane carries the interface's own id and the tag is read trivially.
    pub interface_id: InterfaceId,
    pub len: usize,
    pub bytes: [u8; SLOT],
}

impl<const SLOT: usize> FrameSlot<SLOT> {
    pub const fn empty() -> Self {
        Self {
            interface_id: InterfaceId::new([0u8; INTERFACE_ID_LEN]),
            len: 0,
            bytes: [0u8; SLOT],
        }
    }

    pub fn fill(&mut self, frame: &[u8]) {
        let len = frame.len().min(SLOT);
        self.bytes[..len].copy_from_slice(&frame[..len]);
        self.len = len;
    }

    /// Tag the slot with the interface it belongs to, then fill it. The single call every filler
    /// uses so the tag and the bytes are never out of step.
    pub fn fill_for(&mut self, interface_id: InterfaceId, frame: &[u8]) {
        self.interface_id = interface_id;
        self.fill(frame);
    }

    pub fn frame(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn frame_mut(&mut self) -> &mut [u8] {
        let len = self.len;
        &mut self.bytes[..len]
    }
}

/// The filling half of one interface's grant lane: take the next free slot,
/// write the frame in place, commit it toward the consumer. A lane is SPSC
/// and slot-bounded, so a full lane backpressures the producer.
#[allow(async_fn_in_trait)]
pub trait GrantProducer<const SLOT: usize> {
    fn try_grant(&mut self) -> Option<&mut FrameSlot<SLOT>>;
    async fn grant(&mut self) -> &mut FrameSlot<SLOT>;
    fn commit(&mut self);
}

/// The draining half: borrow the oldest committed slot in place, mutably, so
/// IFAC unmasking and in-place decryption never copy. Afterwards, release it back
/// to the producer.
#[allow(async_fn_in_trait)]
pub trait GrantConsumer<const SLOT: usize> {
    fn try_peek(&mut self) -> Option<&mut FrameSlot<SLOT>>;
    async fn peek(&mut self) -> &mut FrameSlot<SLOT>;
    fn release(&mut self);
}

/// A consumer with its slot size erased, so one reactor can hold lanes sized
/// to different interfaces in a single slice. A blanket impl over every
/// `GrantConsumer<SLOT>` is impossible (the const parameter is unconstrained)
/// so each concrete consumer implements this by hand.
pub trait AnyGrantConsumer {
    /// Peek the oldest committed frame and the interface it belongs to. The id is the slot's tag —
    /// the source interface to ingest the frame under — which for a shared lane is the child the
    /// frame came from, and for a 1:1 lane is the interface's own id.
    fn try_peek_frame(&mut self) -> Option<(InterfaceId, &mut [u8])>;
    fn release_frame(&mut self);
}

/// A producer with its slot size erased: grant, tag, fill, and commit collapse into one call,
/// refusing (not truncating) a frame larger than the lane's slots. The `interface_id` tags the slot
/// so the draining side (a supervisor) knows which child/peer the frame is for.
pub trait AnyGrantProducer {
    fn try_fill_frame_for(&mut self, interface_id: InterfaceId, frame: &[u8]) -> bool;
}
