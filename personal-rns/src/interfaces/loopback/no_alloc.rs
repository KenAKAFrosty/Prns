use core::cell::RefCell;

use heapless::{Deque, Vec};

use crate::interfaces::{
    Capabilities, Interface, InterfaceId, InterfaceMode, InterfaceState, MediumKind,
    PointToPointInterface,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoAllocLoopbackError {
    BufferTooSmall { needed: usize, given: usize },
    PacketTooLarge { packet_len: usize, max: usize },
    QueueFull,
}

/// Type alias for the shared queue both halves of a no-alloc loopback
/// borrow. The caller (test fixture or sim harness) constructs two of
/// these on the stack - or in static storage - and passes references
/// into [`NoAllocLoopback::pair`].
pub type NoAllocLoopbackQueue<const MAX_PACKET_LEN: usize, const QUEUE_CAP: usize> =
    RefCell<Deque<Vec<u8, MAX_PACKET_LEN>, QUEUE_CAP>>;

/// Pure-no_std paired loopback. Same role as
/// [`LoopbackInterface`](crate::interfaces::LoopbackInterface) and
/// [`ThreadedLoopback`](crate::interfaces::ThreadedLoopback) but with
/// no allocator: queues live in caller-owned, fixed-capacity heapless
/// storage that both halves borrow from for their lifetime.
///
/// Awkward but honest API trade-off: without `Rc` or `Arc` there's no
/// way to share interior storage that outlives the constructor, so the
/// caller carries the queues on its stack (or static) and the
/// interfaces borrow. This is exactly the cost of staying alloc-free.
///
/// Declared shape matches the other loopback variants exactly: same
/// capabilities, mode, medium, and constant `Connected` state.
pub struct NoAllocLoopback<'a, const MAX_PACKET_LEN: usize, const QUEUE_CAP: usize> {
    id: InterfaceId,
    inbound: &'a NoAllocLoopbackQueue<MAX_PACKET_LEN, QUEUE_CAP>,
    outbound: &'a NoAllocLoopbackQueue<MAX_PACKET_LEN, QUEUE_CAP>,
}

impl<'a, const MAX_PACKET_LEN: usize, const QUEUE_CAP: usize>
    NoAllocLoopback<'a, MAX_PACKET_LEN, QUEUE_CAP>
{
    pub fn pair(
        left_id: InterfaceId,
        right_id: InterfaceId,
        left_to_right: &'a NoAllocLoopbackQueue<MAX_PACKET_LEN, QUEUE_CAP>,
        right_to_left: &'a NoAllocLoopbackQueue<MAX_PACKET_LEN, QUEUE_CAP>,
    ) -> (Self, Self) {
        let left = NoAllocLoopback {
            id: left_id,
            inbound: right_to_left,
            outbound: left_to_right,
        };
        let right = NoAllocLoopback {
            id: right_id,
            inbound: left_to_right,
            outbound: right_to_left,
        };
        (left, right)
    }
}

impl<'a, const MAX_PACKET_LEN: usize, const QUEUE_CAP: usize> Interface
    for NoAllocLoopback<'a, MAX_PACKET_LEN, QUEUE_CAP>
{
    type Error = NoAllocLoopbackError;

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
        InterfaceState::Connected
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        let mut queue = self.inbound.borrow_mut();
        let Some(packet) = queue.pop_front() else {
            return Ok(None);
        };
        if packet.len() > buf.len() {
            return Err(NoAllocLoopbackError::BufferTooSmall {
                needed: packet.len(),
                given: buf.len(),
            });
        }
        let n = packet.len();
        buf[..n].copy_from_slice(&packet);
        Ok(Some(n))
    }

    fn write(&mut self, packet: &[u8]) -> Result<(), Self::Error> {
        let packet_vec: Vec<u8, MAX_PACKET_LEN> =
            Vec::from_slice(packet).map_err(|()| NoAllocLoopbackError::PacketTooLarge {
                packet_len: packet.len(),
                max: MAX_PACKET_LEN,
            })?;
        self.outbound
            .borrow_mut()
            .push_back(packet_vec)
            .map_err(|_| NoAllocLoopbackError::QueueFull)?;
        Ok(())
    }
}

impl<'a, const MAX_PACKET_LEN: usize, const QUEUE_CAP: usize> PointToPointInterface
    for NoAllocLoopback<'a, MAX_PACKET_LEN, QUEUE_CAP>
{
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    #[test]
    fn paired_no_alloc_loopback_round_trips_packets_in_order() {
        let l2r: NoAllocLoopbackQueue<64, 4> = RefCell::new(Deque::new());
        let r2l: NoAllocLoopbackQueue<64, 4> = RefCell::new(Deque::new());
        let (mut left, mut right) = NoAllocLoopback::<64, 4>::pair(id(0xAA), id(0xBB), &l2r, &r2l);

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
        let l2r: NoAllocLoopbackQueue<8, 4> = RefCell::new(Deque::new());
        let r2l: NoAllocLoopbackQueue<8, 4> = RefCell::new(Deque::new());
        let (mut left, mut right) = NoAllocLoopback::<8, 4>::pair(id(0xAA), id(0xBB), &l2r, &r2l);

        left.write(&[0xCC]).unwrap();

        let mut buf = [0u8; 8];
        assert_eq!(left.try_read(&mut buf).unwrap(), None);
        assert_eq!(right.try_read(&mut buf).unwrap(), Some(1));
        assert_eq!(buf[0], 0xCC);
    }

    #[test]
    fn writing_past_max_packet_len_returns_packet_too_large() {
        let l2r: NoAllocLoopbackQueue<8, 4> = RefCell::new(Deque::new());
        let r2l: NoAllocLoopbackQueue<8, 4> = RefCell::new(Deque::new());
        let (mut left, _right) = NoAllocLoopback::<8, 4>::pair(id(0xAA), id(0xBB), &l2r, &r2l);

        assert_eq!(
            left.write(&[0xCD; 16]),
            Err(NoAllocLoopbackError::PacketTooLarge {
                packet_len: 16,
                max: 8,
            })
        );
    }

    #[test]
    fn writing_past_queue_capacity_returns_queue_full() {
        let l2r: NoAllocLoopbackQueue<32, 2> = RefCell::new(Deque::new());
        let r2l: NoAllocLoopbackQueue<32, 2> = RefCell::new(Deque::new());
        let (mut left, _right) = NoAllocLoopback::<32, 2>::pair(id(0xAA), id(0xBB), &l2r, &r2l);

        left.write(&[1]).unwrap();
        left.write(&[2]).unwrap();
        assert_eq!(left.write(&[3]), Err(NoAllocLoopbackError::QueueFull));
    }

    #[test]
    fn oversize_packet_returns_buffer_too_small_and_consumes_the_packet() {
        let l2r: NoAllocLoopbackQueue<64, 4> = RefCell::new(Deque::new());
        let r2l: NoAllocLoopbackQueue<64, 4> = RefCell::new(Deque::new());
        let (mut left, mut right) = NoAllocLoopback::<64, 4>::pair(id(0xAA), id(0xBB), &l2r, &r2l);

        left.write(&[0xCD; 32]).unwrap();
        left.write(&[0xEF]).unwrap();

        let mut tiny_buf = [0u8; 4];
        assert_eq!(
            right.try_read(&mut tiny_buf),
            Err(NoAllocLoopbackError::BufferTooSmall {
                needed: 32,
                given: 4,
            })
        );

        // Oversize packet was consumed; next read returns the next
        // queued packet, which fits.
        assert_eq!(right.try_read(&mut tiny_buf).unwrap(), Some(1));
        assert_eq!(tiny_buf[0], 0xEF);
    }

    #[test]
    fn declared_shape_matches_loopback_expectations() {
        let l2r: NoAllocLoopbackQueue<32, 4> = RefCell::new(Deque::new());
        let r2l: NoAllocLoopbackQueue<32, 4> = RefCell::new(Deque::new());
        let (left, right) = NoAllocLoopback::<32, 4>::pair(id(0xAA), id(0xBB), &l2r, &r2l);

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
        assert_eq!(left.id(), id(0xAA));
        assert_eq!(right.id(), id(0xBB));
    }
}
