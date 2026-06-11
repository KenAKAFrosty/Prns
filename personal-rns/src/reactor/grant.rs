pub struct FrameSlot<const SLOT: usize> {
    pub len: usize,
    pub bytes: [u8; SLOT],
}

impl<const SLOT: usize> FrameSlot<SLOT> {
    pub const fn empty() -> Self {
        Self {
            len: 0,
            bytes: [0u8; SLOT],
        }
    }

    pub fn fill(&mut self, frame: &[u8]) {
        let len = frame.len().min(SLOT);
        self.bytes[..len].copy_from_slice(&frame[..len]);
        self.len = len;
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
    fn try_peek_frame(&mut self) -> Option<&mut [u8]>;
    fn release_frame(&mut self);
}

/// A producer with its slot size erased: grant, fill, and commit collapse into
/// one call, refusing (not truncating) a frame larger than the lane's slots.
pub trait AnyGrantProducer {
    fn try_fill_frame(&mut self, frame: &[u8]) -> bool;
}
