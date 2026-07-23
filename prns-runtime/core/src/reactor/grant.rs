use crate::engine::FanTarget;
use crate::interfaces::{FrameSink, FrameSinkError, InterfaceId, PacketPhyStats, INTERFACE_ID_LEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// repr(C): crosses the dual-core channel inside `FrameSlot`; see the layout note on `EngineCommand`.
#[repr(C)]
pub enum FrameTarget {
    Direct(InterfaceId),
    Fan(FanTarget),
}

pub struct FrameSlot<const FRAME: usize> {
    pub target: FrameTarget,
    pub len: usize,
    pub bytes: [u8; FRAME],
    pub packet_phy: PacketPhyStats,
}

impl<const FRAME: usize> FrameSlot<FRAME> {
    pub const fn empty() -> Self {
        Self {
            target: FrameTarget::Direct(InterfaceId::new([0u8; INTERFACE_ID_LEN])),
            len: 0,
            bytes: [0u8; FRAME],
            packet_phy: PacketPhyStats {
                rssi: None,
                snr: None,
                quality: None,
            },
        }
    }

    fn fill(&mut self, frame: &[u8]) {
        self.packet_phy = PacketPhyStats::default();
        debug_assert!(
            frame.len() <= FRAME,
            "a {}-byte frame cannot fit this {FRAME}-byte slot",
            frame.len()
        );
        let len = frame.len().min(FRAME);
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

/// As a [`FrameSink`] the slot is a streaming deframer's destination: `len` is the accumulation cursor, and the committer stamps `target` when the frame is done.
impl<const FRAME: usize> FrameSink for FrameSlot<FRAME> {
    fn clear(&mut self) {
        self.len = 0;
        self.packet_phy = PacketPhyStats::default();
    }

    fn frame_len(&self) -> usize {
        self.len
    }

    fn free_capacity(&self) -> usize {
        FRAME.saturating_sub(self.len)
    }

    fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
        if self.len >= FRAME {
            return Err(FrameSinkError::Full);
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        Ok(())
    }

    fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
        if run.len() > FRAME.saturating_sub(self.len) {
            return Err(FrameSinkError::Full);
        }
        self.bytes[self.len..self.len + run.len()].copy_from_slice(run);
        self.len += run.len();
        Ok(())
    }
}

#[allow(async_fn_in_trait)]
pub trait GrantProducer<const FRAME: usize> {
    fn try_grant(&mut self) -> Option<&mut FrameSlot<FRAME>>;
    async fn grant(&mut self) -> &mut FrameSlot<FRAME>;
    fn commit(&mut self);
}

#[allow(async_fn_in_trait)]
pub trait GrantConsumer<const FRAME: usize> {
    fn try_peek(&mut self) -> Option<&mut FrameSlot<FRAME>>;
    async fn peek(&mut self) -> &mut FrameSlot<FRAME>;
    fn release(&mut self);
}

pub trait ReactorLaneReader: Send {
    fn try_read(&mut self) -> Option<(FrameTarget, PacketPhyStats, &mut [u8])>;
    fn release(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum LaneWriteOutcome {
    Written,
    Full,
    FrameTooLarge { frame_len: usize, capacity: usize },
}

pub trait ReactorLaneWriter: Send {
    fn try_write(&mut self, target: FrameTarget, frame: &[u8]) -> LaneWriteOutcome;
}
