use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::interfaces::{
    Capabilities, Interface, InterfaceId, InterfaceMode, InterfaceState, MediumKind,
    PointToPointInterface,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopbackError {
    /// A queued packet's byte length exceeds the caller's `buf` capacity
    /// passed to `try_read`. The packet has been consumed; the caller
    /// should retry future reads with a buffer at least the engine's
    /// MTU.
    BufferTooSmall { needed: usize, given: usize },
}

/// One end of a paired in-memory loopback. Two ends share a pair of
/// packet queues — each side's `write` lands at the other side's next
/// `try_read` — so a test or single-process flow can drive two engine
/// instances against each other without a real transport.
///
/// Single-threaded by construction (uses `Rc<RefCell<…>>` so callers
/// don't take on `Send + Sync` overhead they don't need). A
/// cross-thread variant using `Arc<Mutex<…>>` would be a drop-in
/// replacement if we ever need it.
///
/// Declared shape on this impl:
///
/// | Property      | Value                                      |
/// |---------------|--------------------------------------------|
/// | `capabilities` | receives, transmits — no forwards, no repeats |
/// | `mode`        | [`InterfaceMode::PointToPoint`]            |
/// | `medium_kind` | [`MediumKind::Loopback`]                   |
/// | `state`       | [`InterfaceState::Connected`] (constant)   |
///
/// The hard-coded defaults match the most common test use case: a
/// transparent endpoint pair, not a routing relay. A future
/// configurable variant could expose these as construction parameters
/// if a test needs to declare different capabilities or modes.
pub struct LoopbackInterface {
    id: InterfaceId,
    inbound: Rc<RefCell<VecDeque<Vec<u8>>>>,
    outbound: Rc<RefCell<VecDeque<Vec<u8>>>>,
}

impl LoopbackInterface {
    /// Build a connected pair of interfaces with the supplied
    /// identities. Each end's `write` lands at the other end's next
    /// `try_read`. Both ends are returned in `InterfaceState::Connected`.
    pub fn pair(left_id: InterfaceId, right_id: InterfaceId) -> (Self, Self) {
        let left_to_right = Rc::new(RefCell::new(VecDeque::new()));
        let right_to_left = Rc::new(RefCell::new(VecDeque::new()));
        let left = LoopbackInterface {
            id: left_id,
            inbound: right_to_left.clone(),
            outbound: left_to_right.clone(),
        };
        let right = LoopbackInterface {
            id: right_id,
            inbound: left_to_right,
            outbound: right_to_left,
        };
        (left, right)
    }
}

impl Interface for LoopbackInterface {
    type Error = LoopbackError;

    fn id(&self) -> InterfaceId {
        self.id
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            receives: true,
            transmits: true,
            forwards: false,
            repeats: false,
        }
    }

    fn mode(&self) -> InterfaceMode {
        InterfaceMode::PointToPoint
    }

    fn medium_kind(&self) -> MediumKind {
        MediumKind::Loopback
    }

    fn state(&self) -> InterfaceState {
        // In-process pair: both halves exist immediately on
        // construction and have no failure mode. We report
        // `Connected` unconditionally. A configurable variant could
        // model intentional `Disconnected` transitions later (useful
        // for hot-reload testing).
        InterfaceState::Connected
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        let mut queue = self.inbound.borrow_mut();
        let Some(packet) = queue.pop_front() else {
            return Ok(None);
        };
        if packet.len() > buf.len() {
            // Packet is consumed (popped). The caller should size
            // future buffers at least to engine MTU; we surface the
            // mismatch rather than silently truncate.
            return Err(LoopbackError::BufferTooSmall {
                needed: packet.len(),
                given: buf.len(),
            });
        }
        let n = packet.len();
        buf[..n].copy_from_slice(&packet);
        Ok(Some(n))
    }

    fn write(&mut self, packet: &[u8]) -> Result<(), Self::Error> {
        self.outbound.borrow_mut().push_back(packet.to_vec());
        Ok(())
    }
}

impl PointToPointInterface for LoopbackInterface {}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    #[test]
    fn paired_loopback_round_trips_packets_in_order() {
        let (mut left, mut right) = LoopbackInterface::pair(id(0x11), id(0x22));
        left.write(&[1, 2, 3]).unwrap();
        left.write(&[4, 5]).unwrap();

        let mut buf = [0u8; 64];
        assert_eq!(right.try_read(&mut buf).unwrap(), Some(3));
        assert_eq!(&buf[..3], &[1, 2, 3]);
        assert_eq!(right.try_read(&mut buf).unwrap(), Some(2));
        assert_eq!(&buf[..2], &[4, 5]);
        assert_eq!(right.try_read(&mut buf).unwrap(), None);
    }

    #[test]
    fn each_end_has_its_own_inbound_queue() {
        // Writes from one end do NOT appear at the same end's read —
        // only at the paired end's.
        let (mut left, mut right) = LoopbackInterface::pair(id(0x11), id(0x22));
        left.write(&[0xAA]).unwrap();

        let mut buf = [0u8; 8];
        assert_eq!(left.try_read(&mut buf).unwrap(), None);
        assert_eq!(right.try_read(&mut buf).unwrap(), Some(1));
        assert_eq!(buf[0], 0xAA);
    }

    #[test]
    fn declared_shape_matches_loopback_expectations() {
        let (left, right) = LoopbackInterface::pair(id(0x11), id(0x22));

        // Identities round-trip exactly as supplied.
        assert_eq!(left.id(), id(0x11));
        assert_eq!(right.id(), id(0x22));

        // Both halves declare the same shape.
        for end in [&left, &right] {
            assert_eq!(end.medium_kind(), MediumKind::Loopback);
            assert_eq!(end.mode(), InterfaceMode::PointToPoint);
            assert_eq!(end.state(), InterfaceState::Connected);
            assert_eq!(end.parent_interface(), None);
            assert_eq!(
                end.capabilities(),
                Capabilities {
                    receives: true,
                    transmits: true,
                    forwards: false,
                    repeats: false,
                }
            );
        }
    }

    #[test]
    fn oversize_packet_returns_buffer_too_small_and_consumes_the_packet() {
        let (mut left, mut right) = LoopbackInterface::pair(id(0x11), id(0x22));
        left.write(&[0xCD; 32]).unwrap();
        left.write(&[0xEF]).unwrap();

        let mut tiny_buf = [0u8; 4];
        assert_eq!(
            right.try_read(&mut tiny_buf),
            Err(LoopbackError::BufferTooSmall {
                needed: 32,
                given: 4,
            })
        );

        // The oversize packet was consumed; the next read returns the
        // next queued packet, which fits.
        assert_eq!(right.try_read(&mut tiny_buf).unwrap(), Some(1));
        assert_eq!(tiny_buf[0], 0xEF);
    }
}
