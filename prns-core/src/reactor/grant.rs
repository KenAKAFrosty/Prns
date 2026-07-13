use crate::engine::FanTarget;
use crate::interfaces::{FrameSink, FrameSinkError, InterfaceId, INTERFACE_ID_LEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// repr(C): crosses the dual-core channel inside `FrameSlot`; see the layout note on `EngineCommand`.
#[repr(C)]
pub enum FrameTarget {
    Direct(InterfaceId),
    Fan(FanTarget),
}

pub struct FrameSlot<const SLOT: usize> {
    pub target: FrameTarget,
    pub len: usize,
    pub bytes: [u8; SLOT],
}

impl<const SLOT: usize> FrameSlot<SLOT> {
    pub const fn empty() -> Self {
        Self {
            target: FrameTarget::Direct(InterfaceId::new([0u8; INTERFACE_ID_LEN])),
            len: 0,
            bytes: [0u8; SLOT],
        }
    }

    fn fill(&mut self, frame: &[u8]) {
        debug_assert!(
            frame.len() <= SLOT,
            "a {}-byte frame cannot fit this {SLOT}-byte slot",
            frame.len()
        );
        let len = frame.len().min(SLOT);
        self.bytes[..len].copy_from_slice(&frame[..len]);
        self.len = len;
    }

    pub fn fill_for(&mut self, interface_id: InterfaceId, frame: &[u8]) {
        self.target = FrameTarget::Direct(interface_id);
        self.fill(frame);
    }

    pub fn fill_for_fan(&mut self, fan: FanTarget, frame: &[u8]) {
        self.target = FrameTarget::Fan(fan);
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

/// As a [`FrameSink`] the slot is a streaming deframer's destination: `len` is the
/// accumulation cursor, and the committer stamps `target` when the frame is done.
impl<const SLOT: usize> FrameSink for FrameSlot<SLOT> {
    fn clear(&mut self) {
        self.len = 0;
    }

    fn frame_len(&self) -> usize {
        self.len
    }

    fn free_capacity(&self) -> usize {
        SLOT.saturating_sub(self.len)
    }

    fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
        if self.len >= SLOT {
            return Err(FrameSinkError::Full);
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        Ok(())
    }

    fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
        if run.len() > SLOT.saturating_sub(self.len) {
            return Err(FrameSinkError::Full);
        }
        self.bytes[self.len..self.len + run.len()].copy_from_slice(run);
        self.len += run.len();
        Ok(())
    }
}

#[allow(async_fn_in_trait)]
pub trait GrantProducer<const SLOT: usize> {
    fn try_grant(&mut self) -> Option<&mut FrameSlot<SLOT>>;
    async fn grant(&mut self) -> &mut FrameSlot<SLOT>;
    fn commit(&mut self);
}

#[allow(async_fn_in_trait)]
pub trait GrantConsumer<const SLOT: usize> {
    fn try_peek(&mut self) -> Option<&mut FrameSlot<SLOT>>;
    async fn peek(&mut self) -> &mut FrameSlot<SLOT>;
    fn release(&mut self);
}

pub trait AnyGrantConsumer {
    fn try_peek_frame(&mut self) -> Option<(FrameTarget, &mut [u8])>;
    fn release_frame(&mut self);
}

pub trait AnyGrantProducer {
    fn try_fill_frame_for(&mut self, interface_id: InterfaceId, frame: &[u8]) -> bool;
    fn try_fill_frame_fan(&mut self, fan: FanTarget, frame: &[u8]) -> bool;
}
