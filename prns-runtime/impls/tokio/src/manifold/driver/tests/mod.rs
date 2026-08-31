use super::*;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
#[cfg(feature = "runtime-metrics")]
use tokio::sync::oneshot;

use crate::engine::test_support::{
    bytes_from_hex, pin_transport_id, TestStorageLayout, RNS_1_4_2_ANNOUNCE,
    RNS_1_4_2_RATCHETED_ANNOUNCE, TEST_TRANSPORT_ID,
};
#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::{Departure, IssuedCommand, RouteRemovalCause};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceMode, TransportCapability,
};
use crate::manifold::interface_seam::{Interface, InterfaceSeam, MAX_WIRE_FRAME_LEN};
#[cfg(feature = "runtime-metrics")]
use crate::runtime::AnnounceEgressOutcome;
use crate::runtime::{DropRouteOutcome, PrnsNodeHandle, RoutingControl};
use crate::wire::{DestinationHash, PacketType, WirePacketHeader};

use tokio::sync::mpsc;

fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        gravity: crate::interfaces::InterfaceGravity::ZERO,
        bitrate: BitrateBps::guess(1_000_000_000),
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: crate::interfaces::InterfaceCommonPolicy::RNS_DEFAULT,
    }
}

struct LoopbackInterface {
    descriptor: InterfaceDescriptor,
    wire_in: UnboundedReceiver<std::vec::Vec<u8>>,
    wire_out: UnboundedSender<std::vec::Vec<u8>>,
}

impl Interface for LoopbackInterface {
    const HW_MTU: usize = crate::wire::BROADCAST_MTU;
    const KIND: crate::interfaces::InterfaceKind = crate::interfaces::InterfaceKind::Loopback;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn channel_tag(&self) -> &[u8] {
        self.descriptor.id.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        loop {
            tokio::select! {
                received = self.wire_in.recv() => {
                    match received {
                        Some(bytes) => seam.next_inbound(&bytes).await,
                        None => return,
                    }
                }
                outbound = seam.next_outbound() => {
                    let _ = self.wire_out.send(outbound.to_vec());
                }
            }
        }
    }
}

mod announces;
mod interfaces;
mod links;
mod remote_control_pairing;
mod routes;
mod transport;
