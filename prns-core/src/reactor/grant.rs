use crate::engine::FanTarget;
use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};

pub struct FrameSlot<const SLOT: usize> {
    pub interface_id: InterfaceId,
    pub fan: Option<FanTarget>,
    pub len: usize,
    pub bytes: [u8; SLOT],
}

impl<const SLOT: usize> FrameSlot<SLOT> {
    pub const fn empty() -> Self {
        Self {
            interface_id: InterfaceId::new([0u8; INTERFACE_ID_LEN]),
            fan: None,
            len: 0,
            bytes: [0u8; SLOT],
        }
    }

    pub fn fill(&mut self, frame: &[u8]) {
        let len = frame.len().min(SLOT);
        self.bytes[..len].copy_from_slice(&frame[..len]);
        self.len = len;
    }

    /// Clears any broadcast `fan`: a reused slot must not carry a stale fan into a direct send.
    pub fn fill_for(&mut self, interface_id: InterfaceId, frame: &[u8]) {
        self.interface_id = interface_id;
        self.fan = None;
        self.fill(frame);
    }

    pub fn fill_for_fan(&mut self, fan: FanTarget, frame: &[u8]) {
        self.fan = Some(fan);
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
    fn try_peek_frame(&mut self) -> Option<(InterfaceId, &mut [u8])>;
    fn release_frame(&mut self);
}

pub trait AnyGrantProducer {
    fn try_fill_frame_for(&mut self, interface_id: InterfaceId, frame: &[u8]) -> bool;
    fn try_fill_frame_fan(&mut self, fan: FanTarget, frame: &[u8]) -> bool;
}
