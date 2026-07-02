use crate::interfaces::{AirtimeDutyCycle, AirtimeUtilization};
use crate::wire::BROADCAST_MTU;
use heapless::Deque;
use heapless::Vec as HeaplessVec;

pub trait DutyQueue: Default {
    fn push_back(&mut self, bytes: &[u8], airtime_us: u64);
    fn pop_front_with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<(u64, R)>;
    fn is_empty(&self) -> bool;
    fn is_full(&self) -> bool;
}

struct Queued<F> {
    airtime_us: u64,
    frame: F,
}

#[derive(Default)]
pub struct FixedDutyQueue<const FRAMES: usize> {
    entries: Deque<Queued<HeaplessVec<u8, BROADCAST_MTU>>, FRAMES>,
}

impl<const FRAMES: usize> DutyQueue for FixedDutyQueue<FRAMES> {
    fn push_back(&mut self, bytes: &[u8], airtime_us: u64) {
        let mut frame = HeaplessVec::new();
        if frame.extend_from_slice(bytes).is_err() {
            return;
        }
        let _ = self.entries.push_back(Queued { airtime_us, frame });
    }

    fn pop_front_with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<(u64, R)> {
        let entry = self.entries.pop_front()?;
        Some((entry.airtime_us, f(entry.frame.as_slice())))
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn is_full(&self) -> bool {
        self.entries.is_full()
    }
}

#[cfg(feature = "alloc")]
pub use heap::HeapDutyQueue;

#[cfg(feature = "alloc")]
mod heap {
    use super::{DutyQueue, Queued};
    use alloc::collections::VecDeque;
    use alloc::vec::Vec;

    #[derive(Default)]
    pub struct HeapDutyQueue {
        entries: VecDeque<Queued<Vec<u8>>>,
    }

    impl DutyQueue for HeapDutyQueue {
        fn push_back(&mut self, bytes: &[u8], airtime_us: u64) {
            self.entries.push_back(Queued {
                airtime_us,
                frame: bytes.to_vec(),
            });
        }

        fn pop_front_with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<(u64, R)> {
            let entry = self.entries.pop_front()?;
            Some((entry.airtime_us, f(&entry.frame)))
        }

        fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }

        fn is_full(&self) -> bool {
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DutyVerdict {
    Transmit,
    Held,
}

pub struct DutyGate<Q: DutyQueue> {
    queue: Q,
    queued_airtime_us: u64,
    dropped: u64,
}

impl<Q: DutyQueue> Default for DutyGate<Q> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Q: DutyQueue> DutyGate<Q> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: Q::default(),
            queued_airtime_us: 0,
            dropped: 0,
        }
    }

    pub fn offer(
        &mut self,
        wire: &[u8],
        airtime_us: u64,
        utilization: AirtimeUtilization,
        duty: &AirtimeDutyCycle,
    ) -> DutyVerdict {
        if self.queue.is_empty() && !duty.exceeded_by(utilization) {
            return DutyVerdict::Transmit;
        }
        self.enqueue(wire, airtime_us, duty);
        DutyVerdict::Held
    }

    pub fn release_ready(
        &mut self,
        utilization: AirtimeUtilization,
        duty: &AirtimeDutyCycle,
        send: impl FnOnce(&[u8]),
    ) -> bool {
        if duty.exceeded_by(utilization) {
            return false;
        }
        let Some((airtime_us, ())) = self.queue.pop_front_with(send) else {
            return false;
        };
        self.queued_airtime_us = self.queued_airtime_us.saturating_sub(airtime_us);
        true
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    #[must_use]
    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }

    fn enqueue(&mut self, wire: &[u8], airtime_us: u64, duty: &AirtimeDutyCycle) {
        let budget_us = u64::from(duty.max_queued_airtime_ms).saturating_mul(1_000);
        if airtime_us > budget_us {
            self.dropped += 1;
            return;
        }
        while self.queued_airtime_us.saturating_add(airtime_us) > budget_us || self.queue.is_full()
        {
            let Some((evicted_airtime_us, ())) = self.queue.pop_front_with(|_| ()) else {
                break;
            };
            self.queued_airtime_us = self.queued_airtime_us.saturating_sub(evicted_airtime_us);
            self.dropped += 1;
        }
        self.queue.push_back(wire, airtime_us);
        self.queued_airtime_us = self.queued_airtime_us.saturating_add(airtime_us);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUTY: AirtimeDutyCycle = AirtimeDutyCycle {
        limit_short_per_mille: Some(100),
        limit_long_per_mille: None,
        max_queued_airtime_ms: 2_000,
    };

    fn idle() -> AirtimeUtilization {
        AirtimeUtilization {
            short_per_mille: 0,
            long_per_mille: 0,
        }
    }

    fn saturated() -> AirtimeUtilization {
        AirtimeUtilization {
            short_per_mille: 100,
            long_per_mille: 0,
        }
    }

    fn transmits_under_limit_holds_over_it<Q: DutyQueue>() {
        let mut gate: DutyGate<Q> = DutyGate::new();
        assert_eq!(
            gate.offer(&[1, 2, 3], 50_000, idle(), &DUTY),
            DutyVerdict::Transmit
        );
        assert!(gate.is_empty());

        assert_eq!(
            gate.offer(&[7; 10], 50_000, saturated(), &DUTY),
            DutyVerdict::Held
        );

        let mut sent = std::vec::Vec::new();
        assert!(!gate.release_ready(saturated(), &DUTY, |bytes| sent.push(bytes.to_vec())));
        assert!(sent.is_empty(), "still over the limit");

        assert!(gate.release_ready(idle(), &DUTY, |bytes| sent.push(bytes.to_vec())));
        assert_eq!(sent, std::vec![std::vec![7u8; 10]]);
        assert!(gate.is_empty());
        assert_eq!(gate.dropped_count(), 0);
    }

    fn budgets_the_queue_in_airtime<Q: DutyQueue>() {
        let mut gate: DutyGate<Q> = DutyGate::new();
        gate.offer(&[1], 900_000, saturated(), &DUTY);
        gate.offer(&[2], 900_000, saturated(), &DUTY);
        assert_eq!(gate.dropped_count(), 0, "1.8s of the 2s budget queued");

        gate.offer(&[3], 900_000, saturated(), &DUTY);
        assert_eq!(
            gate.dropped_count(),
            1,
            "the oldest fell out to fit the newcomer"
        );

        let mut sent = std::vec::Vec::new();
        while gate.release_ready(idle(), &DUTY, |bytes| sent.push(bytes.to_vec())) {}
        assert_eq!(
            sent,
            std::vec![std::vec![2u8], std::vec![3u8]],
            "FIFO order among the survivors",
        );

        gate.offer(&[9; 5], 2_500_000, saturated(), &DUTY);
        assert!(
            gate.is_empty(),
            "a frame bigger than the whole budget drops"
        );
        assert_eq!(gate.dropped_count(), 2);
    }

    #[test]
    fn the_fixed_queue_transmits_under_limit_holds_over_it() {
        transmits_under_limit_holds_over_it::<FixedDutyQueue<8>>();
    }

    #[test]
    fn the_fixed_queue_budgets_in_airtime() {
        budgets_the_queue_in_airtime::<FixedDutyQueue<8>>();
    }

    #[test]
    fn the_fixed_frame_capacity_is_only_the_allocation_ceiling() {
        let mut gate: DutyGate<FixedDutyQueue<2>> = DutyGate::new();
        gate.offer(&[1], 100_000, saturated(), &DUTY);
        gate.offer(&[2], 100_000, saturated(), &DUTY);
        gate.offer(&[3], 100_000, saturated(), &DUTY);
        assert_eq!(
            gate.dropped_count(),
            1,
            "well under the airtime budget, the ring itself still bounds memory",
        );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn the_heap_queue_transmits_under_limit_holds_over_it() {
        transmits_under_limit_holds_over_it::<HeapDutyQueue>();
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn the_heap_queue_budgets_in_airtime() {
        budgets_the_queue_in_airtime::<HeapDutyQueue>();
    }
}
