use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Sender;
use embassy_sync::signal::Signal;
use heapless::Vec as HeaplessVec;

use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::reactor::driver::{EmbassyGrantConsumer, EmbassyGrantProducer, InterfaceLifecycle};
use crate::reactor::grant::{FrameTarget, GrantConsumer, GrantProducer};

pub struct FleetWire<M: RawMutex + 'static, const SLOT: usize, const NOTIFY: usize> {
    pub inbound: EmbassyGrantProducer<'static, M, SLOT>,
    pub outbound: EmbassyGrantConsumer<'static, M, SLOT>,
    pub notify: Sender<'static, M, InterfaceId, NOTIFY>,
    pub outbound_wake: &'static Signal<M, ()>,
}

pub struct Fleet<
    M: RawMutex + 'static,
    const SLOT: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
> {
    wire: FleetWire<M, SLOT, NOTIFY>,
    lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
}

impl<M: RawMutex + 'static, const SLOT: usize, const NOTIFY: usize, const LIFECYCLE: usize>
    Fleet<M, SLOT, NOTIFY, LIFECYCLE>
{
    #[must_use]
    pub fn new(
        wire: FleetWire<M, SLOT, NOTIFY>,
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

    /// Funnel one inbound frame from peer `child` into the shared lane, tagged so the reactor ingests it as `child`'s, then announce the commit on the notify funnel. `false` if the lane is momentarily full (the frame drops, as a full lane does), so a slow reactor never stalls the medium read.
    pub fn deliver_inbound(&mut self, child: InterfaceId, bytes: &[u8]) -> bool {
        let Some(grant) = self.wire.inbound.try_grant() else {
            return false;
        };
        grant.fill_for(child, bytes);
        self.wire.inbound.commit();
        let _ = self.wire.notify.try_send(child);
        true
    }

    /// Park until the reactor grants an outbound frame, returning a copy plus its [`FrameTarget`]: the one peer it addresses, or the fan a fleet broadcast selects members by. The frame is copied out rather than borrowed, so the returned value owns nothing of the fleet (it can ride a `select` arm without a borrow clash), and the slot is released before returning, so the depth-1 lane refills at once and each frame is carried exactly once.
    pub async fn next_outbound<const OUT: usize>(&mut self) -> (FrameTarget, HeaplessVec<u8, OUT>) {
        self.wire.outbound.release();
        let slot = self.wire.outbound.peek().await;
        let target = slot.target;
        let mut bytes: HeaplessVec<u8, OUT> = HeaplessVec::new();
        let _ = bytes.extend_from_slice(slot.frame());
        self.wire.outbound.release();
        (target, bytes)
    }

    /// Park until the reactor commits an outbound frame onto this fleet's shared lane: the reactor signals every commit, rousing a waiting supervisor across the task boundary without depending on the lane's own consumer waker. On wake, drain with [`try_next_outbound`](Self::try_next_outbound) until `None`.
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
