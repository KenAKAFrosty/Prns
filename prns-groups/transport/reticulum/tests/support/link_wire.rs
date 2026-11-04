//! A controlled transport shared by independent and catalog-managed fixtures.
//! It only supplies interfaces and frame forwarding. Every endpoint, key,
//! authenticated Link and request remains owned by the production consumer.
use personal_rns::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceDescriptor, InterfaceGravity, InterfaceId, InterfaceKind,
    InterfaceMode, ReportsStatus, TransportCapability,
};
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use personal_rns::runtime::{Attachable, AttachedInterface};
use personal_rns::wire::{PacketType, WireContext, WirePacketHeader, BROADCAST_MTU};
use personal_rns::PrnsNodeHandle;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct ControlledInterface {
    descriptor: InterfaceDescriptor,
    peer: usize,
    inbound: mpsc::UnboundedReceiver<Vec<u8>>,
    outbound: mpsc::UnboundedSender<(usize, Vec<u8>)>,
}

impl Interface for ControlledInterface {
    const HW_MTU: usize = BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Loopback;
    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }
    fn channel_tag(&self) -> &[u8] {
        self.descriptor.id.as_bytes()
    }
    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        loop {
            tokio::select! {
                input = self.inbound.recv() => match input {
                    Some(bytes) => seam.next_inbound(&bytes).await,
                    None => return,
                },
                output = seam.next_outbound() => {
                    if self.outbound.send((self.peer, output.to_vec())).is_err() { return; }
                }
            }
        }
    }
}
impl ReportsStatus for ControlledInterface {
    fn status_view(&self) -> Option<personal_rns::interfaces::StatusView> {
        let id = self.descriptor.id;
        Some(Arc::new(move || {
            vec![personal_rns::interfaces::InterfaceVitals {
                endpoint: None,
                id,
                connection: personal_rns::interfaces::ConnectionState::Connected,
                failure_reason: None,
                rx_bytes: 0,
                tx_bytes: 0,
                transfer_rates: None,
                frame_accounting: None,
                radio: personal_rns::interfaces::RadioIndication::for_kind(Some(
                    InterfaceKind::Loopback,
                )),
                details: personal_rns::interfaces::PeerDetails::NotApplicable,
            }]
        }))
    }
}
impl Attachable for ControlledInterface {
    type Attached = AttachedInterface;
    fn attach_to(self, node: &PrnsNodeHandle) -> Self::Attached {
        node.add_interface(self)
    }
    fn attach_to_with_ifac(
        self,
        node: &PrnsNodeHandle,
        ifac: personal_rns::interfaces::IfacContext,
        name: Option<String>,
    ) -> Self::Attached {
        node.add_interface_with_ifac_name(self, ifac, name)
    }
}

fn descriptor(peer: usize) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id: InterfaceId::new([peer as u8 + 0xA0; 8]),
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        gravity: InterfaceGravity::ZERO,
        bitrate: BitrateBps::guess(1_000_000),
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: InterfaceCommonPolicy::RNS_DEFAULT,
    }
}

pub struct WireState<const N: usize> {
    online: [AtomicBool; N],
    pub unsolicited_announces: AtomicUsize,
    pub path_responses: AtomicUsize,
    pub resources: AtomicUsize,
    pub resource_proofs: AtomicUsize,
    pub frames: [AtomicUsize; N],
    pub jobs: [AtomicUsize; N],
}
impl<const N: usize> WireState<N> {
    pub fn set_online(&self, peer: usize, online: bool) {
        self.online[peer].store(online, Ordering::SeqCst);
    }
}

/// Prepare N connected interfaces without creating hosts, keys or services.
/// Start the returned forwarder after each host has attached its interface.
pub fn prepare<const N: usize>() -> (
    Vec<ControlledInterface>,
    Arc<WireState<N>>,
    impl std::future::Future<Output = ()> + Send + 'static,
) {
    let state = Arc::new(WireState {
        online: std::array::from_fn(|_| AtomicBool::new(true)),
        unsolicited_announces: AtomicUsize::new(0),
        path_responses: AtomicUsize::new(0),
        resources: AtomicUsize::new(0),
        resource_proofs: AtomicUsize::new(0),
        frames: std::array::from_fn(|_| AtomicUsize::new(0)),
        jobs: std::array::from_fn(|_| AtomicUsize::new(0)),
    });
    let (outbound, mut frames) = mpsc::unbounded_channel::<(usize, Vec<u8>)>();
    let mut inbound = Vec::new();
    let mut interfaces = Vec::new();
    for peer in 0..N {
        let (sender, receiver) = mpsc::unbounded_channel();
        inbound.push(sender);
        interfaces.push(ControlledInterface {
            descriptor: descriptor(peer),
            peer,
            inbound: receiver,
            outbound: outbound.clone(),
        });
    }
    let forward_state = Arc::clone(&state);
    let forwarder = async move {
        while let Some((origin, frame)) = frames.recv().await {
            if !forward_state.online[origin].load(Ordering::SeqCst) {
                continue;
            }
            forward_state.frames[origin].fetch_add(1, Ordering::SeqCst);
            if let Ok((header, _)) = WirePacketHeader::parse(&frame) {
                if header.packet_type == PacketType::Announce {
                    if header.context == WireContext::PathResponse {
                        forward_state.path_responses.fetch_add(1, Ordering::SeqCst);
                    } else {
                        forward_state
                            .unsolicited_announces
                            .fetch_add(1, Ordering::SeqCst);
                    }
                }
                if header.context == WireContext::ResourceAdvertisement {
                    forward_state.resources.fetch_add(1, Ordering::SeqCst);
                }
                if header.context == WireContext::ResourceProof
                    && header.packet_type == PacketType::Proof
                {
                    forward_state.resource_proofs.fetch_add(1, Ordering::SeqCst);
                }
            }
            for (recipient, channel) in inbound.iter().enumerate() {
                if recipient != origin && forward_state.online[recipient].load(Ordering::SeqCst) {
                    let _ = channel.send(frame.clone());
                }
            }
        }
    };
    (interfaces, state, forwarder)
}
