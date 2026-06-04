use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::schedule::{RebroadcastQueue, ScheduledRebroadcast};
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapRebroadcastQueue {
    pending: Vec<ScheduledRebroadcast>,
}

impl RebroadcastQueue for HeapRebroadcastQueue {
    fn pending_count(&self) -> usize {
        self.pending.len()
    }
    fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
    ) {
        if let Some(existing) = self
            .pending
            .iter_mut()
            .find(|entry| entry.destination == destination)
        {
            existing.due_at = due_at;
            existing.source_interface = source_interface;
        } else {
            self.pending.push(ScheduledRebroadcast {
                destination,
                due_at,
                source_interface,
            });
        }
    }
    fn drain_due(&mut self, now: InstantMillis) -> usize {
        let before = self.pending.len();
        self.pending.retain(|entry| entry.due_at > now);
        before - self.pending.len()
    }
    fn earliest_due_at(&self) -> Option<InstantMillis> {
        self.pending.iter().map(|entry| entry.due_at).min()
    }
    fn as_slice(&self) -> &[ScheduledRebroadcast] {
        &self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }
    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    #[test]
    fn grows_past_a_fixed_cap_upserts_and_drains() {
        let mut pending = HeapRebroadcastQueue::default();
        for n in 0..200u8 {
            pending.schedule(dest(n), InstantMillis(100 + n as u64), iface(0xAA));
        }
        assert_eq!(pending.pending_count(), 200);
        assert_eq!(pending.earliest_due_at(), Some(InstantMillis(100)));

        pending.schedule(dest(0), InstantMillis(50), iface(0xBB));
        assert_eq!(pending.pending_count(), 200);
        assert_eq!(pending.earliest_due_at(), Some(InstantMillis(50)));

        assert_eq!(pending.drain_due(InstantMillis(10_000)), 200);
        assert_eq!(pending.pending_count(), 0);
    }
}
