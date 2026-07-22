use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Sender;
use embassy_sync::signal::Signal;

use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::reactor::driver::{EmbassyGrantConsumer, EmbassyGrantProducer, InterfaceLifecycle};
use crate::reactor::grant::{FrameTarget, GrantConsumer, GrantProducer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundDeliveryError {
    FrameTooLarge { len: usize, capacity: usize },
    LaneFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundFrameError {
    FrameTooLarge { len: usize, capacity: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundFrame<const OUTBOUND_CAPACITY: usize> {
    target: FrameTarget,
    bytes: [u8; OUTBOUND_CAPACITY],
    len: usize,
}

impl<const OUTBOUND_CAPACITY: usize> OutboundFrame<OUTBOUND_CAPACITY> {
    #[must_use]
    pub fn target(&self) -> FrameTarget {
        self.target
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }
}

pub(super) struct FleetWire<M: RawMutex + 'static, const FRAME: usize, const NOTIFY: usize> {
    pub(super) inbound: EmbassyGrantProducer<'static, M, FRAME>,
    pub(super) outbound: EmbassyGrantConsumer<'static, M, FRAME>,
    pub(super) notify: Sender<'static, M, InterfaceId, NOTIFY>,
    pub(super) outbound_wake: &'static Signal<M, ()>,
}

pub struct Fleet<
    M: RawMutex + 'static,
    const FRAME: usize,
    const OUTBOUND_CAPACITY: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
> {
    wire: FleetWire<M, FRAME, NOTIFY>,
    lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
}

impl<
        M: RawMutex + 'static,
        const FRAME: usize,
        const OUTBOUND_CAPACITY: usize,
        const NOTIFY: usize,
        const LIFECYCLE: usize,
    > Fleet<M, FRAME, OUTBOUND_CAPACITY, NOTIFY, LIFECYCLE>
{
    #[must_use]
    pub(super) fn new(
        wire: FleetWire<M, FRAME, NOTIFY>,
        lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
    ) -> Self {
        const { assert!(OUTBOUND_CAPACITY <= FRAME) };
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

    pub fn try_deliver_inbound(
        &mut self,
        child: InterfaceId,
        bytes: &[u8],
    ) -> Result<(), InboundDeliveryError> {
        if bytes.len() > FRAME {
            return Err(InboundDeliveryError::FrameTooLarge {
                len: bytes.len(),
                capacity: FRAME,
            });
        }
        let grant = self
            .wire
            .inbound
            .try_grant()
            .ok_or(InboundDeliveryError::LaneFull)?;
        grant.fill_for(child, bytes);
        self.wire.inbound.commit();
        let _ = self.wire.notify.try_send(child);
        Ok(())
    }

    pub async fn next_outbound(
        &mut self,
    ) -> Result<OutboundFrame<OUTBOUND_CAPACITY>, OutboundFrameError> {
        self.wire.outbound.release();
        let slot = self.wire.outbound.peek().await;
        let target = slot.target;
        let len = slot.len;
        if len > OUTBOUND_CAPACITY {
            self.wire.outbound.release();
            return Err(OutboundFrameError::FrameTooLarge {
                len,
                capacity: OUTBOUND_CAPACITY,
            });
        }
        let mut bytes = [0; OUTBOUND_CAPACITY];
        bytes[..len].copy_from_slice(slot.frame());
        self.wire.outbound.release();
        Ok(OutboundFrame { target, bytes, len })
    }

    /// Waits for a shared-lane commit; drain with [`try_next_outbound`](Self::try_next_outbound) after waking.
    pub async fn outbound_ready(&self) {
        self.wire.outbound_wake.wait().await;
    }

    pub fn try_next_outbound(
        &mut self,
    ) -> Result<Option<OutboundFrame<OUTBOUND_CAPACITY>>, OutboundFrameError> {
        let Some(slot) = self.wire.outbound.try_peek() else {
            return Ok(None);
        };
        let target = slot.target;
        let len = slot.len;
        if len > OUTBOUND_CAPACITY {
            self.wire.outbound.release();
            return Err(OutboundFrameError::FrameTooLarge {
                len,
                capacity: OUTBOUND_CAPACITY,
            });
        }
        let mut bytes = [0; OUTBOUND_CAPACITY];
        bytes[..len].copy_from_slice(slot.frame());
        self.wire.outbound.release();
        Ok(Some(OutboundFrame { target, bytes, len }))
    }
}

#[cfg(test)]
mod tests;
