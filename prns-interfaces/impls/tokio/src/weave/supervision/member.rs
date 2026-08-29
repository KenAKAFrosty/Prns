use tokio::sync::mpsc;

use prns_core::engine::InstantMillis;
use prns_core::interfaces::weave::{self, EndpointId};
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

pub(super) const PEER_INBOUND_DEPTH: usize = 32;
pub(super) const DEVICE_OUTBOUND_DEPTH: usize = 32;

pub(super) struct OutboundPacket {
    pub(super) endpoint: EndpointId,
    pub(super) payload: Vec<u8>,
}

pub(super) struct WeavePeer {
    id: InterfaceId,
    endpoint: EndpointId,
    policy: EffectiveInterfacePolicy,
    channel_tag: Vec<u8>,
    inbound: mpsc::Receiver<Vec<u8>>,
    outbound: mpsc::Sender<OutboundPacket>,
    status: TokioInterfaceStatus,
}

impl WeavePeer {
    pub(super) fn new(
        parent_channel_tag: &[u8],
        endpoint: EndpointId,
        policy: EffectiveInterfacePolicy,
        inbound: mpsc::Receiver<Vec<u8>>,
        outbound: mpsc::Sender<OutboundPacket>,
    ) -> Self {
        let mut channel_tag = (parent_channel_tag.len() as u64).to_be_bytes().to_vec();
        channel_tag.extend_from_slice(parent_channel_tag);
        channel_tag.extend_from_slice(endpoint.as_bytes());
        let id = InterfaceId::from_channel_tag(InterfaceKind::WeavePeer, &channel_tag);
        Self {
            id,
            endpoint,
            policy,
            channel_tag,
            inbound,
            outbound,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected),
        }
    }

    pub(super) fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl Interface for WeavePeer {
    const HW_MTU: usize = weave::WEAVE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::WeavePeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        weave::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let started = tokio::time::Instant::now();
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        loop {
            tokio::select! {
                inbound = self.inbound.recv() => {
                    let Some(packet) = inbound else {
                        break;
                    };
                    self.status.add_rx(packet.len() as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_rx(now, packet.len() as u64);
                    self.status.set_transfer_rates(throughput.rates());
                    seam.next_inbound(&packet).await;
                }
                outbound = seam.next_outbound() => {
                    if outbound.is_empty() || outbound.len() > weave::WEAVE_MAX_WIRE_PACKET {
                        continue;
                    }
                    let packet = OutboundPacket {
                        endpoint: self.endpoint,
                        payload: outbound.to_vec(),
                    };
                    let packet_len = packet.payload.len();
                    if self.outbound.send(packet).await.is_err() {
                        break;
                    }
                    self.status.add_tx(packet_len as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_tx(now, packet_len as u64);
                    self.status.set_transfer_rates(throughput.rates());
                    self.status.set_airtime(airtime.record_tx(
                        now,
                        frame_airtime_us(packet_len, self.policy.bitrate),
                    ));
                }
            }
        }
        self.status.set_connection(ConnectionState::Disconnected);
    }
}

impl prns_core::interfaces::ReportsStatus for WeavePeer {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }
}
