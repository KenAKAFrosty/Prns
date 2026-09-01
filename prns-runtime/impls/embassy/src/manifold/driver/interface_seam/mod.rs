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
    inbound: EmbassyGrantProducer<'a, M, FRAME>,
    notify: Sender<'a, M, InterfaceId, NOTIFY>,
    outbound: EmbassyGrantConsumer<'a, M, FRAME>,
    entropy: EntropyHandle<M, S>,
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
    fn fill_random(&mut self, bytes: &mut [u8]) {
        self.entropy.fill_random(bytes);
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

    fn accept_outbound_custody(&mut self) {
        self.outbound.release();
    }

    fn complete_outbound(&mut self, _disposition: OutboundDisposition) {
        self.outbound.release();
    }
}

#[cfg(test)]
mod tests;
