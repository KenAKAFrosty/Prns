use personal_rns::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceDescriptor, InterfaceGravity, InterfaceId, InterfaceKind,
    InterfaceMode, ReportsStatus, TransportCapability,
};
use personal_rns::manifold::interface_seam::{
    Interface, InterfaceSeam, OutboundDisposition, OutboundDropReason,
};
use tokio::sync::mpsc;

use crate::medium::{EndpointId, TransmitError, VirtualMedium};

const VIRTUAL_BITRATE: BitrateBps = BitrateBps::guess(1_000_000_000);

pub struct VirtualInterface {
    medium: VirtualMedium,
    endpoint: EndpointId,
    channel_tag: Vec<u8>,
    inbound: mpsc::Receiver<Vec<u8>>,
}

impl VirtualInterface {
    pub(crate) fn new(
        medium: VirtualMedium,
        endpoint: EndpointId,
        channel_tag: Vec<u8>,
        inbound: mpsc::Receiver<Vec<u8>>,
    ) -> Self {
        Self {
            medium,
            endpoint,
            channel_tag,
            inbound,
        }
    }

    #[must_use]
    pub const fn endpoint_id(&self) -> EndpointId {
        self.endpoint
    }
}

impl Drop for VirtualInterface {
    fn drop(&mut self) {
        self.medium.detach(self.endpoint);
    }
}

impl ReportsStatus for VirtualInterface {}

impl Interface for VirtualInterface {
    const HW_MTU: usize = personal_rns::wire::BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Loopback;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    fn descriptor(&self) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id: InterfaceId::from_channel_tag(Self::KIND, &self.channel_tag),
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            gravity: InterfaceGravity::ZERO,
            bitrate: VIRTUAL_BITRATE,
            hardware_mtu: Some(Self::HW_MTU),
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
            common: InterfaceCommonPolicy::RNS_DEFAULT,
        }
    }

    async fn run<S: InterfaceSeam>(mut self, mut seam: S) {
        loop {
            tokio::select! {
                received = self.inbound.recv() => {
                    let Some(frame) = received else { return };
                    seam.next_inbound(&frame).await;
                }
                outbound = seam.next_outbound() => {
                    let frame = outbound.to_vec();
                    let disposition = match self.medium.transmit(self.endpoint, frame) {
                        Ok(()) => OutboundDisposition::Sent,
                        Err(TransmitError::DetachedEndpoint) => {
                            OutboundDisposition::Dropped(OutboundDropReason::Disconnected)
                        }
                        Err(TransmitError::TransmissionOrdinalsExhausted) => {
                            OutboundDisposition::Dropped(OutboundDropReason::Rejected)
                        }
                    };
                    seam.complete_outbound(disposition);
                }
            }
        }
    }
}
