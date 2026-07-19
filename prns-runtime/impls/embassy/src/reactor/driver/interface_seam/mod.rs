use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Sender;

use crate::interfaces::{FrameSink, InterfaceId, PacketPhyStats};
use crate::reactor::grant::{FrameTarget, GrantConsumer, GrantProducer};
use crate::reactor::interface_seam::InterfaceSeam;

use super::{EmbassyGrantConsumer, EmbassyGrantProducer};

pub struct EmbassyInterfaceSeam<'a, M: RawMutex, const NOTIFY: usize, const SLOT: usize> {
    id: InterfaceId,
    inbound: EmbassyGrantProducer<'a, M, SLOT>,
    notify: Sender<'a, M, InterfaceId, NOTIFY>,
    outbound: EmbassyGrantConsumer<'a, M, SLOT>,
    fill_entropy: fn(&mut [u8]),
}

impl<'a, M: RawMutex, const NOTIFY: usize, const SLOT: usize>
    EmbassyInterfaceSeam<'a, M, NOTIFY, SLOT>
{
    #[must_use]
    pub fn new(
        id: InterfaceId,
        inbound: EmbassyGrantProducer<'a, M, SLOT>,
        notify: Sender<'a, M, InterfaceId, NOTIFY>,
        outbound: EmbassyGrantConsumer<'a, M, SLOT>,
        fill_entropy: fn(&mut [u8]),
    ) -> Self {
        Self {
            id,
            inbound,
            notify,
            outbound,
            fill_entropy,
        }
    }
}

impl<M: RawMutex, const NOTIFY: usize, const SLOT: usize> InterfaceSeam
    for EmbassyInterfaceSeam<'_, M, NOTIFY, SLOT>
{
    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        (self.fill_entropy)(bytes);
    }

    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        let slot = self.inbound.grant().await;
        slot.target = FrameTarget::Direct(self.id);
        slot
    }

    async fn commit_inbound(&mut self) {
        let slot = self.inbound.grant().await;
        if slot.len == 0 {
            return;
        }
        self.inbound.commit();
        let _ = self.notify.try_send(self.id);
    }

    async fn next_inbound_with_phy(&mut self, frame: &[u8], packet_phy: PacketPhyStats) {
        let slot = self.inbound.grant().await;
        slot.clear();
        if slot.extend_from_slice(frame).is_err() {
            return;
        }
        slot.packet_phy = packet_phy;
        self.commit_inbound().await;
    }

    async fn next_outbound(&mut self) -> &[u8] {
        self.outbound.release();
        self.outbound.peek().await.frame()
    }
}

#[cfg(test)]
mod tests;
