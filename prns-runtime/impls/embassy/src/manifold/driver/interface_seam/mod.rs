use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Sender;
use prns_core::entropy::EntropySource;

use crate::interfaces::{FrameSink, InterfaceId, PacketPhyStats};
use crate::manifold::grant::{FrameTarget, GrantConsumer, GrantProducer};
use crate::manifold::interface_seam::{InterfaceSeam, OutboundDisposition};
use crate::runtime::EntropyHandle;

use super::{EmbassyGrantConsumer, EmbassyGrantProducer};

/// The Embassy engine seam accepts only a handle to the authoritative runtime stream.
///
/// ```compile_fail
/// use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
/// use prns_runtime_embassy::manifold::driver::EmbassyInterfaceSeam;
///
/// let callback = |output: &mut [u8]| output.fill(0x42);
/// let _ = EmbassyInterfaceSeam::<CriticalSectionRawMutex, _, 1, 512>::new(
///     panic!(),
///     panic!(),
///     panic!(),
///     panic!(),
///     callback,
/// );
/// ```
pub struct EmbassyInterfaceSeam<
    'a,
    M: RawMutex + 'static,
    S: EntropySource + 'static,
    const NOTIFY: usize,
    const FRAME: usize,
> {
    id: InterfaceId,
    channel: ChannelBinding,
    channel_change_drops: u32,
    inbound: EmbassyGrantProducer<'a, M, FRAME>,
    notify: Sender<'a, M, InterfaceId, NOTIFY>,
    outbound: EmbassyGrantConsumer<'a, M, FRAME>,
    entropy: EntropyHandle<M, S>,
}

#[derive(Clone, Copy)]
enum ChannelBinding {
    Original,
    Published,
    Retained,
}

impl<'a, M, S, const NOTIFY: usize, const FRAME: usize>
    EmbassyInterfaceSeam<'a, M, S, NOTIFY, FRAME>
where
    M: RawMutex + Sync + 'static,
    S: EntropySource + Send + 'static,
{
    #[must_use]
    pub fn new(
        id: InterfaceId,
        inbound: EmbassyGrantProducer<'a, M, FRAME>,
        notify: Sender<'a, M, InterfaceId, NOTIFY>,
        outbound: EmbassyGrantConsumer<'a, M, FRAME>,
        entropy: EntropyHandle<M, S>,
    ) -> Self {
        Self {
            id,
            channel: ChannelBinding::Original,
            channel_change_drops: 0,
            inbound,
            notify,
            outbound,
            entropy,
        }
    }
}

impl<M, S, const NOTIFY: usize, const FRAME: usize> InterfaceSeam
    for EmbassyInterfaceSeam<'_, M, S, NOTIFY, FRAME>
where
    M: RawMutex + Sync + 'static,
    S: EntropySource + Send + 'static,
{
    fn set_channel_id(&mut self, id: InterfaceId) {
        if self.id == id {
            if matches!(self.channel, ChannelBinding::Original) {
                self.channel = ChannelBinding::Published;
            }
            return;
        }
        self.id = id;
        self.channel = ChannelBinding::Published;
        // Publication is acknowledged before this boundary, so old-channel
        // entries form a prefix. Drain that prefix now to prevent A→B→A reuse.
        while let Some(frame) = self.outbound.try_peek() {
            if frame.target == FrameTarget::Direct(id) {
                self.channel = ChannelBinding::Retained;
                break;
            }
            self.complete_outbound(OutboundDisposition::Dropped(
                crate::manifold::interface_seam::OutboundDropReason::ChannelChanged,
            ));
        }
    }

    fn take_channel_change_drops(&mut self) -> u32 {
        core::mem::take(&mut self.channel_change_drops)
    }

    fn fill_random(&mut self, bytes: &mut [u8]) {
        self.entropy.fill_random(bytes);
    }

    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        self.inbound.grant().await
    }

    async fn commit_inbound(&mut self) {
        let slot = self.inbound.grant().await;
        if slot.len == 0 {
            return;
        }
        slot.target = FrameTarget::Direct(self.id);
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
        match self.channel {
            ChannelBinding::Original | ChannelBinding::Published => self.outbound.release(),
            ChannelBinding::Retained => self.channel = ChannelBinding::Published,
        }
        loop {
            let target = self.outbound.peek().await.target;
            let accepts = match self.channel {
                ChannelBinding::Original => true,
                ChannelBinding::Published | ChannelBinding::Retained => {
                    target == FrameTarget::Direct(self.id)
                }
            };
            if accepts {
                return self.outbound.peek().await.frame();
            }
            self.complete_outbound(OutboundDisposition::Dropped(
                crate::manifold::interface_seam::OutboundDropReason::ChannelChanged,
            ));
        }
    }

    fn accept_outbound_custody(&mut self) {
        if matches!(self.channel, ChannelBinding::Retained) {
            self.channel = ChannelBinding::Published;
        }
        self.outbound.release();
    }

    fn complete_outbound(&mut self, disposition: OutboundDisposition) {
        if matches!(self.channel, ChannelBinding::Retained) {
            self.channel = ChannelBinding::Published;
        }
        if disposition
            == OutboundDisposition::Dropped(
                crate::manifold::interface_seam::OutboundDropReason::ChannelChanged,
            )
        {
            self.channel_change_drops = self.channel_change_drops.saturating_add(1);
        }
        self.outbound.release();
    }
}

#[cfg(test)]
mod tests;
