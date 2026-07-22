use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Sender;
use embassy_sync::signal::Signal;
use heapless::Vec as HeaplessVec;

use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::reactor::driver::{EmbassyGrantConsumer, EmbassyGrantProducer, InterfaceLifecycle};
use crate::reactor::grant::{FrameTarget, GrantConsumer, GrantProducer};

pub(super) struct FleetWire<M: RawMutex + 'static, const FRAME: usize, const NOTIFY: usize> {
    pub(super) inbound: EmbassyGrantProducer<'static, M, FRAME>,
    pub(super) outbound: EmbassyGrantConsumer<'static, M, FRAME>,
    pub(super) notify: Sender<'static, M, InterfaceId, NOTIFY>,
    pub(super) outbound_wake: &'static Signal<M, ()>,
}

pub struct Fleet<
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
> {
    wire: FleetWire<M, FRAME, NOTIFY>,
    lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
}

impl<M: RawMutex + 'static, const FRAME: usize, const NOTIFY: usize, const LIFECYCLE: usize>
    Fleet<M, FRAME, NOTIFY, LIFECYCLE>
{
    #[must_use]
    pub(super) fn new(
        wire: FleetWire<M, FRAME, NOTIFY>,
        lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
    ) -> Self {
        Self { wire, lifecycle }
    }

    pub async fn register_member(&self, descriptor: InterfaceDescriptor) {
        self.lifecycle
            .send(InterfaceLifecycle::Add { descriptor })
            .await;
    }

    pub async fn deregister_member(&self, id: InterfaceId) {
        self.lifecycle.send(InterfaceLifecycle::Remove { id }).await;
    }

    /// Returns `false` when the shared inbound lane is full.
    pub fn deliver_inbound(&mut self, child: InterfaceId, bytes: &[u8]) -> bool {
        let Some(grant) = self.wire.inbound.try_grant() else {
            return false;
        };
        grant.fill_for(child, bytes);
        self.wire.inbound.commit();
        let _ = self.wire.notify.try_send(child);
        true
    }

    /// Copies and releases the next outbound slot so callers can hold the frame across `select` branches.
    pub async fn next_outbound<const OUT: usize>(&mut self) -> (FrameTarget, HeaplessVec<u8, OUT>) {
        self.wire.outbound.release();
        let slot = self.wire.outbound.peek().await;
        let target = slot.target;
        let mut bytes: HeaplessVec<u8, OUT> = HeaplessVec::new();
        let _ = bytes.extend_from_slice(slot.frame());
        self.wire.outbound.release();
        (target, bytes)
    }

    /// Waits for a shared-lane commit; drain with [`try_next_outbound`](Self::try_next_outbound) after waking.
    pub async fn outbound_ready(&self) {
        self.wire.outbound_wake.wait().await;
    }

    pub fn try_next_outbound<const OUT: usize>(
        &mut self,
    ) -> Option<(FrameTarget, HeaplessVec<u8, OUT>)> {
        let slot = self.wire.outbound.try_peek()?;
        let target = slot.target;
        let mut bytes: HeaplessVec<u8, OUT> = HeaplessVec::new();
        let _ = bytes.extend_from_slice(slot.frame());
        self.wire.outbound.release();
        Some((target, bytes))
    }
}

#[cfg(test)]
mod tests;
