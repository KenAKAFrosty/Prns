use crate::engine::FanTarget;
use crate::interfaces::{FrameSink, FrameSinkError, InterfaceId, PacketPhyStats, INTERFACE_ID_LEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// repr(C): crosses the dual-core channel inside `FrameSlot`; see the layout note on `PrnsCommand`.
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
    /// Original local lane admission time; not transmitted on the wire.
    pub enqueued_at: Option<crate::engine::InstantMillis>,
    /// Administrative generation of a locally bound lane; never transmitted.
    /// The lane stamps this from grant admission, not from commit time.
    pub lane_generation: Option<u32>,
}

impl<const FRAME: usize> FrameSlot<FRAME> {
    pub const fn empty() -> Self {
        Self {
            target: FrameTarget::Direct(InterfaceId::new([0u8; INTERFACE_ID_LEN])),
            len: 0,
            enqueued_at: None,
            lane_generation: None,
            bytes: [0u8; FRAME],
            packet_phy: PacketPhyStats {
                rssi: None,
                snr: None,
                quality: None,
            },
        }
    }

    /// Initialize caller-owned storage without a `FRAME`-sized stack temporary.
    /// Useful for frame rings allocated directly in external memory.
    #[allow(
        unsafe_code,
        reason = "field-wise initialization avoids a frame-sized stack temporary"
    )]
    pub fn initialize_empty(storage: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        let slot = storage.as_mut_ptr();
        macro_rules! initialize_header {
            ($($field:ident),+ $(,)?) => {
                // Exhaustive destructuring makes new fields a compile error here until
                // they are also initialized. The zero-capacity slot supplies the same
                // defaults as `empty()` without materializing a frame on the stack.
                let FrameSlot { bytes: _, $($field),+ } = FrameSlot::<0>::empty();
                // SAFETY: `slot` is exclusive, aligned storage for Self. Each listed
                // field is written once without forming a reference to uninitialized data.
                unsafe { $(core::ptr::addr_of_mut!((*slot).$field).write($field);)+ }
            };
        }
        initialize_header!(target, len, packet_phy, enqueued_at, lane_generation);
        // SAFETY: the byte array is a valid FRAME-byte region of the exclusive slot.
        // Every other field was initialized above, so the entire value is now valid.
        unsafe {
            core::ptr::addr_of_mut!((*slot).bytes)
                .cast::<u8>()
                .write_bytes(0, FRAME);
            storage.assume_init_mut()
        }
    }

    fn fill(&mut self, frame: &[u8]) {
        self.enqueued_at = None;
        self.lane_generation = None;
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
        self.enqueued_at = None;
        self.lane_generation = None;
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

pub trait ManifoldLaneReader: Send {
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

pub trait ManifoldLaneWriter: Send {
    fn try_write(&mut self, target: FrameTarget, frame: &[u8]) -> LaneWriteOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::InstantMillis;

    #[allow(unsafe_code, reason = "poison caller-owned uninitialized test storage")]
    fn check_in_place_initialization<const FRAME: usize>() {
        let mut storage = core::mem::MaybeUninit::<FrameSlot<FRAME>>::uninit();
        // SAFETY: MaybeUninit accepts any byte pattern. Poisoning the storage catches
        // fields accidentally relying on zero-filled allocator memory.
        unsafe {
            storage
                .as_mut_ptr()
                .cast::<u8>()
                .write_bytes(0xA5, core::mem::size_of::<FrameSlot<FRAME>>());
        }
        let slot = FrameSlot::initialize_empty(&mut storage);
        let expected = FrameSlot::<FRAME>::empty();
        assert_eq!(slot.target, expected.target);
        assert_eq!(slot.len, expected.len);
        assert_eq!(slot.bytes, expected.bytes);
        assert_eq!(slot.packet_phy, expected.packet_phy);
        assert_eq!(slot.enqueued_at, expected.enqueued_at);
        assert_eq!(slot.lane_generation, expected.lane_generation);
        assert_eq!(slot.frame(), &[]);
    }

    #[test]
    fn in_place_initialization_overwrites_poisoned_storage() {
        check_in_place_initialization::<0>();
        check_in_place_initialization::<1>();
        check_in_place_initialization::<512>();
        check_in_place_initialization::<4096>();
    }

    #[test]
    fn every_frame_reuse_path_clears_original_admission_time() {
        let mut slot = FrameSlot::<512>::empty();
        assert_eq!(slot.enqueued_at, None);
        assert_eq!(slot.lane_generation, None);
        let id = InterfaceId::new(*b"radio000");
        slot.enqueued_at = Some(InstantMillis(42));
        slot.lane_generation = Some(7);
        slot.fill_for(id, &[0xA5; 508]);
        assert_eq!(slot.enqueued_at, None);
        assert_eq!(slot.lane_generation, None);
        slot.enqueued_at = Some(InstantMillis(42));
        slot.lane_generation = Some(7);
        slot.fill_for_fan(FanTarget::All, &[0xB5; 508]);
        assert_eq!(slot.enqueued_at, None);
        assert_eq!(slot.lane_generation, None);
        slot.enqueued_at = Some(InstantMillis(42));
        slot.lane_generation = Some(7);
        slot.clear();
        slot.extend_from_slice(&[0xC5; 508]).unwrap();
        slot.push(0xD5).unwrap();
        assert_eq!(slot.enqueued_at, None);
        assert_eq!(slot.lane_generation, None);
        assert_eq!(slot.frame_len(), 509);
    }
}
