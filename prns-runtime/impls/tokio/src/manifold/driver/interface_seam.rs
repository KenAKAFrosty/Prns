use tokio::sync::mpsc::UnboundedSender;

use crate::interfaces::{FrameSink, InterfaceId, InterfaceOriginKind, PacketPhyStats};
use crate::manifold::interface_seam::InterfaceSeam;

use super::{HostCommand, TokioEntropy, TokioGrantConsumer, TokioGrantProducer};

/// The tokio side of one interface's seam: `next_inbound` frames funnel into the manifold's one inbound stream (tagged with this interface's id), and `next_outbound` parks on this interface's own outbound queue until the manifold enqueues a frame for it.
pub struct TokioInterfaceSeam {
    id: InterfaceId,
    origin: InterfaceOriginKind,
    inbound: TokioGrantProducer,
    notify: UnboundedSender<InterfaceId>,
    outbound: TokioGrantConsumer,
    commands: Option<UnboundedSender<HostCommand>>,
    entropy: TokioEntropy,
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
            entropy: TokioEntropy,
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
    fn fill_random(&mut self, bytes: &mut [u8]) {
        self.entropy.fill(bytes);
    }

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

    fn accept_outbound_custody(&mut self) {
        self.outbound.release();
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
#[cfg(test)]
mod tests {
    use super::*;

    use crate::interfaces::{RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb};
    use crate::manifold::grant_lane::tokio_grant_lane;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn the_seam_signals_a_synthesize_request_carrying_its_interface_id() {
        let id = InterfaceId::new([0xC7; 8]);
        let (in_producer, _in_consumer) = tokio_grant_lane(64, 2);
        let (_out_producer, out_consumer) = tokio_grant_lane(64, 2);
        let (notify_tx, _notify_rx) = mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<HostCommand>();
        let mut seam =
            TokioInterfaceSeam::new(id, in_producer, notify_tx, out_consumer).with_commands(cmd_tx);

        seam.request_tunnel_synthesis().await;

        let got = cmd_rx
            .try_recv()
            .expect("a synthesize request reached the manifold");
        assert!(matches!(got, HostCommand::SynthesizeTunnel { interface } if interface == id));
    }

    #[tokio::test]
    async fn a_seam_without_a_command_channel_drops_the_synthesize_request() {
        let id = InterfaceId::new([0xC8; 8]);
        let (in_producer, _in_consumer) = tokio_grant_lane(64, 2);
        let (_out_producer, out_consumer) = tokio_grant_lane(64, 2);
        let (notify_tx, _notify_rx) = mpsc::unbounded_channel();
        let mut seam = TokioInterfaceSeam::new(id, in_producer, notify_tx, out_consumer);

        seam.request_tunnel_synthesis().await;
    }

    #[tokio::test]
    async fn packet_phy_crosses_the_tokio_ingress_seam_with_its_frame() {
        let id = InterfaceId::new([0xC9; 8]);
        let (in_producer, mut in_consumer) = tokio_grant_lane(64, 2);
        let (_out_producer, out_consumer) = tokio_grant_lane(64, 2);
        let (notify_tx, _notify_rx) = mpsc::unbounded_channel();
        let mut seam = TokioInterfaceSeam::new(id, in_producer, notify_tx, out_consumer);
        let packet_phy = PacketPhyStats {
            rssi: Some(RssiDbm::new(-91)),
            snr: Some(SnrQuarterDb::new(-7)),
            quality: SignalQualityTenthsPercent::new(812),
        };

        seam.next_inbound_with_phy(b"observed", packet_phy).await;

        let retained = in_consumer.try_peek().expect("the frame crossed the seam");
        assert_eq!(
            (retained.frame(), retained.packet_phy),
            (b"observed".as_slice(), packet_phy)
        );
    }
}
