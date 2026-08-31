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

#[test]
fn local_command_bursts_make_bounded_room_for_shared_producers() {
    use crate::engine::{CloseLink, CommandId, PrnsCommand};
    use crate::routing::links::LinkId;
    use crate::wire::TRUNCATED_HASH_BYTE_LEN;

    let command = |id| {
        HostCommand::Engine(IssuedCommand {
            id: CommandId(id),
            command: PrnsCommand::CloseLink(CloseLink {
                link_id: LinkId::new([id as u8; TRUNCATED_HASH_BYTE_LEN]),
            }),
        })
    };
    let id = |command| {
        let HostCommand::Engine(issued) = command else {
            panic!("test command")
        };
        issued.id
    };
    let (mut local_tx, mut local_rx) = local_command_lane(4);
    let (shared_tx, mut shared_rx) = mpsc::unbounded_channel();
    assert!(local_tx.send(command(1)).is_ok());
    assert!(local_tx.send(command(2)).is_ok());
    assert!(local_tx.send(command(3)).is_ok());
    assert!(shared_tx.send(command(99)).is_ok());
    let mut local_streak = 0;

    assert_eq!(
        id(next_command(&mut local_rx, &mut shared_rx, &mut local_streak, 2).unwrap()),
        CommandId(1)
    );
    assert_eq!(
        id(next_command(&mut local_rx, &mut shared_rx, &mut local_streak, 2).unwrap()),
        CommandId(2)
    );
    assert_eq!(
        id(next_command(&mut local_rx, &mut shared_rx, &mut local_streak, 2).unwrap()),
        CommandId(99),
        "the shared producer runs after one bounded local burst"
    );
    assert_eq!(
        id(next_command(&mut local_rx, &mut shared_rx, &mut local_streak, 2).unwrap()),
        CommandId(3)
    );
}
