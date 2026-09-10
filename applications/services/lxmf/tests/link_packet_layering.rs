#![cfg(feature = "tokio-host")]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use personal_rns::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceDescriptor, InterfaceGravity, InterfaceId, InterfaceKind,
    InterfaceMode, ReportsStatus, TransportCapability,
};
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use personal_rns::remote_control::RemoteControlService;
use personal_rns::routing::delivery::Delivery;
use personal_rns::runtime::{
    Attachable, AttachedInterface, ManuallyAttached, Message, NoPersistence, PrnsEvent,
    PrnsNodeRecipe,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::{
    DestinationType, PacketType, WireContext, WirePacketHeader, BROADCAST_MTU,
};
use personal_rns::{PrnsNode, PrnsNodeHandle};
use prns_lxmf::direct::{
    prepare_local_lxmf_destination, DirectLxmfService, DirectNetwork, PrnsDirectNetwork,
};
use prns_lxmf::wire::{
    compose_basic_direct_lxmf, encode_current_lxmf_announce, MAX_BASIC_LXMF_WIRE_BYTES,
};
use prns_lxmf::LxmfVerification;
use tokio::sync::{mpsc, oneshot};

const SENDER_SECRET: [u8; 64] = [0x31; 64];
const RECEIVER_SECRET: [u8; 64] = [0x52; 64];
const SENDER_INTERFACE: InterfaceId = InterfaceId::new([0xA1; 8]);
const RECEIVER_INTERFACE: InterfaceId = InterfaceId::new([0xB2; 8]);
const WAIT: Duration = Duration::from_secs(5);

struct ControlledInterface {
    descriptor: InterfaceDescriptor,
    inbound: mpsc::UnboundedReceiver<Vec<u8>>,
    outbound: mpsc::UnboundedSender<Vec<u8>>,
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
                inbound = self.inbound.recv() => match inbound {
                    Some(bytes) => seam.next_inbound(&bytes).await,
                    None => return,
                },
                outbound = seam.next_outbound() => {
                    let _sent = self.outbound.send(outbound.to_vec());
                }
            }
        }
    }
}

impl ReportsStatus for ControlledInterface {}

impl Attachable for ControlledInterface {
    type Attached = AttachedInterface;

    fn attach_to(self, handle: &PrnsNodeHandle) -> Self::Attached {
        handle.add_interface(self)
    }

    fn attach_to_with_ifac(
        self,
        handle: &PrnsNodeHandle,
        ifac: personal_rns::interfaces::IfacContext,
        network_name: Option<String>,
    ) -> Self::Attached {
        handle.add_interface_with_ifac_name(self, ifac, network_name)
    }
}

fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
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

#[derive(Default)]
struct WireState {
    receiver_proofs: AtomicUsize,
    corrupt_next_sender_data: AtomicBool,
    last_sender_data: Mutex<Option<Vec<u8>>>,
}

impl WireState {
    fn proof_count(&self) -> usize {
        self.receiver_proofs.load(Ordering::Acquire)
    }

    fn corrupt_next_sender_data(&self) {
        self.corrupt_next_sender_data.store(true, Ordering::Release);
    }

    fn captured_sender_data(&self) -> Vec<u8> {
        self.last_sender_data
            .lock()
            .unwrap()
            .clone()
            .expect("a sender Link DATA frame was captured")
    }
}

struct WireHarness {
    sender: ControlledInterface,
    receiver: ControlledInterface,
    receiver_inbound: mpsc::UnboundedSender<Vec<u8>>,
    state: Arc<WireState>,
    forwarder: tokio::task::JoinHandle<()>,
}

impl WireHarness {
    fn new() -> Self {
        let (sender_inbound, sender_inbound_rx) = mpsc::unbounded_channel();
        let (receiver_inbound, receiver_inbound_rx) = mpsc::unbounded_channel();
        let (sender_outbound, mut sender_outbound_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (receiver_outbound, mut receiver_outbound_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let state = Arc::new(WireState::default());
        let forward_state = Arc::clone(&state);
        let receiver_forward = receiver_inbound.clone();
        let forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    sender_frame = sender_outbound_rx.recv() => {
                        let Some(mut frame) = sender_frame else { return };
                        if is_user_link_data(&frame) {
                            *forward_state.last_sender_data.lock().unwrap() = Some(frame.clone());
                            if forward_state
                                .corrupt_next_sender_data
                                .swap(false, Ordering::AcqRel)
                            {
                                let last = frame.len().checked_sub(1).expect("a Link frame is nonempty");
                                frame[last] ^= 0x01;
                            }
                        }
                        if receiver_forward.send(frame).is_err() {
                            return;
                        }
                    }
                    receiver_frame = receiver_outbound_rx.recv() => {
                        let Some(frame) = receiver_frame else { return };
                        if is_link_proof(&frame) {
                            forward_state.receiver_proofs.fetch_add(1, Ordering::AcqRel);
                        }
                        if sender_inbound.send(frame).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Self {
            sender: ControlledInterface {
                descriptor: descriptor(SENDER_INTERFACE),
                inbound: sender_inbound_rx,
                outbound: sender_outbound,
            },
            receiver: ControlledInterface {
                descriptor: descriptor(RECEIVER_INTERFACE),
                inbound: receiver_inbound_rx,
                outbound: receiver_outbound,
            },
            receiver_inbound,
            state,
            forwarder,
        }
    }
}

fn is_user_link_data(frame: &[u8]) -> bool {
    WirePacketHeader::parse(frame).is_ok_and(|(header, _)| {
        header.destination_type == DestinationType::Link
            && header.packet_type == PacketType::Data
            && header.context == WireContext::None
    })
}

fn is_link_proof(frame: &[u8]) -> bool {
    WirePacketHeader::parse(frame).is_ok_and(|(header, _)| {
        header.destination_type == DestinationType::Link && header.packet_type == PacketType::Proof
    })
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the layered packet condition becomes true");
}

async fn wait_for_message_count(service: &DirectLxmfService, expected: usize) {
    tokio::time::timeout(WAIT, async {
        loop {
            if service.snapshot().await.messages.len() == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the LXMF service reaches the expected logical message count");
}

async fn wait_for_route(network: &PrnsDirectNetwork, destination: [u8; 16]) {
    tokio::time::timeout(WAIT, async {
        while !network.has_route(destination).await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the direct announce creates a route");
}

async fn stop_node(
    shutdown: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<(), personal_rns::runtime::NodeRunError>>,
) {
    let _sent = shutdown.send(());
    tokio::time::timeout(WAIT, task)
        .await
        .expect("node shutdown is bounded")
        .expect("node task joins")
        .expect("node shuts down cleanly");
}

#[tokio::test(flavor = "current_thread")]
async fn link_proof_packet_replay_and_lxmf_dedup_are_layered() {
    tokio::task::LocalSet::new()
        .run_until(run_link_proof_packet_replay_and_lxmf_dedup_are_layered())
        .await;
}

async fn run_link_proof_packet_replay_and_lxmf_dedup_are_layered() {
    let mut sender_announce = [0_u8; 64];
    let sender_announce_len =
        encode_current_lxmf_announce(b"Layer sender", &mut sender_announce).unwrap();
    let (sender_identity, sender_destination) = prepare_local_lxmf_destination(
        personal_rns::identity::Zeroizing::new(SENDER_SECRET),
        Box::leak(
            sender_announce[..sender_announce_len]
                .to_vec()
                .into_boxed_slice(),
        ),
    )
    .unwrap();
    let sender_destination_hash = sender_identity.destination();

    let mut receiver_announce = [0_u8; 64];
    let receiver_announce_len =
        encode_current_lxmf_announce(b"Layer receiver", &mut receiver_announce).unwrap();
    let (receiver_identity, receiver_destination) = prepare_local_lxmf_destination(
        personal_rns::identity::Zeroizing::new(RECEIVER_SECRET),
        Box::leak(
            receiver_announce[..receiver_announce_len]
                .to_vec()
                .into_boxed_slice(),
        ),
    )
    .unwrap();
    let receiver_destination_hash = receiver_identity.destination();
    let (pending_receiver, receiver_callbacks) = DirectLxmfService::prepare(receiver_identity);

    let raw_deliveries = Arc::new(AtomicUsize::new(0));
    let delivery_counter = Arc::clone(&raw_deliveries);
    let event_callbacks = receiver_callbacks.clone();
    let receiver_node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: RemoteControlService::Unavailable,
        pre_configured_destinations: [receiver_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: personal_rns::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state: &()| {
            let _outcome = event_callbacks.on_prns_event(&event);
            if matches!(
                event,
                PrnsEvent::Message(Message::Delivered(Delivery::Link(_)))
            ) {
                delivery_counter.fetch_add(1, Ordering::AcqRel);
            }
        },
    })
    .with_accepted_announce_observer(receiver_callbacks.accepted_announce_observer());
    let sender_node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: RemoteControlService::Unavailable,
        pre_configured_destinations: [sender_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: personal_rns::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    });
    let receiver_handle = receiver_node.handle();
    let sender_handle = sender_node.handle();

    let WireHarness {
        sender,
        receiver,
        receiver_inbound,
        state: wire,
        forwarder,
    } = WireHarness::new();
    let _sender_attachment = sender_handle.attach(sender);
    let _receiver_attachment = receiver_handle.attach(receiver);
    let (sender_shutdown, sender_shutdown_rx) = oneshot::channel();
    let (receiver_shutdown, receiver_shutdown_rx) = oneshot::channel();
    let sender_task = tokio::task::spawn_local(sender_node.run_until(async move {
        let _stopped = sender_shutdown_rx.await;
    }));
    let receiver_task = tokio::task::spawn_local(receiver_node.run_until(async move {
        let _stopped = receiver_shutdown_rx.await;
    }));

    let sender_network = PrnsDirectNetwork::new(sender_handle.clone());
    let receiver_network = Arc::new(PrnsDirectNetwork::new(receiver_handle.clone()));
    let receiver_service = pending_receiver.start(receiver_network).unwrap();
    sender_network
        .announce(sender_destination_hash)
        .await
        .unwrap();
    receiver_service.announce().await.unwrap();
    wait_for_route(&sender_network, receiver_destination_hash).await;

    let link = sender_handle
        .establish_link(personal_rns::wire::DestinationHash::new(
            receiver_destination_hash,
        ))
        .await
        .unwrap();
    let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let message = compose_basic_direct_lxmf(
        receiver_destination_hash,
        sender_destination_hash,
        1_700_000_000_123,
        b"layered",
        b"one logical message",
        None,
        &sender_identity,
        &mut output,
    )
    .unwrap();
    let exact_wire = output[..usize::from(message.wire_len())].to_vec();

    let proofs_before = wire.proof_count();
    sender_handle
        .send_link_packet(link, &exact_wire)
        .await
        .unwrap();
    wait_for_message_count(&receiver_service, 1).await;
    let received = receiver_service.snapshot().await;
    assert_eq!(
        received.messages[0].verification,
        LxmfVerification::Verified
    );
    assert_eq!(received.messages[0].exact_wire, exact_wire);
    assert_eq!(raw_deliveries.load(Ordering::Acquire), 1);
    assert_eq!(wire.proof_count(), proofs_before + 1);

    let replay = wire.captured_sender_data();
    let replay_proofs = wire.proof_count();
    let replay_deliveries = raw_deliveries.load(Ordering::Acquire);
    receiver_inbound.send(replay.clone()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        wire.proof_count(),
        replay_proofs,
        "a packet replay receives no proof"
    );
    assert_eq!(
        raw_deliveries.load(Ordering::Acquire),
        replay_deliveries,
        "a packet replay reaches no application delivery"
    );

    sender_handle
        .send_link_packet(link, &exact_wire)
        .await
        .unwrap();
    wait_until(|| raw_deliveries.load(Ordering::Acquire) == replay_deliveries + 1).await;
    assert_ne!(
        wire.captured_sender_data(),
        replay,
        "resending the same LXM uses fresh Link encryption"
    );
    wait_for_message_count(&receiver_service, 1).await;
    assert_eq!(
        wire.proof_count(),
        replay_proofs + 1,
        "fresh encryption of a duplicate LXM is proven"
    );

    let malformed_proofs = wire.proof_count();
    let malformed_deliveries = raw_deliveries.load(Ordering::Acquire);
    sender_handle.send_link_packet(link, &[0xC1]).await.unwrap();
    wait_until(|| raw_deliveries.load(Ordering::Acquire) == malformed_deliveries + 1).await;
    wait_for_message_count(&receiver_service, 1).await;
    assert_eq!(
        wire.proof_count(),
        malformed_proofs + 1,
        "successfully decrypted malformed LXM is still transport-proven"
    );

    let corrupt_proofs = wire.proof_count();
    let corrupt_deliveries = raw_deliveries.load(Ordering::Acquire);
    wire.corrupt_next_sender_data();
    let corrupt_sender: PrnsNodeHandle = sender_handle.clone();
    let corrupt_attempt = tokio::spawn(async move {
        corrupt_sender
            .send_link_packet(link, b"does not decrypt")
            .await
    });
    wait_until(|| !wire.corrupt_next_sender_data.load(Ordering::Acquire)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        wire.proof_count(),
        corrupt_proofs,
        "a carrier that fails Reticulum decryption receives no proof"
    );
    assert_eq!(
        raw_deliveries.load(Ordering::Acquire),
        corrupt_deliveries,
        "a carrier that fails decryption reaches no application callback"
    );
    wait_for_message_count(&receiver_service, 1).await;
    corrupt_attempt.abort();

    receiver_service.stop().await.unwrap();
    stop_node(sender_shutdown, sender_task).await;
    stop_node(receiver_shutdown, receiver_task).await;
    forwarder.abort();
}
