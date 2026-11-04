//! Three physical segments; only the two transport nodes can cross between them.
//! The middle segment is paced independently, leaving the local fast segment live.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use personal_rns::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConnectionState, EgressCapability, IngressCapability,
    InterfaceCapabilities, InterfaceCommonPolicy, InterfaceDescriptor, InterfaceGravity,
    InterfaceId, InterfaceKind, InterfaceMode, InterfaceVitals, PeerDetails, RadioIndication,
    ReportsStatus, StatusView, TransportCapability,
};
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use personal_rns::runtime::{Attachable, AttachedInterface};
use personal_rns::wire::{WireContext, WirePacketHeader, BROADCAST_MTU};
use personal_rns::PrnsNodeHandle;
use prns_groups_reticulum::{enrollment, Ingress, Limits, RequestJob};
use prns_host::{
    DestinationConfig, HostConfig, HostRole, IdentityConfig, IdentitySecret, PersistenceConfig,
    PrnsLimits,
};
use prns_host_runtime::owner::OwnedSession;
use prns_host_runtime::{ApplicationEventDispatch, HostEmbedding, HostPreparedAttachment};
use tokio::sync::mpsc;

pub const MEMBERS: usize = 3;
const NODES: usize = 5;
// A(0) and C(2) share a fast segment with relay 3. B(1) is behind relay 4.
const LANES: [(usize, usize); 7] = [(0, 0), (2, 0), (3, 0), (3, 1), (4, 1), (4, 2), (1, 2)];

#[derive(Clone, Copy)]
struct MiddleProfile {
    frame_overhead: Duration,
    bytes_per_second: Option<u32>,
    bitrate_bps: u64,
    queue_capacity: usize,
}
impl MiddleProfile {
    const BYTE_PACED: Self = Self {
        frame_overhead: Duration::from_millis(250),
        bytes_per_second: Some(87),
        bitrate_bps: 696,
        queue_capacity: 8,
    };

    fn service_time(self, frame_bytes: usize) -> Duration {
        self.frame_overhead
            + self.bytes_per_second.map_or(Duration::ZERO, |rate| {
                Duration::from_secs_f64(frame_bytes as f64 / f64::from(rate))
            })
    }
}

struct Carrier {
    lane: usize,
    descriptor: InterfaceDescriptor,
    inbound: mpsc::Receiver<Vec<u8>>,
    outbound: mpsc::Sender<(usize, Vec<u8>)>,
}
impl Interface for Carrier {
    const HW_MTU: usize = BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Loopback;
    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }
    fn channel_tag(&self) -> &[u8] {
        self.descriptor.id.as_bytes()
    }
    async fn run<S: InterfaceSeam>(mut self, mut seam: S) {
        loop {
            tokio::select! {
                input = self.inbound.recv() => match input {
                    Some(bytes) => seam.next_inbound(&bytes).await,
                    None => return,
                },
                output = seam.next_outbound() => {
                    if self.outbound.send((self.lane, output.to_vec())).await.is_err() { return; }
                }
            }
        }
    }
}
impl ReportsStatus for Carrier {
    fn status_view(&self) -> Option<StatusView> {
        let id = self.descriptor.id;
        Some(Arc::new(move || {
            vec![InterfaceVitals {
                endpoint: None,
                id,
                connection: ConnectionState::Connected,
                failure_reason: None,
                rx_bytes: 0,
                tx_bytes: 0,
                transfer_rates: None,
                frame_accounting: None,
                radio: RadioIndication::NotRadio,
                details: PeerDetails::NotApplicable,
            }]
        }))
    }
}
impl Attachable for Carrier {
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
fn descriptor(lane: usize, segment: usize, middle: MiddleProfile) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id: InterfaceId::new([0xD0 + lane as u8; 8]),
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        gravity: InterfaceGravity::ZERO,
        bitrate: BitrateBps::guess(if segment == 1 {
            middle.bitrate_bps
        } else {
            1_000_000
        }),
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: InterfaceCommonPolicy::RNS_DEFAULT,
    }
}
pub fn limits() -> Limits {
    Limits {
        // Capacity is independent of the simulated path's throughput/latency.
        max_frame_bytes: 131_072,
        max_page_items: 8,
        max_peers: MEMBERS,
        inbound_jobs: 16,
        exchange_timeout: Duration::from_secs(5),
    }
}
pub fn enrollment_limits() -> enrollment::Limits {
    enrollment::Limits {
        max_frame_bytes: enrollment::MAX_FRAME_BYTES,
        inbound_jobs: 4,
        exchange_timeout: Duration::from_secs(5),
    }
}
pub struct State {
    pub middle_online: AtomicBool,
    pub frames: [AtomicUsize; 3],
    pub resources: AtomicUsize,
    pub jobs: [AtomicUsize; MEMBERS],
    pub middle_bytes: AtomicUsize,
    pub middle_dropped: AtomicUsize,
    pub middle_peak_queue: AtomicUsize,
}
impl State {
    fn new() -> Self {
        Self {
            middle_online: AtomicBool::new(true),
            frames: std::array::from_fn(|_| AtomicUsize::new(0)),
            resources: AtomicUsize::new(0),
            jobs: std::array::from_fn(|_| AtomicUsize::new(0)),
            middle_bytes: AtomicUsize::new(0),
            middle_dropped: AtomicUsize::new(0),
            middle_peak_queue: AtomicUsize::new(0),
        }
    }
}

async fn forward_segment(
    segment: usize,
    mut frames: mpsc::Receiver<(usize, Vec<u8>)>,
    channels: Vec<mpsc::Sender<Vec<u8>>>,
    state: Arc<State>,
    middle: MiddleProfile,
) {
    while let Some((origin, frame)) = frames.recv().await {
        if segment == 1 {
            state
                .middle_peak_queue
                .fetch_max(frames.len(), Ordering::SeqCst);
            if !state.middle_online.load(Ordering::SeqCst) {
                state.middle_dropped.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            tokio::time::sleep(middle.service_time(frame.len())).await;
            if !state.middle_online.load(Ordering::SeqCst) {
                state.middle_dropped.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            state.middle_bytes.fetch_add(frame.len(), Ordering::SeqCst);
        }
        state.frames[segment].fetch_add(1, Ordering::SeqCst);
        if segment == 1
            && WirePacketHeader::parse(&frame)
                .is_ok_and(|(header, _)| header.context == WireContext::ResourceAdvertisement)
        {
            state.resources.fetch_add(1, Ordering::SeqCst);
        }
        for (lane, &(_, recipient_segment)) in LANES.iter().enumerate() {
            if recipient_segment == segment
                && lane != origin
                && channels[lane].send(frame.clone()).await.is_err()
            {
                return;
            }
        }
    }
}
pub struct Nodes {
    pub owners: Vec<Arc<OwnedSession>>,
    pub jobs: Vec<Option<mpsc::Receiver<RequestJob>>>,
    pub enrollment_jobs: Vec<Option<mpsc::Receiver<RequestJob>>>,
    pub state: Arc<State>,
    forwarders: Vec<tokio::task::JoinHandle<()>>,
}
impl Nodes {
    pub async fn open(middle_delay: Duration) -> Self {
        Self::open_with_profile(MiddleProfile {
            frame_overhead: middle_delay,
            bytes_per_second: None,
            bitrate_bps: 8_000,
            queue_capacity: 128,
        })
        .await
    }

    /// Synthetic stress profile, not a calibrated RF/collision model. One shared
    /// eight-frame queue serves both directions; it applies backpressure instead
    /// of creating an unbounded backlog. No loss is injected while online.
    pub async fn byte_paced() -> Self {
        Self::open_with_profile(MiddleProfile::BYTE_PACED).await
    }

    async fn open_with_profile(middle: MiddleProfile) -> Self {
        let state = Arc::new(State::new());
        let mut inbound = Vec::new();
        let mut carriers: Vec<Vec<Carrier>> = (0..NODES).map(|_| Vec::new()).collect();
        let mut senders = Vec::new();
        let mut receivers = Vec::new();
        for segment in 0..3 {
            let capacity = if segment == 1 {
                middle.queue_capacity
            } else {
                128
            };
            let (sender, receiver) = mpsc::channel::<(usize, Vec<u8>)>(capacity);
            senders.push(sender);
            receivers.push(receiver);
        }
        for (lane, &(node, segment)) in LANES.iter().enumerate() {
            let (sender, receiver) = mpsc::channel(128);
            inbound.push(sender);
            carriers[node].push(Carrier {
                lane,
                descriptor: descriptor(lane, segment, middle),
                inbound: receiver,
                outbound: senders[segment].clone(),
            });
        }
        let mut forwarders = Vec::new();
        for (segment, frames) in receivers.into_iter().enumerate() {
            forwarders.push(tokio::spawn(forward_segment(
                segment,
                frames,
                inbound.clone(),
                state.clone(),
                middle,
            )));
        }
        let mut owners = Vec::new();
        let mut jobs = Vec::new();
        let mut enrollment_jobs = Vec::new();
        for (index, interfaces) in carriers.into_iter().enumerate() {
            let ingress = Arc::new(Mutex::new(None::<Ingress>));
            let callback = ingress.clone();
            let enrollment_ingress = Arc::new(Mutex::new(None::<enrollment::Ingress>));
            let enrollment_callback = enrollment_ingress.clone();
            let callback_state = state.clone();
            let destinations = if index < MEMBERS {
                vec![
                    DestinationConfig::Single(
                        prns_groups_reticulum::destination_config(limits()).unwrap(),
                    ),
                    DestinationConfig::Single(
                        enrollment::destination_config(enrollment_limits()).unwrap(),
                    ),
                ]
            } else {
                vec![]
            };
            let owner = OwnedSession::open_with_embedding(
                HostConfig {
                    // Test-only fixed identities make peer ordering stable.
                    // Group MLS secrets and wire randomness still use real entropy.
                    identity: if middle.bytes_per_second.is_some() {
                        IdentityConfig::Existing(IdentitySecret::new([0x61 + index as u8; 64]))
                    } else {
                        IdentityConfig::GenerateEphemeral
                    },
                    persistence: PersistenceConfig::Ephemeral,
                    role: if index < MEMBERS {
                        HostRole::Endpoint
                    } else {
                        HostRole::Transport
                    },
                    destinations,
                    required_capabilities: vec![],
                    limits: PrnsLimits::balanced(),
                },
                HostEmbedding {
                    on_event: Some(Box::new(move |event| {
                        if let Some(ingress) = callback.lock().unwrap().as_ref() {
                            if ingress.on_event_ref(&event)
                                == prns_groups_reticulum::IngressOutcome::Queued
                            {
                                callback_state.jobs[index].fetch_add(1, Ordering::SeqCst);
                            }
                        }
                        if let Some(ingress) = enrollment_callback.lock().unwrap().as_ref() {
                            ingress.on_event_ref(&event);
                        }
                    })),
                    application_events: ApplicationEventDispatch::EmbeddingCallback,
                    prepare_interfaces: Some(Box::new(move |client| {
                        Ok(interfaces
                            .into_iter()
                            .map(|interface| HostPreparedAttachment::Interface {
                                attachment: client.protocols().attach(interface),
                                kind: prns_host::InterfaceKind::Pipe,
                            })
                            .collect())
                    })),
                    ..HostEmbedding::default()
                },
            )
            .await
            .unwrap();
            if index < MEMBERS {
                let identity = owner.client().identity_hash().unwrap();
                let (callback, receiver) = Ingress::new(
                    prns_groups_reticulum::destination_for(identity).unwrap(),
                    limits(),
                )
                .unwrap();
                *ingress.lock().unwrap() = Some(callback);
                jobs.push(Some(receiver));
                let (callback, receiver) = enrollment::Ingress::new(
                    enrollment::destination_for(identity).unwrap(),
                    enrollment_limits(),
                )
                .unwrap();
                *enrollment_ingress.lock().unwrap() = Some(callback);
                enrollment_jobs.push(Some(receiver));
            }
            owners.push(owner);
        }
        Self {
            owners,
            jobs,
            enrollment_jobs,
            state,
            forwarders,
        }
    }
    pub async fn stop(self) {
        for owner in self.owners {
            owner.stop().await.unwrap();
        }
        for forwarder in self.forwarders {
            forwarder.abort();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn middle_carrier_serializes_directions_bounds_backlog_and_drops_during_cut() {
    let profile = MiddleProfile::BYTE_PACED;
    let state = Arc::new(State::new());
    let (send, receive) = mpsc::channel(profile.queue_capacity);
    let mut channels = Vec::new();
    let mut receivers = Vec::new();
    for _ in LANES {
        let (tx, rx) = mpsc::channel(16);
        channels.push(tx);
        receivers.push(rx);
    }
    // Queue admission is bounded before the carrier begins consuming it.
    for index in 0..profile.queue_capacity {
        send.try_send((if index % 2 == 0 { 3 } else { 4 }, vec![index as u8; 87]))
            .unwrap();
    }
    assert!(matches!(
        send.try_send((3, vec![9; 87])),
        Err(mpsc::error::TrySendError::Full(_))
    ));
    let task = tokio::spawn(forward_segment(
        1,
        receive,
        channels,
        state.clone(),
        profile,
    ));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(1_249)).await;
    tokio::task::yield_now().await;
    assert!(receivers[4].try_recv().is_err());
    assert!(receivers[3].try_recv().is_err());
    tokio::time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(receivers[4].try_recv().unwrap(), vec![0; 87]);
    assert!(
        receivers[3].try_recv().is_err(),
        "reverse transmission cannot overlap"
    );
    tokio::time::advance(Duration::from_millis(1_250)).await;
    tokio::task::yield_now().await;
    assert_eq!(receivers[3].try_recv().unwrap(), vec![1; 87]);
    assert!(receivers[4].try_recv().is_err());

    // The in-flight and waiting frames are lost on a cut; they must not emerge
    // when the carrier returns. Only a subsequent real protocol retry can pass.
    state.middle_online.store(false, Ordering::SeqCst);
    tokio::time::advance(Duration::from_millis(1_250)).await;
    tokio::task::yield_now().await;
    assert_eq!(state.middle_dropped.load(Ordering::SeqCst), 6);
    state.middle_online.store(true, Ordering::SeqCst);
    send.send((3, vec![42; 174])).await.unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(2_249)).await;
    tokio::task::yield_now().await;
    assert!(receivers[4].try_recv().is_err());
    tokio::time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(receivers[4].try_recv().unwrap(), vec![42; 174]);
    assert_eq!(state.frames[1].load(Ordering::SeqCst), 3);
    assert_eq!(state.middle_bytes.load(Ordering::SeqCst), 348);
    drop(send);
    task.await.unwrap();
}
