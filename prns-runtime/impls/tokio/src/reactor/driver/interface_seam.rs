use tokio::sync::mpsc::UnboundedSender;

use crate::interfaces::{FrameSink, InterfaceId, InterfaceOriginKind, PacketPhyStats};
use crate::reactor::interface_seam::InterfaceSeam;

use super::{HostCommand, TokioGrantConsumer, TokioGrantProducer};

/// The tokio side of one interface's seam: `next_inbound` frames funnel into the reactor's one inbound stream (tagged with this interface's id), and `next_outbound` parks on this interface's own outbound queue until the reactor enqueues a frame for it.
pub struct TokioInterfaceSeam {
    id: InterfaceId,
    origin: InterfaceOriginKind,
    inbound: TokioGrantProducer,
    notify: UnboundedSender<InterfaceId>,
    outbound: TokioGrantConsumer,
    commands: Option<UnboundedSender<HostCommand>>,
}

impl TokioInterfaceSeam {
    #[must_use]
    pub fn new(
        id: InterfaceId,
        inbound: TokioGrantProducer,
        notify: UnboundedSender<InterfaceId>,
        outbound: TokioGrantConsumer,
    ) -> Self {
        Self {
            id,
            origin: InterfaceOriginKind::Configured,
            inbound,
            notify,
            outbound,
            commands: None,
        }
    }

    #[must_use]
    pub fn with_origin(mut self, origin: InterfaceOriginKind) -> Self {
        self.origin = origin;
        self
    }

    #[must_use]
    pub fn with_commands(mut self, commands: UnboundedSender<HostCommand>) -> Self {
        self.commands = Some(commands);
        self
    }
}

impl InterfaceSeam for TokioInterfaceSeam {
    fn interface_origin(&self) -> InterfaceOriginKind {
        self.origin
    }

    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        self.inbound.grant().await
    }

    async fn commit_inbound(&mut self) {
        let Some(slot) = self.inbound.granted.as_mut() else {
            return;
        };
        if slot.bytes.is_empty() {
            return;
        }
        slot.len = slot.bytes.len();
        self.inbound.commit();
        if self.inbound.needs_announce() {
            let _ = self.notify.send(self.id);
        }
    }

    async fn next_inbound_with_phy(&mut self, frame: &[u8], packet_phy: PacketPhyStats) {
        let slot = self.inbound.grant().await;
        if frame.len() > slot.cap {
            slot.clear();
            return;
        }
        slot.fill(frame);
        slot.packet_phy = packet_phy;
        self.commit_inbound().await;
    }

    async fn next_outbound(&mut self) -> &[u8] {
        self.outbound.release();
        self.outbound.peek().await.frame()
    }

    fn try_next_outbound(&mut self) -> Option<&[u8]> {
        self.outbound.release();
        Some(self.outbound.try_peek()?.frame())
    }

    async fn request_tunnel_synthesis(&mut self) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(HostCommand::SynthesizeTunnel { interface: self.id });
        }
    }
}
