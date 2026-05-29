use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::interfaces::loopback::LoopbackError;
use crate::interfaces::{
    Capabilities, Interface, InterfaceId, InterfaceMode, InterfaceState, MediumKind,
    PointToPointInterface,
};

/// Cross-thread paired loopback. Same role as
/// [`LoopbackInterface`](crate::interfaces::LoopbackInterface) but uses
/// `Arc<Mutex<…>>` instead of `Rc<RefCell<…>>`, so the two halves can
/// safely be sent to and driven from different threads (e.g., one half
/// per `tokio` task, or one per OS thread for a stress test). Pays
/// locking overhead for the privilege.
///
/// Declared shape matches `LoopbackInterface` exactly — same
/// capabilities, same mode, same medium kind, same constant
/// `Connected` state.
///
/// Mutex poisoning is treated as fatal: if a thread holding a queue
/// lock panics, the loopback pair's invariants are gone and the test
/// is already broken — we propagate the panic rather than try to
/// recover.
pub struct ThreadedLoopback {
    id: InterfaceId,
    inbound: Arc<Mutex<VecDeque<Vec<u8>>>>,
    outbound: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl ThreadedLoopback {
    /// Build a connected, thread-safe pair. Each end is `Send + Sync`
    /// and can be moved to a different thread.
    pub fn pair(left_id: InterfaceId, right_id: InterfaceId) -> (Self, Self) {
        let left_to_right = Arc::new(Mutex::new(VecDeque::new()));
        let right_to_left = Arc::new(Mutex::new(VecDeque::new()));
        let left = ThreadedLoopback {
            id: left_id,
            inbound: right_to_left.clone(),
            outbound: left_to_right.clone(),
        };
        let right = ThreadedLoopback {
            id: right_id,
            inbound: left_to_right,
            outbound: right_to_left,
        };
        (left, right)
    }
}

impl Interface for ThreadedLoopback {
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
}

impl PointToPointInterface for ThreadedLoopback {
    type Error = LoopbackError;

    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        let mut queue = self.inbound.lock().expect("loopback queue lock poisoned");
        let Some(packet) = queue.pop_front() else {
            return Ok(None);
        };
        if packet.len() > buf.len() {
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
        self.outbound
            .lock()
            .expect("loopback queue lock poisoned")
            .push_back(packet.to_vec());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn id(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    #[test]
    fn paired_threaded_loopback_round_trips_in_order_single_thread() {
        // Same round-trip property as LoopbackInterface; verifies the
        // Mutex-backed variant behaves identically when driven from
        // one thread.
        let (mut left, mut right) = ThreadedLoopback::pair(id(0xAA), id(0xBB));
        left.write(&[1, 2, 3]).unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(right.try_read(&mut buf).unwrap(), Some(3));
        assert_eq!(&buf[..3], &[1, 2, 3]);
    }

    #[test]
    fn the_two_halves_drive_across_threads() {
        // The point of this variant: each half goes to a different
        // thread, and packets still flow.
        let (mut left, mut right) = ThreadedLoopback::pair(id(0xAA), id(0xBB));

        let writer = thread::spawn(move || {
            for n in 0u8..5 {
                left.write(&[n, n.wrapping_mul(2)]).unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });

        let mut received = std::vec::Vec::new();
        let mut buf = [0u8; 16];
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while received.len() < 5 && std::time::Instant::now() < deadline {
            if let Some(n) = right.try_read(&mut buf).unwrap() {
                received.push(buf[..n].to_vec());
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }

        writer.join().expect("writer thread");
        assert_eq!(received.len(), 5);
        for (i, packet) in received.iter().enumerate() {
            let n = i as u8;
            assert_eq!(packet, &[n, n.wrapping_mul(2)]);
        }
    }

    #[test]
    fn declared_shape_matches_loopback_expectations() {
        let (left, right) = ThreadedLoopback::pair(id(0xAA), id(0xBB));
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

    #[test]
    fn each_end_is_send_and_sync() {
        // Compile-time assertion: ThreadedLoopback satisfies the
        // Send + Sync bounds that drove this variant's existence.
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<ThreadedLoopback>();
        assert_sync::<ThreadedLoopback>();
    }
}
