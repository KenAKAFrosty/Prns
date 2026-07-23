use tokio::sync::mpsc;

use prns_core::engine::InstantMillis;
use prns_core::interfaces::kiss::KissTransmissionControl;
use prns_core::interfaces::rnode::{multi, policy};
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
    PacketPhyStats,
};
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

pub(super) struct InboundFrame {
    pub(super) payload: Vec<u8>,
    pub(super) phy: PacketPhyStats,
}

pub(super) struct OutboundFrame {
    pub(super) vport: multi::VPort,
    pub(super) payload: Vec<u8>,
}

pub(super) struct RNodeMultiMember {
    pub(super) id: InterfaceId,
    pub(super) vport: multi::VPort,
    pub(super) policy: EffectiveInterfacePolicy,
    pub(super) channel_tag: Vec<u8>,
    pub(super) inbound: mpsc::UnboundedReceiver<InboundFrame>,
    pub(super) outbound: mpsc::UnboundedSender<OutboundFrame>,
    pub(super) status: TokioInterfaceStatus,
}

impl Interface for RNodeMultiMember {
    const HW_MTU: usize = policy::RNODE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Rnode;

    fn descriptor(&self) -> InterfaceDescriptor {
        policy::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        loop {
            tokio::select! {
                inbound = self.inbound.recv() => {
                    let Some(inbound) = inbound else {
                        break;
                    };
                    seam.next_inbound_with_phy(&inbound.payload, inbound.phy).await;
                }
                outbound = seam.next_outbound() => {
                    if self.outbound.send(OutboundFrame {
                        vport: self.vport,
                        payload: outbound.to_vec(),
                    }).is_err() {
                        break;
                    }
                }
            }
        }
        self.status.set_connection(ConnectionState::Disconnected);
    }
}

impl prns_core::interfaces::ReportsStatus for RNodeMultiMember {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(
            self.status.clone(),
        ))
    }
}

pub(super) struct MemberMeters {
    pub(super) status: TokioInterfaceStatus,
    pub(super) airtime: AirtimeLedger,
    pub(super) throughput: ThroughputLedger,
    pub(super) started: tokio::time::Instant,
    pub(super) bitrate: prns_core::interfaces::BitrateBps,
}

impl MemberMeters {
    pub(super) fn record_rx(&mut self, bytes: usize) {
        let bytes = bytes as u64;
        self.status.add_rx(bytes);
        let elapsed = InstantMillis(self.started.elapsed().as_millis() as u64);
        self.throughput.record_rx(elapsed, bytes);
        self.status.set_transfer_rates(self.throughput.rates());
    }

    pub(super) fn record_tx(&mut self, bytes: usize) {
        let bytes = bytes as u64;
        self.status.add_tx(bytes);
        let elapsed = InstantMillis(self.started.elapsed().as_millis() as u64);
        self.throughput.record_tx(elapsed, bytes);
        self.status.set_transfer_rates(self.throughput.rates());
        self.status.set_airtime(
            self.airtime
                .record_tx(elapsed, frame_airtime_us(bytes as usize, self.bitrate)),
        );
    }
}

pub(super) struct LiveMember {
    pub(super) vport: multi::VPort,
    pub(super) radio: multi::RadioConfig,
    pub(super) inbound: mpsc::UnboundedSender<InboundFrame>,
    pub(super) control: KissTransmissionControl,
    pub(super) meters: MemberMeters,
}
