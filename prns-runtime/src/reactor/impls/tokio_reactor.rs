use core::cell::{Cell, RefCell};
use prns_core::interfaces::IndexedAttachedInterfaces;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::crypto::{
    ed25519_sign, x25519_diffie_hellman, x25519_keys_for_seal, Ed25519Signature, Ed25519Verifier,
    X25519PublicKey, X25519SharedSecret,
};
use crate::engine::write_implicit_proof_wire_packet;
#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::{
    AnnounceVerifyOwed, CommandId, DecryptOwed, DeferredCrypto, DeferredProofSign, Departure,
    Directive, EncryptOwed, EngineCommand, EngineReaction, EngineState, FanTarget, IngestIo,
    InstantMillis, IssuedCommand, Journaled, NextWake, ProofIngest, ProofRequest,
    RatchetDecryptOwed, Respond, RespondData, SendRequest, SendRequestData, SendRequestFailure,
    SendSinglePacketEntropy, SendSinglePacketFailure, SendSinglePacketPrepared,
    SendSinglePacketWriteError, Settlement, WakeReason, WakeSchedules,
};
use crate::identity::{
    decrypt_token_in_place_with_ratchets, IdentitySigningPublicKey, OpenedBy, OpenedToken,
    Zeroizing,
};
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{
    AirtimeUtilization, ConnectionState, ConnectionView, FrameSink, InboundPacket,
    InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceStatus, PacketPhyStats,
    TransferRates,
};
#[cfg(feature = "runtime-metrics")]
use crate::reactor::announce_pacer::PacerOffer;
use crate::reactor::announce_pacer::{AnnouncePacer, HeapPacerQueue};
use crate::reactor::driver::{fire_due_reason, merge_wake_schedules_delta};
use crate::reactor::interface_seam::{
    frame_cap_for, InterfaceSeam, BROADCAST_WIRE_FRAME_LEN, MAX_WIRE_FRAME_LEN,
};
use crate::reactor::AppDeciders;
use crate::reactor::Host;
use crate::routing::announce::Announce;
use crate::routing::dedup::PacketHash;
use crate::routing::ingress::MAX_RATCHET_DECRYPT_PAYLOAD_LEN;
use crate::routing::links::channel::byte_stream::{self, StreamId, STREAM_DATA_TYPE};
use crate::routing::links::handshake::{
    link_proof_signature_valid, link_proof_signed_data, LinkProofSignOwed, LinkProofVerifyOwed,
};
use crate::routing::links::request::{write_request_plaintext, RequestId, REQUEST_WIRE_OVERHEAD};
use crate::routing::links::resources::build_outgoing::{
    seal_staged_resource, BuildOutgoingResourceError, BuildRegions, SealedStagedResource,
    SALT_REROLL_CAP,
};
use crate::routing::links::resources::receive::offload::OffloadedOpenSpan;
use crate::routing::links::resources::send::OffloadedStagedSeal;
use crate::routing::links::resources::streamed_open::{ResourceOpenLane, StreamedOpen};
use crate::routing::links::resources::ResourceOffer;
use crate::routing::links::resources::{sealed_transfer_len, MAP_HASH_LEN, RESOURCE_NONCE_LEN};
use crate::routing::links::resources::{
    ResourceBody, ResourceCorrelation, ResourceHash, ResourceMetadata, ResourceSegment,
    ResourceSend, ResourceStrategy,
};
use crate::routing::links::{LinkId, LinkKey};
use crate::routing::proof::IMPLICIT_PROOF_WIRE_LEN;
use crate::routing::request_handlers::RequestPathHash;
use crate::runtime::InterfaceStore;
#[cfg(feature = "runtime-metrics")]
use crate::runtime::{
    AnnounceEgressOutcome, CryptoMetricsSnapshot, EgressMetricsSnapshot,
    ReliabilityMetricsSnapshot, RuntimeMetricsSnapshot,
};
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::units::RttMillis;
use crate::wire::{DestinationHash, PacketType, WirePacketHeader};
use heapless::Vec as HeaplessVec;

pub use super::tokio_grant_lane::{
    tokio_grant_lane, HeapFrameSlot, TokioGrantConsumer, TokioGrantProducer,
};

const MAX_TIMER_ARM_MILLIS: u64 = 24 * 60 * 60 * 1_000;

fn bounded_timer_deadline(now: Instant, logical_now: InstantMillis, at: InstantMillis) -> Instant {
    let delay = at.0.saturating_sub(logical_now.0).min(MAX_TIMER_ARM_MILLIS);
    now.checked_add(Duration::from_millis(delay)).unwrap_or(now)
}

fn retain_packet_phy(store: Option<&InterfaceStore>, bytes: &[u8], packet_phy: PacketPhyStats) {
    if packet_phy.is_empty() || WirePacketHeader::parse(bytes).is_err() {
        return;
    }
    let Some(store) = store else {
        return;
    };
    let Ok(packet_hash) = PacketHash::of_wire_packet(bytes) else {
        return;
    };
    store.remember_packet_phy(packet_hash, packet_phy);
}

pub struct TokioHost {
    base: Instant,
    logical_start: InstantMillis,
}

impl TokioHost {
    #[must_use]
    pub fn new() -> Self {
        Self::start_at(InstantMillis(0))
    }

    /// Mirrors `EmbassyTimebase::start_at`: the logical timeline resumes from `logical_start` instead of zero, so persisted timestamps stay in this boot's past.
    #[must_use]
    pub fn start_at(logical_start: InstantMillis) -> Self {
        Self {
            base: Instant::now(),
            logical_start,
        }
    }
}

impl Default for TokioHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for TokioHost {
    fn now(&self) -> InstantMillis {
        let elapsed = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX);
        InstantMillis(self.logical_start.0.saturating_add(elapsed))
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        loop {
            let remaining = deadline.0.saturating_sub(self.now().0);
            if remaining == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(remaining.min(MAX_TIMER_ARM_MILLIS))).await;
        }
    }

    #[allow(clippy::expect_used)]
    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        getrandom::getrandom(bytes).expect("OS CSPRNG must provide reactor entropy");
    }
}

/// The tokio side of one interface's seam: `next_inbound` frames funnel into the reactor's one
/// inbound stream (tagged with this interface's id), and `next_outbound` parks on this
/// interface's own outbound queue until the reactor enqueues a frame for it.
pub struct TokioInterfaceSeam {
    id: InterfaceId,
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
            inbound,
            notify,
            outbound,
            commands: None,
        }
    }

    #[must_use]
    pub fn with_commands(mut self, commands: UnboundedSender<HostCommand>) -> Self {
        self.commands = Some(commands);
        self
    }
}

impl InterfaceSeam for TokioInterfaceSeam {
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

pub struct Egress {
    lanes: std::vec::Vec<EgressLane>,
    #[cfg(feature = "runtime-metrics")]
    metrics: EgressMetricsSnapshot,
}

struct EgressLane {
    id: InterfaceId,
    #[cfg(feature = "runtime-metrics")]
    logical_interface: InterfaceId,
    producer: TokioGrantProducer,
    connection: Option<ConnectionView>,
}

impl EgressLane {
    fn is_available(&self) -> bool {
        self.connection
            .as_ref()
            .is_none_or(|connection| connection.connection().is_online())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EgressEnqueueOutcome {
    Enqueued,
    LaneFull,
    LaneMissing,
}

impl Egress {
    #[must_use]
    pub fn new(lanes: std::vec::Vec<(InterfaceId, TokioGrantProducer)>) -> Self {
        let lanes = lanes
            .into_iter()
            .map(|(id, producer)| EgressLane {
                id,
                #[cfg(feature = "runtime-metrics")]
                logical_interface: id,
                producer,
                connection: None,
            })
            .collect::<std::vec::Vec<_>>();
        #[cfg(feature = "runtime-metrics")]
        let metrics = {
            let mut metrics = EgressMetricsSnapshot::default();
            for lane in &lanes {
                metrics.announces.register_interface(lane.logical_interface);
            }
            metrics
        };
        Self {
            lanes,
            #[cfg(feature = "runtime-metrics")]
            metrics,
        }
    }

    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]) -> EgressEnqueueOutcome {
        for lane in &mut self.lanes {
            if lane.id == target {
                if let Some(slot) = lane.producer.try_grant() {
                    slot.fill(bytes);
                    lane.producer.commit();
                    #[cfg(feature = "runtime-metrics")]
                    {
                        self.metrics.enqueued_frames =
                            self.metrics.enqueued_frames.saturating_add(1);
                    }
                    return EgressEnqueueOutcome::Enqueued;
                } else {
                    #[cfg(feature = "runtime-metrics")]
                    {
                        self.metrics.full_lane_drops =
                            self.metrics.full_lane_drops.saturating_add(1);
                    }
                    return EgressEnqueueOutcome::LaneFull;
                }
            }
        }
        #[cfg(feature = "runtime-metrics")]
        {
            self.metrics.missing_lane_drops = self.metrics.missing_lane_drops.saturating_add(1);
        }
        EgressEnqueueOutcome::LaneMissing
    }

    fn skip_unavailable(&mut self, target: InterfaceId) -> bool {
        let unavailable = self
            .lanes
            .iter()
            .find(|lane| lane.id == target)
            .is_some_and(|lane| !lane.is_available());
        if unavailable {
            #[cfg(feature = "runtime-metrics")]
            {
                self.metrics.unavailable_frame_skips =
                    self.metrics.unavailable_frame_skips.saturating_add(1);
            }
        }
        unavailable
    }

    #[cfg(feature = "runtime-metrics")]
    fn record_announce(
        &mut self,
        target: InterfaceId,
        bytes: usize,
        origin: AnnounceOrigin,
        outcome: AnnounceEgressOutcome,
    ) {
        let logical_interface = self
            .lanes
            .iter()
            .find(|lane| lane.id == target)
            .map_or(target, |lane| lane.logical_interface);
        self.metrics
            .announces
            .record(origin, logical_interface, outcome, bytes);
    }

    /// The member interfaces a fleet broadcast targets: every lane of the supervisor's member kind
    /// that `fan` selects. The host owns a lane per member, so a broadcast fans out as one per-member
    /// send each — no shared-lane collision to dodge, unlike the embedded supervisor.
    fn broadcast_targets(
        &self,
        supervisor: InterfaceKind,
        fan: FanTarget,
    ) -> std::vec::Vec<InterfaceId> {
        let member = supervisor.member_kind();
        self.lanes
            .iter()
            .map(|lane| lane.id)
            .filter(|id| id.kind() == member)
            .filter(|id| match fan {
                FanTarget::All => true,
                FanTarget::Only(only) => *id == only,
                FanTarget::AllExcept(except) => *id != except,
            })
            .collect()
    }

    fn emit(
        &mut self,
        target: InterfaceId,
        size_hint: usize,
        fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
        discard: &mut [u8],
    ) {
        for lane in &mut self.lanes {
            if lane.id == target {
                match lane.producer.try_grant() {
                    Some(slot) => {
                        let hint = size_hint.clamp(1, MAX_WIRE_FRAME_LEN);
                        if slot.bytes.len() < hint {
                            slot.bytes.resize(hint, 0);
                        }
                        if let Some(len) = fill(&mut slot.bytes[..hint]) {
                            slot.len = len.min(hint);
                            lane.producer.commit();
                            #[cfg(feature = "runtime-metrics")]
                            {
                                self.metrics.enqueued_frames =
                                    self.metrics.enqueued_frames.saturating_add(1);
                            }
                        }
                    }
                    None => {
                        if fill(discard).is_some() {
                            #[cfg(feature = "runtime-metrics")]
                            {
                                self.metrics.full_lane_drops =
                                    self.metrics.full_lane_drops.saturating_add(1);
                            }
                        }
                    }
                }
                return;
            }
        }
        if fill(discard).is_some() {
            #[cfg(feature = "runtime-metrics")]
            {
                self.metrics.missing_lane_drops = self.metrics.missing_lane_drops.saturating_add(1);
            }
        }
    }

    fn add_lane(
        &mut self,
        id: InterfaceId,
        logical_interface: InterfaceId,
        producer: TokioGrantProducer,
        connection: Option<ConnectionView>,
    ) {
        #[cfg(not(feature = "runtime-metrics"))]
        let _ = logical_interface;
        self.lanes.push(EgressLane {
            id,
            #[cfg(feature = "runtime-metrics")]
            logical_interface,
            producer,
            connection,
        });
        #[cfg(feature = "runtime-metrics")]
        self.metrics.announces.register_interface(logical_interface);
    }

    fn remove_lane(&mut self, id: InterfaceId) {
        self.lanes.retain(|lane| lane.id != id);
    }

    #[cfg(feature = "runtime-metrics")]
    fn metrics_snapshot(&self, pacers: &[InterfacePacer]) -> EgressMetricsSnapshot {
        let mut snapshot = self.metrics.clone();
        snapshot.announces.reset_pacer_depths();
        for entry in pacers {
            snapshot
                .announces
                .add_pacer_depth(entry.logical_interface, entry.pacer.queued_len());
        }
        snapshot
    }
}

/// A cheap-clone handle to one interface's live state: the interface holds a clone and writes
/// it as the wire moves (connection on connect/disconnect, bytes as they cross); the app holds
/// a clone and reads it lock-free via [`InterfaceStatus`] on its own render cadence.
#[derive(Clone)]
pub struct TokioInterfaceStatus {
    inner: Arc<StatusCell>,
}

struct StatusCell {
    id: InterfaceId,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    airtime: AtomicU32,
    transfer_rates: AtomicU64,
    enabled: AtomicBool,
}

const AIRTIME_UNPUBLISHED: u32 = u32::MAX;
const RATES_UNPUBLISHED: u64 = u64::MAX;

fn pack_airtime(utilization: AirtimeUtilization) -> u32 {
    (u32::from(utilization.short_per_mille) << 16) | u32::from(utilization.long_per_mille)
}

fn unpack_airtime(packed: u32) -> Option<AirtimeUtilization> {
    if packed == AIRTIME_UNPUBLISHED {
        return None;
    }
    Some(AirtimeUtilization {
        short_per_mille: (packed >> 16) as u16,
        long_per_mille: packed as u16,
    })
}

impl TokioInterfaceStatus {
    #[must_use]
    pub fn new(id: InterfaceId, connection: ConnectionState) -> Self {
        Self {
            inner: Arc::new(StatusCell {
                id,
                connection: AtomicU8::new(connection.as_u8()),
                rx: AtomicU64::new(0),
                tx: AtomicU64::new(0),
                airtime: AtomicU32::new(AIRTIME_UNPUBLISHED),
                transfer_rates: AtomicU64::new(RATES_UNPUBLISHED),
                enabled: AtomicBool::new(true),
            }),
        }
    }

    /// Turn this interface off or back on from the application. The driver reads
    /// [`is_enabled`](Self::is_enabled) and tears its wires down — releasing any resource it holds,
    /// e.g. an open serial port — while off, standing them back up on resume.
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether the interface is enabled (the default). The driver polls this to leave or re-enter
    /// its dormant state.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    pub fn set_connection(&self, connection: ConnectionState) {
        self.inner
            .connection
            .store(connection.as_u8(), Ordering::Relaxed);
    }

    pub fn add_rx(&self, bytes: u64) {
        self.inner.rx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_tx(&self, bytes: u64) {
        self.inner.tx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn set_airtime(&self, utilization: AirtimeUtilization) {
        self.inner
            .airtime
            .store(pack_airtime(utilization), Ordering::Relaxed);
    }

    pub fn set_transfer_rates(&self, rates: TransferRates) {
        let packed = (u64::from(rates.rx_bps) << 32) | u64::from(rates.tx_bps);
        self.inner.transfer_rates.store(packed, Ordering::Relaxed);
    }
}

impl InterfaceStatus for TokioInterfaceStatus {
    fn id(&self) -> InterfaceId {
        self.inner.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            return ConnectionState::Disabled;
        }
        ConnectionState::from_u8(self.inner.connection.load(Ordering::Relaxed))
    }

    fn rx_bytes(&self) -> u64 {
        self.inner.rx.load(Ordering::Relaxed)
    }

    fn tx_bytes(&self) -> u64 {
        self.inner.tx.load(Ordering::Relaxed)
    }

    fn airtime(&self) -> Option<AirtimeUtilization> {
        unpack_airtime(self.inner.airtime.load(Ordering::Relaxed))
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        let packed = self.inner.transfer_rates.load(Ordering::Relaxed);
        if packed == RATES_UNPUBLISHED {
            return None;
        }
        Some(TransferRates {
            rx_bps: (packed >> 32) as u32,
            tx_bps: packed as u32,
        })
    }
}

/// What a std host feeds the reactor: the queueable engine commands, plus the owned-bytes
/// verbs that deliberately never ride `EngineCommand`. A resource payload can reach a
/// mebibyte, so the std host owns or shares the heap allocation and the engine borrows it for
/// exactly one call; an embedded host calls the borrow-taking entry points directly instead.
#[allow(clippy::large_enum_variant)]
pub enum HostCommand {
    Engine(IssuedCommand),
    /// An engine command whose settlement a caller is awaiting: the `completion` rides the
    /// command to the single reactor task, which stashes it keyed by `issued.id` and fires it
    /// when the matching `CommandSettled` is journaled, so the await-correlation registry has
    /// one owner and needs no lock.
    AwaitedEngine {
        issued: IssuedCommand,
        completion: oneshot::Sender<Settlement>,
    },
    SendResource(SendResourceHostCommand),
    SendResourceSegment(SendResourceSegmentHostCommand),
    RespondAny(RespondAnyHostCommand),
    RequestAny(RequestAnyHostCommand),
    ProvideDecompressed(ProvideDecompressedHostCommand),
    /// Register a runtime-added interface's descriptor and lanes so the reactor routes to it;
    /// its run loop is driven separately on the runtime's interface driver, only the `Send` lane halves cross.
    AddInterface(AddInterfaceCommand),
    /// Deregister the interface with this id: drop its descriptor and lanes so the reactor stops
    /// routing to it (its run loop is stopped separately on the interface driver).
    RemoveInterface {
        id: InterfaceId,
        departure: Departure,
    },
    SynthesizeTunnel {
        interface: InterfaceId,
    },
    /// Register a byte-stream reader's inbound sink: the run loop routes matching channel
    /// messages to it, suppressed from the app event stream. `ready` fires once the sink is in
    /// the routing table, so the opener can hold back the reader until no arriving chunk can slip past; awaited, not raced.
    RegisterStreamReader {
        link_id: LinkId,
        stream_id: StreamId,
        sink: UnboundedSender<StreamInbound>,
        ready: oneshot::Sender<()>,
    },
    /// Register a sink for the next inbound resource on this link: the run loop routes the
    /// resource's chunks to it and signals completion, suppressed from the app event stream.
    /// `ready` fires once registered, so a segment arriving the instant after cannot slip past to the app.
    RegisterResourceSink {
        link_id: LinkId,
        sink: UnboundedSender<ResourceInbound>,
        ready: oneshot::Sender<()>,
    },
    /// Set the default resource strategy for `destination`. `ready` carries back whether the
    /// destination was held, so a caller learns a misaddressed strategy rather than having it silently dropped.
    SetResourceStrategy {
        destination: DestinationHash,
        strategy: ResourceStrategy,
        ready: oneshot::Sender<bool>,
    },
    /// Serialize every persisted region on the reactor — the one place a consistent view exists — and
    /// hand the sealed images back; the caller owns the store IO, so flush cadence stays host policy.
    SnapshotPersistedState {
        reply: oneshot::Sender<PersistedStateSnapshot>,
    },
    /// Seal every tracked destination's self-ratchet record. Secrets ride these blobs, so they
    /// go to the caller's identity vault, never a `PersistedStore`.
    SnapshotSelfRatchets {
        reply: oneshot::Sender<SelfRatchetsSnapshot>,
    },
    #[cfg(feature = "runtime-metrics")]
    SnapshotMetrics {
        reply: oneshot::Sender<RuntimeMetricsSnapshot>,
    },
}

/// One sealed self-ratchet blob per tracked destination, zeroized on drop.
pub struct SelfRatchetsSnapshot {
    pub blobs: std::vec::Vec<(DestinationHash, Zeroizing<std::vec::Vec<u8>>)>,
}

/// The sealed region images of one snapshot pass and the engine instant they were taken at — the timebase high-water a flush of these images should record.
pub struct PersistedStateSnapshot {
    pub routing_table: std::vec::Vec<u8>,
    pub tunnels: std::vec::Vec<u8>,
    pub taken_at: InstantMillis,
}

/// One inbound stream chunk handed from the run loop's demux to a `ByteStreamReader`'s sink: the
/// payload bytes (owned, copied out of the borrowed reaction) with the eof and compressed flags.
pub struct StreamInbound {
    pub payload: std::vec::Vec<u8>,
    pub eof: bool,
    pub compressed: bool,
}

/// One inbound resource event handed from the run loop to a `receive_resource` future's sink:
/// a chunk of bytes, the completion marker carrying the assembled identity and total size, or
/// a failure. Owned, copied out of the borrowed reaction.
pub enum ResourceInbound {
    /// The transfer's packed metadata, arriving ahead of the first chunk when one traveled.
    Metadata(std::vec::Vec<u8>),
    Chunk(std::vec::Vec<u8>),
    Complete {
        original_hash: ResourceHash,
        total_size: u64,
    },
    Failed,
}

/// The payload of [`HostCommand::AddInterface`]: a runtime-added interface's descriptor and
/// the reactor's halves of its grant lanes. All `Send`, so the reactor stays `Send`; the
/// interface's `!Send` run future is driven on the runtime's interface driver, not here.
pub struct AddInterfaceCommand {
    pub descriptor: InterfaceDescriptor,
    pub logical_interface: InterfaceId,
    pub inbound: TokioGrantConsumer,
    pub egress: TokioGrantProducer,
    pub connection: Option<ConnectionView>,
    pub ifac: Option<crate::interfaces::ifac::IfacContext>,
}

/// Fire an awaited command's settlement to the caller parked on it, or pass the event through. The
/// reactor owns `pending` (a single-task `RefCell` map, no `Arc`/`Mutex`): a `CommandSettled` whose
/// id a caller awaits is handed to that caller's [`oneshot`] and dropped from the event stream;
/// every other journaled event — including a settlement nobody awaited — is returned for the app.
fn settle_or_forward<'a>(
    pending: &RefCell<HashMap<CommandId, oneshot::Sender<Settlement>>>,
    journaled: Journaled<'a>,
) -> Option<Journaled<'a>> {
    if let Journaled::CommandSettled { id, settlement } = &journaled {
        if let Some(completion) = pending.borrow_mut().remove(id) {
            let _ = completion.send(settlement.clone());
            return None;
        }
    }
    Some(journaled)
}

/// A `request()` parked on its answer: the bytes land first (stashed here), then the round trip
/// arrives with the settlement.
struct RequestPending {
    completion: oneshot::Sender<Result<(std::vec::Vec<u8>, RttMillis), SendRequestFailure>>,
    data: Option<std::vec::Vec<u8>>,
}

/// The outbound-request demux, mirror of [`settle_or_forward`]: a `ResponseReceived` with a
/// parked `request()` stashes its bytes (a split response's segments append, in the arrival
/// order the engine's gate enforces); the matching `SendRequest` settlement then fires
/// `(data, rtt)` or the typed failure to the awaiter and drops the events from the app stream.
fn route_request_or_forward<'a>(
    pending: &RefCell<HashMap<CommandId, RequestPending>>,
    journaled: Journaled<'a>,
) -> Option<Journaled<'a>> {
    match &journaled {
        Journaled::ResponseReceived {
            command_id, data, ..
        } => {
            if let Some(entry) = pending.borrow_mut().get_mut(command_id) {
                entry.data = Some(data.to_vec());
                return None;
            }
        }
        Journaled::ResponseSegmentReceived {
            command_id, data, ..
        } => {
            if let Some(entry) = pending.borrow_mut().get_mut(command_id) {
                entry
                    .data
                    .get_or_insert_with(std::vec::Vec::new)
                    .extend_from_slice(data);
                return None;
            }
        }

        Journaled::CommandSettled {
            id,
            settlement: Settlement::SendRequest(result),
        } => {
            if let Some(entry) = pending.borrow_mut().remove(id) {
                let resolved = match (*result, entry.data) {
                    (Ok(delivered), Some(data)) => Ok((data, delivered.rtt)),
                    (Ok(_), None) => Err(SendRequestFailure::WriteFailed),
                    (Err(failure), _) => Err(failure),
                };
                let _ = entry.completion.send(resolved);
                return None;
            }
        }
        _ => {}
    }
    Some(journaled)
}

/// Resolve a parked `request()` with a typed failure — for the request-forming paths the size
/// yardstick already rules out, so the awaiter never hangs even on the unreachable branch.
fn fail_request(
    pending: &RefCell<HashMap<CommandId, RequestPending>>,
    id: CommandId,
) -> WakeSchedules {
    if let Some(entry) = pending.borrow_mut().remove(&id) {
        let _ = entry.completion.send(Err(SendRequestFailure::WriteFailed));
    }
    WakeSchedules::UNCHANGED
}

/// The byte-stream demux: a `ChannelMessageReceived` of the reserved stream type whose `(link, id)`
/// has a registered reader is parsed, pushed to that reader's sink, and dropped from the event
/// stream; a reader whose receiver is gone is pruned. Everything else is returned for the app.
fn route_stream_or_forward<'a>(
    readers: &RefCell<HashMap<(LinkId, StreamId), UnboundedSender<StreamInbound>>>,
    journaled: Journaled<'a>,
) -> Option<Journaled<'a>> {
    if let Journaled::ChannelMessageReceived {
        link_id,
        message_type,
        data,
    } = &journaled
    {
        if *message_type == STREAM_DATA_TYPE {
            if let Ok(frame) = byte_stream::parse(data) {
                let key = (*link_id, frame.header.stream_id);
                let mut readers = readers.borrow_mut();
                if let Some(sink) = readers.get(&key) {
                    let inbound = StreamInbound {
                        payload: frame.payload.to_vec(),
                        eof: frame.header.eof,
                        compressed: frame.header.compressed,
                    };
                    if sink.send(inbound).is_err() {
                        readers.remove(&key);
                    }
                    return None;
                }
            }
        }
    }
    Some(journaled)
}

/// The resource mirror of [`route_stream_or_forward`]: an inbound resource journal on a link
/// with a registered `receive_resource` sink is routed to it and suppressed from the app event
/// stream. The sink is one-shot: it retires on the resource's completion or failure, leaving
/// the link free to register the next.
fn route_resource_or_forward<'a>(
    sinks: &RefCell<HashMap<LinkId, UnboundedSender<ResourceInbound>>>,
    journaled: Journaled<'a>,
) -> Option<Journaled<'a>> {
    let link = match &journaled {
        Journaled::ResourceReceived { link_id, .. }
        | Journaled::ResourceSegmentReceived { link_id, .. }
        | Journaled::ResourceAssembled { link_id, .. }
        | Journaled::ResourceFailed { link_id, .. } => *link_id,
        _ => return Some(journaled),
    };
    let sink = match sinks.borrow().get(&link) {
        Some(sink) => sink.clone(),
        None => return Some(journaled),
    };
    let retire = match &journaled {
        Journaled::ResourceReceived {
            hash,
            metadata,
            data,
            ..
        } => {
            if let Some(metadata) = metadata {
                let _ = sink.send(ResourceInbound::Metadata(metadata.to_vec()));
            }
            let _ = sink.send(ResourceInbound::Chunk(data.to_vec()));
            let _ = sink.send(ResourceInbound::Complete {
                original_hash: *hash,
                total_size: data.len() as u64,
            });
            true
        }
        Journaled::ResourceSegmentReceived { metadata, data, .. } => {
            if let Some(metadata) = metadata {
                let _ = sink.send(ResourceInbound::Metadata(metadata.to_vec()));
            }
            sink.send(ResourceInbound::Chunk(data.to_vec())).is_err()
        }
        Journaled::ResourceAssembled {
            original_hash,
            total_size,
            ..
        } => {
            let _ = sink.send(ResourceInbound::Complete {
                original_hash: *original_hash,
                total_size: *total_size,
            });
            true
        }
        Journaled::ResourceFailed { .. } => {
            let _ = sink.send(ResourceInbound::Failed);
            true
        }
        _ => unreachable!("the link only matched a resource journal above"),
    };
    if retire {
        sinks.borrow_mut().remove(&link);
    }
    None
}

#[derive(Debug)]
enum HostResourceStorage {
    Owned(std::vec::Vec<u8>),
    Shared(Arc<[u8]>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostResourcePayloadError {
    PrefixOutOfRange,
}

#[derive(Debug)]
pub struct HostResourcePayload {
    storage: HostResourceStorage,
    len: usize,
}

impl HostResourcePayload {
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        match &self.storage {
            HostResourceStorage::Owned(bytes) => &bytes[..self.len],
            HostResourceStorage::Shared(bytes) => &bytes[..self.len],
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn shared_prefix(bytes: Arc<[u8]>, len: usize) -> Result<Self, HostResourcePayloadError> {
        if len > bytes.len() {
            return Err(HostResourcePayloadError::PrefixOutOfRange);
        }
        Ok(Self {
            storage: HostResourceStorage::Shared(bytes),
            len,
        })
    }
}

impl AsRef<[u8]> for HostResourcePayload {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl From<std::vec::Vec<u8>> for HostResourcePayload {
    fn from(bytes: std::vec::Vec<u8>) -> Self {
        let len = bytes.len();
        Self {
            storage: HostResourceStorage::Owned(bytes),
            len,
        }
    }
}

impl From<Arc<[u8]>> for HostResourcePayload {
    fn from(bytes: Arc<[u8]>) -> Self {
        let len = bytes.len();
        Self {
            storage: HostResourceStorage::Shared(bytes),
            len,
        }
    }
}

/// The host half of [`ResourceMetadata`]: owned packed bytes crossing to the node thread.
pub enum HostResourceMetadata {
    None,
    /// This (first or only) segment carries the block in-stream.
    Packed(HostResourcePayload),
    /// A later segment of a split whose first segment carried the block.
    SentInFirstSegment {
        packed_len: u32,
    },
}

impl HostResourceMetadata {
    fn as_engine(&self) -> ResourceMetadata<'_> {
        match self {
            Self::None => ResourceMetadata::None,
            Self::Packed(payload) => ResourceMetadata::Packed(payload.as_slice()),
            Self::SentInFirstSegment { packed_len } => ResourceMetadata::SentInFirstSegment {
                packed_len: *packed_len,
            },
        }
    }
}

pub struct SendResourceHostCommand {
    pub id: CommandId,
    pub link_id: LinkId,
    pub data: HostResourcePayload,
    pub compressed_candidate: Option<HostResourcePayload>,
    pub metadata: HostResourceMetadata,
    pub request_id: Option<RequestId>,
}

/// One segment of a resource send, awaited: the `completion` rides the command to the reactor, which
/// stashes it keyed by `id` and fires it when the segment's proof settles — so a host `send_resource`
/// loop drains its source one segment at a time, sending the next only once the last is proven. The
/// engine threads the chain's original hash across segments; the host carries only the bytes.
pub struct SendResourceSegmentHostCommand {
    pub id: CommandId,
    pub link_id: LinkId,
    pub data: HostResourcePayload,
    pub compressed_candidate: Option<HostResourcePayload>,
    pub metadata: HostResourceMetadata,
    pub request_id: Option<RequestId>,
    pub segment_index: u64,
    pub total_segments: u64,
    pub total_data_size: u64,
    pub completion: oneshot::Sender<Settlement>,
}

/// Answer a request with `data` of any length: the engine picks the rung, a single RESPONSE
/// packet when it fits the link MDU, an outgoing resource (named back to `request_id`) when it
/// doesn't. Host-held payload, since a large answer never rides an enum; the request router's verb.
pub struct RespondAnyHostCommand {
    pub id: CommandId,
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub data: HostResourcePayload,
    pub compressed_candidate: Option<HostResourcePayload>,
}

/// Make a request of `path_hash` with `data` of any length: the reactor picks the rung and
/// fires `completion` with the response bytes and the round trip once the answer settles. The
/// payload is host-held like every owned-bytes verb.
pub struct RequestAnyHostCommand {
    pub id: CommandId,
    pub link_id: LinkId,
    pub path_hash: RequestPathHash,
    pub data: HostResourcePayload,
    pub completion: oneshot::Sender<Result<(std::vec::Vec<u8>, RttMillis), SendRequestFailure>>,
}

pub struct ProvideDecompressedHostCommand {
    pub link_id: LinkId,
    pub hash: ResourceHash,
    pub plaintext: HostResourcePayload,
}

use core::num::NonZeroUsize;

/// How the host runtime runs the engine's asymmetric crypto. `Pooled` offloads
/// verify/seal/sign/decrypt to worker threads and keeps the reactor hot; `Inline`
/// runs them on the reactor thread (the embedded shape, and the mobile default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoPoolConfig {
    Inline,
    Pooled { workers: PoolWorkers },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolWorkers {
    /// Size to the host: available parallelism minus reactor headroom (min 1).
    Auto,
    Fixed(NonZeroUsize),
}

impl CryptoPoolConfig {
    /// `Pooled`/`Auto` on a host that benefits; `Inline` on mobile targets, where
    /// the reactor stays single-threaded to protect battery.
    #[must_use]
    pub fn host_default() -> Self {
        if cfg!(any(target_os = "ios", target_os = "android")) {
            Self::Inline
        } else {
            Self::Pooled {
                workers: PoolWorkers::Auto,
            }
        }
    }

    fn with_env_override(self) -> Self {
        let workers_env = std::env::var("PRNS_CRYPTO_WORKERS")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .map(PoolWorkers::Fixed);
        match std::env::var("PRNS_CRYPTO_POOL")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("0" | "off" | "false" | "no") => Self::Inline,
            Some("") | None => match self {
                Self::Inline => Self::Inline,
                Self::Pooled { workers } => Self::Pooled {
                    workers: workers_env.unwrap_or(workers),
                },
            },
            Some(_) => Self::Pooled {
                workers: workers_env.unwrap_or(PoolWorkers::Auto),
            },
        }
    }
}

const REACTOR_IO_HEADROOM: usize = 2;
const MIN_POOL_WORKERS: usize = 4;

impl PoolWorkers {
    fn resolve(self) -> NonZeroUsize {
        match self {
            Self::Fixed(workers) => workers,
            Self::Auto => {
                let logical = std::thread::available_parallelism()
                    .map(NonZeroUsize::get)
                    .unwrap_or(6);
                let workers = match performance_cores() {
                    Some(performance) if performance < logical => performance
                        .saturating_sub(REACTOR_IO_HEADROOM)
                        .max(MIN_POOL_WORKERS),
                    _ => logical.saturating_sub(REACTOR_IO_HEADROOM).max(1),
                };
                NonZeroUsize::new(workers).unwrap_or(NonZeroUsize::MIN)
            }
        }
    }
}

fn performance_cores() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        linux_cpu_list_len("/sys/devices/cpu_core/cpus").or_else(linux_highest_capacity_cores)
    }
    #[cfg(target_os = "macos")]
    {
        macos_sysctl_usize("hw.perflevel0.logicalcpu")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_cpu_list_len(path: &str) -> Option<usize> {
    let raw = std::fs::read_to_string(path).ok()?;
    let count: usize = raw
        .trim()
        .split(',')
        .filter_map(|span| {
            let mut bounds = span
                .split('-')
                .filter_map(|n| n.trim().parse::<usize>().ok());
            let first = bounds.next()?;
            let last = bounds.next().unwrap_or(first);
            last.checked_sub(first).map(|range| range + 1)
        })
        .sum();
    (count > 0).then_some(count)
}

#[cfg(target_os = "linux")]
fn linux_highest_capacity_cores() -> Option<usize> {
    let logical = std::thread::available_parallelism().ok()?.get();
    let capacities: Vec<usize> = (0..logical)
        .filter_map(|cpu| {
            std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{cpu}/cpu_capacity"))
                .ok()
                .and_then(|raw| raw.trim().parse::<usize>().ok())
        })
        .collect();
    let highest = *capacities.iter().max()?;
    let count = capacities.iter().filter(|&&c| c == highest).count();
    (count < capacities.len()).then_some(count)
}

#[cfg(target_os = "macos")]
fn macos_sysctl_usize(name: &str) -> Option<usize> {
    let output = std::process::Command::new("sysctl")
        .arg("-n")
        .arg(name)
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

/// Everything the reactor is wired to for one run: the interface topology snapshot,
/// per-interface IFAC state, the wake and command channels, the inbound grant lanes, and the egress fan-out.
pub struct ReactorWiring {
    pub interfaces: std::vec::Vec<InterfaceDescriptor>,
    pub ifacs: std::vec::Vec<InterfaceIfac>,
    pub notify: UnboundedReceiver<InterfaceId>,
    pub inbound_lanes: std::vec::Vec<(InterfaceId, TokioGrantConsumer)>,
    pub commands: UnboundedReceiver<HostCommand>,
    pub egress: Egress,
}

pub async fn run<S, H, J>(engine: EngineState<S>, host: H, wiring: ReactorWiring, on_journaled: J)
where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
{
    run_with_deciders(
        engine,
        host,
        wiring,
        on_journaled,
        crate::reactor::decline_all(),
    )
    .await
}

struct EngineVerifyJob {
    packet_hash: PacketHash,
    signing_key: IdentitySigningPublicKey,
    signature: Ed25519Signature,
    id: CommandId,
    settlement: Settlement,
}

/// A deferred staged-continuation seal: everything [`seal_staged_resource`] needs, copied off the engine so the row can park as `StagedSealing` while a worker runs the crypto.
/// The salts are pre-drawn because workers carry no entropy source; [`SALT_REROLL_CAP`] of them is exactly the attempt budget.
struct StagedSealJob {
    link_id: LinkId,
    key: LinkKey,
    sdu: usize,
    nonce_prefixed_len: usize,
    plaintext: Vec<u8>,
    seal_iv: [u8; 16],
    salts: [[u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP],
}

/// One deferred chew of an incoming transfer's newly-contiguous span: the streamed-open state rides with a copy of the ciphertext, decrypts it in place on the worker, and both come home in the verdict.
/// No key travels — the state already carries every midstate the chew needs.
struct OpenSpanJob {
    link_id: LinkId,
    hash: ResourceHash,
    span_start: usize,
    state: StreamedOpen,
    bytes: Vec<u8>,
}

/// One unit of asymmetric crypto handed off the engine thread (one saturated pool, not one per
/// op): `Verify` is an inbound proof's Ed25519 check, `SealScalars` the two X25519 scalar
/// mults an outbound single's seal needs, `Sign` the Ed25519 signature an inbound delivery's
/// proof owes. The seal job carries the whole obligation so the reactor keeps no per-in-flight
/// side table; it is transient and its large variant is the common one, so it is not boxed.
#[allow(clippy::large_enum_variant)]
enum CryptoJob {
    Verify(EngineVerifyJob),
    SealStaged(Box<StagedSealJob>),
    OpenSpan(Box<OpenSpanJob>),
    SealScalars(EncryptOwed),
    Sign(DeferredProofSign),
    Decrypt(DecryptOwed),
    DecryptWithRatchets(Box<RatchetDecryptOwed>),
    VerifyLinkProof(LinkProofVerifyOwed),
    SignLinkProof(LinkProofSignOwed),
    VerifyAnnounce(AnnounceVerifyOwed),
}

impl CryptoJob {
    fn owes_packet_verdict(&self) -> bool {
        !matches!(self, Self::SealStaged(_))
    }
}

/// What a worker hands back to the reactor thread, where the engine-state
/// mutation it gates runs: `Verified` settles the proof's receipt, `Sealed`
/// finishes the single's frame + receipt track from the returned scalars.
#[allow(clippy::large_enum_variant)]
enum CryptoResult {
    Verified {
        id: CommandId,
        settlement: Settlement,
        valid: bool,
    },
    Sealed {
        owed: EncryptOwed,
        ephemeral_public: X25519PublicKey,
        shared: X25519SharedSecret,
    },
    Signed {
        target: InterfaceId,
        packet_hash: PacketHash,
        signature: Ed25519Signature,
    },
    Decrypted {
        owed: DecryptOwed,
        shared: X25519SharedSecret,
    },
    RatchetDecrypted {
        owed: Box<RatchetDecryptOwed>,
        opened: Option<(OpenedBy, HeaplessVec<u8, MAX_RATCHET_DECRYPT_PAYLOAD_LEN>)>,
    },
    LinkProofVerified {
        owed: LinkProofVerifyOwed,
        shared: Option<X25519SharedSecret>,
    },
    LinkProofSigned {
        owed: LinkProofSignOwed,
        responder_encryption: X25519PublicKey,
        shared: X25519SharedSecret,
        signature: Ed25519Signature,
    },
    AnnounceVerified {
        owed: AnnounceVerifyOwed,
        valid: bool,
    },
    StagedSealed {
        link_id: LinkId,
        stream_nonce: [u8; RESOURCE_NONCE_LEN],
        nonce_prefixed_len: usize,
        transfer: Vec<u8>,
        names: Vec<u8>,
        outcome: Result<SealedStagedResource, BuildOutgoingResourceError>,
    },
    SpanOpened {
        link_id: LinkId,
        hash: ResourceHash,
        span_start: usize,
        state: StreamedOpen,
        bytes: Vec<u8>,
    },
}

impl CryptoResult {
    fn settles_packet_verdict(&self) -> bool {
        !matches!(self, Self::StagedSealed { .. })
    }
}

struct CryptoQueue {
    jobs: Mutex<VecDeque<CryptoJob>>,
    len: AtomicUsize,
    backpressure_depth: usize,
    ready: Condvar,
    shutdown: AtomicBool,
}

struct CryptoPool {
    queue: Arc<CryptoQueue>,
    workers: Vec<std::thread::JoinHandle<()>>,
    #[cfg(feature = "runtime-metrics")]
    submitted_jobs: Cell<u64>,
    #[cfg(feature = "runtime-metrics")]
    completed_jobs: Cell<u64>,
    #[cfg(feature = "runtime-metrics")]
    maximum_queue_depth: Cell<usize>,
    #[cfg(feature = "runtime-metrics")]
    backpressure_deferrals: Cell<u64>,
    /// In-flight jobs whose verdict gates packet work; while any is owed the reactor's yield arm keeps spinning, so the verdict lands without paying a park/unpark round trip.
    /// A wall-clock hot window sat here before and let the reactor park whenever a slow worker wake outran the window with the verdict still in flight — under tokio's paused test clock that early park auto-advanced time straight past whatever the verdict was gating.
    /// Gating on owed work instead makes an idle runtime mean a truly quiet pool.
    /// Staged seals go uncounted: nothing on the packet path waits for one, and spinning through a multi-millisecond seal burns a core for zero wall gain — that verdict's channel send wakes the reactor like any other event.
    packet_verdicts_owed: Cell<usize>,
    /// Stamped at every packet-verdict submit and settle; the yield arm stays hot for [`Self::PACKET_VERDICT_LINGER`] past the newest stamp.
    last_packet_verdict_event: Cell<Option<std::time::Instant>>,
}

impl CryptoPool {
    /// How long the yield arm outlives the last packet-verdict event.
    /// A saturated verdict stream still has transient owed=0 instants (every verdict drained, the next submit microseconds away); parking there pays a deep-idle unpark per gap, which halved firehose throughput and produced the 60-90 gate timeouts per run that a purely owed-gated spin showed.
    /// Any bridge from 50µs up restored both in full on the bench host; 200µs is margin, still 25× shorter than the wall-clock hot window this linger replaced.
    const PACKET_VERDICT_LINGER: Duration = Duration::from_micros(200);

    fn spawn(workers: usize, results: UnboundedSender<CryptoResult>) -> Option<Self> {
        let queue = Arc::new(CryptoQueue {
            jobs: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
            backpressure_depth: crypto_backpressure_depth(workers),
            ready: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let mut handles = Vec::with_capacity(workers.max(1));
        for _ in 0..workers.max(1) {
            let worker_queue = queue.clone();
            let worker_results = results.clone();
            match std::thread::Builder::new()
                .spawn(move || crypto_worker(&worker_queue, &worker_results))
            {
                Ok(handle) => handles.push(handle),
                Err(_) => {
                    queue.shutdown.store(true, Ordering::Release);
                    queue.ready.notify_all();
                    for worker in handles {
                        let _ = worker.join();
                    }
                    return None;
                }
            }
        }
        Some(Self {
            queue,
            workers: handles,
            #[cfg(feature = "runtime-metrics")]
            submitted_jobs: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            completed_jobs: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            maximum_queue_depth: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            backpressure_deferrals: Cell::new(0),
            packet_verdicts_owed: Cell::new(0),
            last_packet_verdict_event: Cell::new(None),
        })
    }

    fn submit(&self, job: CryptoJob) {
        let queue = &*self.queue;
        if let Ok(mut jobs) = queue.jobs.lock() {
            if job.owes_packet_verdict() {
                self.packet_verdicts_owed
                    .set(self.packet_verdicts_owed.get().saturating_add(1));
                self.last_packet_verdict_event
                    .set(Some(std::time::Instant::now()));
            }
            jobs.push_back(job);
            #[cfg(feature = "runtime-metrics")]
            let queue_depth = queue.len.fetch_add(1, Ordering::Release).saturating_add(1);
            #[cfg(not(feature = "runtime-metrics"))]
            let _ = queue.len.fetch_add(1, Ordering::Release);
            #[cfg(feature = "runtime-metrics")]
            {
                self.submitted_jobs
                    .set(self.submitted_jobs.get().saturating_add(1));
                self.maximum_queue_depth
                    .set(self.maximum_queue_depth.get().max(queue_depth));
            }
            drop(jobs);
            queue.ready.notify_one();
        }
    }

    fn has_queue_capacity(&self, additional: usize) -> bool {
        let has_capacity = self
            .queue
            .len
            .load(Ordering::Acquire)
            .saturating_add(additional)
            <= self.queue.backpressure_depth;
        #[cfg(feature = "runtime-metrics")]
        if !has_capacity {
            self.backpressure_deferrals
                .set(self.backpressure_deferrals.get().saturating_add(1));
        }
        has_capacity
    }

    fn awaits_packet_verdict(&self) -> bool {
        self.packet_verdicts_owed.get() > 0
            || self
                .last_packet_verdict_event
                .get()
                .is_some_and(|at| at.elapsed() < Self::PACKET_VERDICT_LINGER)
    }

    fn packet_verdict_settled(&self) {
        let owed = self.packet_verdicts_owed.get();
        debug_assert!(owed > 0, "a packet verdict landed that no submit counted");
        self.packet_verdicts_owed.set(owed.saturating_sub(1));
        self.last_packet_verdict_event
            .set(Some(std::time::Instant::now()));
    }

    #[cfg(feature = "runtime-metrics")]
    fn record_completed(&self) {
        self.completed_jobs
            .set(self.completed_jobs.get().saturating_add(1));
    }

    #[cfg(feature = "runtime-metrics")]
    fn metrics_snapshot(&self) -> CryptoMetricsSnapshot {
        CryptoMetricsSnapshot {
            submitted_jobs: self.submitted_jobs.get(),
            completed_jobs: self.completed_jobs.get(),
            queue_depth: bounded_u32(self.queue.len.load(Ordering::Acquire)),
            maximum_queue_depth: bounded_u32(self.maximum_queue_depth.get()),
            backpressure_deferrals: self.backpressure_deferrals.get(),
            packet_verdicts_owed: bounded_u32(self.packet_verdicts_owed.get()),
        }
    }
}

#[cfg(feature = "runtime-metrics")]
fn bounded_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

impl Drop for CryptoPool {
    fn drop(&mut self) {
        self.queue.shutdown.store(true, Ordering::Release);
        let mut jobs = self
            .queue
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        jobs.clear();
        self.queue.len.store(0, Ordering::Release);
        drop(jobs);
        self.queue.ready.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

const CRYPTO_QUEUE_PER_WORKER: usize = 4;
const MIN_CRYPTO_QUEUE_DEPTH: usize = 16;
const MAX_CRYPTO_QUEUE_DEPTH: usize = 64;

fn crypto_backpressure_depth(workers: usize) -> usize {
    workers
        .saturating_mul(CRYPTO_QUEUE_PER_WORKER)
        .clamp(MIN_CRYPTO_QUEUE_DEPTH, MAX_CRYPTO_QUEUE_DEPTH)
}

fn run_crypto_job(job: CryptoJob) -> CryptoResult {
    match job {
        CryptoJob::SealStaged(job) => {
            let StagedSealJob {
                link_id,
                key,
                sdu,
                nonce_prefixed_len,
                plaintext,
                seal_iv,
                salts,
            } = *job;
            let mut stream_nonce = [0u8; RESOURCE_NONCE_LEN];
            stream_nonce.copy_from_slice(&plaintext[16..16 + RESOURCE_NONCE_LEN]);
            let stream_len = nonce_prefixed_len - RESOURCE_NONCE_LEN;
            let mut transfer = plaintext;
            transfer.resize(sealed_transfer_len(stream_len), 0);
            let mut names = vec![0u8; transfer.len().div_ceil(sdu) * MAP_HASH_LEN];
            let mut fresh_salts = salts.into_iter();
            let outcome = seal_staged_resource(
                &key,
                &seal_iv,
                || fresh_salts.next().unwrap_or_default(),
                sdu,
                nonce_prefixed_len,
                BuildRegions {
                    transfer: &mut transfer,
                    hashmap: &mut names,
                },
            );
            CryptoResult::StagedSealed {
                link_id,
                stream_nonce,
                nonce_prefixed_len,
                transfer,
                names,
                outcome,
            }
        }
        CryptoJob::OpenSpan(job) => {
            let OpenSpanJob {
                link_id,
                hash,
                span_start,
                mut state,
                mut bytes,
            } = *job;
            state.chew_span(&mut bytes);
            CryptoResult::SpanOpened {
                link_id,
                hash,
                span_start,
                state,
                bytes,
            }
        }
        CryptoJob::Verify(job) => {
            let valid = Ed25519Verifier::new(job.signing_key.as_ed25519())
                .map(|verifier| {
                    verifier
                        .verify(job.packet_hash.as_bytes(), &job.signature)
                        .is_ok()
                })
                .unwrap_or(false);
            CryptoResult::Verified {
                id: job.id,
                settlement: job.settlement,
                valid,
            }
        }
        CryptoJob::SealScalars(owed) => {
            let (ephemeral_public, shared) =
                x25519_keys_for_seal(&owed.ephemeral_secret, &owed.dh_target);
            CryptoResult::Sealed {
                owed,
                ephemeral_public,
                shared,
            }
        }
        CryptoJob::Sign(job) => {
            let signature = ed25519_sign(&job.signing_secret, job.packet_hash.as_bytes());
            CryptoResult::Signed {
                target: job.target,
                packet_hash: job.packet_hash,
                signature,
            }
        }
        CryptoJob::Decrypt(owed) => {
            let shared = x25519_diffie_hellman(&owed.encryption_secret, &owed.ephemeral_public);
            CryptoResult::Decrypted { owed, shared }
        }
        CryptoJob::DecryptWithRatchets(mut owed) => {
            let opened = decrypt_token_in_place_with_ratchets(
                &owed.ratchet_secrets,
                &owed.encryption_secret,
                &owed.identity,
                owed.identity_key_fallback,
                &mut owed.token,
            )
            .ok()
            .map(|opened| {
                let mut buf = HeaplessVec::new();
                let _ = buf.extend_from_slice(opened.plaintext);
                (opened.opened_by, buf)
            });
            CryptoResult::RatchetDecrypted { owed, opened }
        }
        CryptoJob::VerifyLinkProof(owed) => {
            let shared = link_proof_signature_valid(&owed)
                .then(|| x25519_diffie_hellman(&owed.initiator_secret, &owed.responder_encryption));
            CryptoResult::LinkProofVerified { owed, shared }
        }
        CryptoJob::SignLinkProof(owed) => {
            let (responder_encryption, shared) =
                x25519_keys_for_seal(&owed.ephemeral_secret, &owed.request.initiator_encryption);
            let signed_data = link_proof_signed_data(
                &owed.request.link_id,
                &responder_encryption,
                owed.responder_signing.as_ed25519(),
                owed.mtu,
                owed.request.mode,
            );
            let signature = ed25519_sign(&owed.signing_secret, &signed_data);
            CryptoResult::LinkProofSigned {
                owed,
                responder_encryption,
                shared,
                signature,
            }
        }
        CryptoJob::VerifyAnnounce(owed) => {
            let valid = Announce::from_wire_unverified(&owed.header, &owed.payload)
                .is_ok_and(|announce| announce.signature_is_valid());
            CryptoResult::AnnounceVerified { owed, valid }
        }
    }
}

fn crypto_worker(queue: &CryptoQueue, results: &UnboundedSender<CryptoResult>) {
    let Ok(mut jobs) = queue.jobs.lock() else {
        return;
    };
    loop {
        if queue.shutdown.load(Ordering::Acquire) {
            return;
        }
        match jobs.pop_front() {
            Some(job) => {
                queue.len.fetch_sub(1, Ordering::Release);
                drop(jobs);
                if results.send(run_crypto_job(job)).is_err() {
                    return;
                }
                let Ok(relocked) = queue.jobs.lock() else {
                    return;
                };
                jobs = relocked;
            }
            None => {
                let Ok(waited) = queue.ready.wait(jobs) else {
                    return;
                };
                jobs = waited;
            }
        }
    }
}

pub async fn run_with_deciders<S, H, J, P, A>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring,
    on_journaled: J,
    deciders: AppDeciders<P, A>,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        deciders,
        None,
        CryptoPoolConfig::host_default(),
    )
    .await
}

pub async fn run_with_store<S, H, J>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring,
    on_journaled: J,
    store: InterfaceStore,
    crypto_pool_config: CryptoPoolConfig,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
{
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        crate::reactor::decline_all(),
        Some(store),
        crypto_pool_config,
    )
    .await
}

async fn run_inner<S, H, J, P, A>(
    mut engine: EngineState<S>,
    mut host: H,
    wiring: ReactorWiring,
    mut on_journaled: J,
    deciders: AppDeciders<P, A>,
    store: Option<InterfaceStore>,
    crypto_pool_config: CryptoPoolConfig,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    let AppDeciders {
        mut should_prove,
        mut should_accept_resource,
    } = deciders;
    let ReactorWiring {
        interfaces,
        mut ifacs,
        mut notify,
        mut inbound_lanes,
        mut commands,
        mut egress,
    } = wiring;
    let mut interfaces = IndexedAttachedInterfaces::from(interfaces);
    for descriptor in interfaces.descriptors() {
        #[cfg(feature = "runtime-metrics")]
        engine.attach_metrics_interface(descriptor.id, descriptor.id);
        engine.interface_attached(descriptor.id, host.now());
    }
    let mut wake_schedules = engine.wake_schedules(interfaces.view());
    let mut pacers: std::vec::Vec<InterfacePacer> = interfaces
        .descriptors()
        .iter()
        .map(|descriptor| InterfacePacer {
            id: descriptor.id,
            #[cfg(feature = "runtime-metrics")]
            logical_interface: descriptor.id,
            pacer: AnnouncePacer::new(descriptor.announce_bandwidth_cap, descriptor.bitrate),
        })
        .collect();
    let mut scratch_cap = interfaces
        .descriptors()
        .iter()
        .map(frame_cap_for)
        .max()
        .unwrap_or(BROADCAST_WIRE_FRAME_LEN);
    let mut wire_scratch = WireScratch::new(scratch_cap);
    let mut unmask_scratch = std::vec![0u8; scratch_cap].into_boxed_slice();
    let pending_completions: RefCell<HashMap<CommandId, oneshot::Sender<Settlement>>> =
        RefCell::new(HashMap::new());
    let pending_responses: RefCell<HashMap<CommandId, RequestPending>> =
        RefCell::new(HashMap::new());
    let stream_readers: RefCell<HashMap<(LinkId, StreamId), UnboundedSender<StreamInbound>>> =
        RefCell::new(HashMap::new());
    let resource_sinks: RefCell<HashMap<LinkId, UnboundedSender<ResourceInbound>>> =
        RefCell::new(HashMap::new());
    #[cfg(feature = "runtime-metrics")]
    let mut reliability = ReliabilityMetricsSnapshot::default();
    macro_rules! journaled_sink {
        () => {
            |journaled: Journaled<'_>| {
                #[cfg(feature = "runtime-metrics")]
                reliability.record_journaled(&journaled);
                if let Some(journaled) = settle_or_forward(&pending_completions, journaled) {
                    if let Some(journaled) = route_request_or_forward(&pending_responses, journaled)
                    {
                        if let Some(journaled) = route_stream_or_forward(&stream_readers, journaled)
                        {
                            if let Some(journaled) =
                                route_resource_or_forward(&resource_sinks, journaled)
                            {
                                on_journaled(journaled);
                            }
                        }
                    }
                }
            }
        };
    }
    macro_rules! defer_send_single_packet {
        ($pool:expr, $id:expr, $send:expr, $now:expr) => {{
            let mut entropy_bytes = [0u8; SendSinglePacketEntropy::LEN];
            host.fill_entropy(&mut entropy_bytes);
            match engine.prepare_send_single_packet_deferred(
                $id,
                $send,
                $now,
                SendSinglePacketEntropy::new(entropy_bytes),
            ) {
                SendSinglePacketPrepared::Owed(owed) => {
                    $pool.submit(CryptoJob::SealScalars(owed));
                }
                SendSinglePacketPrepared::Rejected { id, rejection } => {
                    route_reaction(
                        EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::SendSinglePacket(Err(
                                SendSinglePacketFailure::Rejected(rejection),
                            )),
                        }),
                        &mut egress,
                        &ifacs,
                        &mut pacers,
                        &mut wire_scratch,
                        $now,
                        &mut journaled_sink!(),
                    );
                }
                SendSinglePacketPrepared::RouteVanished { id } => {
                    route_reaction(
                        EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::SendSinglePacket(Err(
                                SendSinglePacketFailure::WriteFailed(
                                    SendSinglePacketWriteError::RouteVanished,
                                ),
                            )),
                        }),
                        &mut egress,
                        &ifacs,
                        &mut pacers,
                        &mut wire_scratch,
                        $now,
                        &mut journaled_sink!(),
                    );
                }
            }
            WakeSchedules::UNCHANGED
        }};
    }
    const MAX_INBOUND_BATCH: usize = 64;
    const MAX_COMMAND_BATCH: usize = 64;
    let (crypto_tx, mut crypto_rx) = tokio::sync::mpsc::unbounded_channel::<CryptoResult>();
    let crypto_pool = match crypto_pool_config.with_env_override() {
        CryptoPoolConfig::Inline => None,
        CryptoPoolConfig::Pooled { workers } => {
            CryptoPool::spawn(workers.resolve().get(), crypto_tx.clone())
        }
    };
    let _crypto_tx = crypto_tx;
    if crypto_pool.is_some() {
        engine.resource_open_lane = ResourceOpenLane::PoolWhenContended;
    }
    let mut dirty: std::vec::Vec<InterfaceId> = std::vec::Vec::new();
    let due_timer = tokio::time::sleep_until(Instant::now());
    tokio::pin!(due_timer);
    let mut armed: Option<(InstantMillis, WakeReason)> = None;
    let pacer_timer = tokio::time::sleep_until(Instant::now());
    tokio::pin!(pacer_timer);
    let mut pacer_armed: Option<InstantMillis> = None;
    loop {
        match soonest_pacer_release(&pacers) {
            None => pacer_armed = None,
            Some(at) => {
                if pacer_armed != Some(at) {
                    pacer_timer.as_mut().reset(bounded_timer_deadline(
                        Instant::now(),
                        host.now(),
                        at,
                    ));
                }
                pacer_armed = Some(at);
            }
        }
        match wake_schedules.soonest(host.now()) {
            NextWake::Idle => armed = None,
            NextWake::Due(reason) => {
                due_timer.as_mut().reset(Instant::now());
                armed = Some((InstantMillis(0), reason));
            }
            NextWake::At { at, reason } => {
                if armed.map(|(deadline, _)| deadline) != Some(at) {
                    due_timer.as_mut().reset(bounded_timer_deadline(
                        Instant::now(),
                        host.now(),
                        at,
                    ));
                }
                armed = Some((at, reason));
            }
        }
        /// Walk every owed receive-side chew into the pool: copy the span, move the state, submit.
        /// Unlike staged seals, a span counts as a packet verdict — it sits on the transfer's critical path (the proof is gated on it), which is exactly the latency the owed spin exists to cut.
        macro_rules! dispatch_owed_open_spans {
            () => {{
                if let Some(pool) = crypto_pool.as_ref() {
                    while let Some((link_id, hash)) = engine.owed_open_span() {
                        if !pool.has_queue_capacity(1) {
                            break;
                        }
                        let Some(view) = engine.open_span_job_view(&link_id, &hash) else {
                            break;
                        };
                        let span_start = view.span_start;
                        let bytes = view.bytes.to_vec();
                        let Some(state) = engine.begin_open_chew(&link_id, &hash) else {
                            break;
                        };
                        pool.submit(CryptoJob::OpenSpan(Box::new(OpenSpanJob {
                            link_id,
                            hash,
                            span_start,
                            state,
                            bytes,
                        })));
                    }
                }
            }};
        }
        /// One bounded sweep over every lane in `dirty`: up to `MAX_INBOUND_BATCH` frames per
        /// lane, then `dirty` retains the lanes still holding frames. The retain is the strand
        /// guard: entering a sweep consumes the lanes' queued notifications, so a lane deeper
        /// than one batch has no commit left to re-announce its tail — the backlog-gated yield
        /// arm below re-enters this sweep instead of letting the reactor park on stranded frames.
        macro_rules! process_dirty_lanes {
            () => {{
                let now = host.now();
                for &source in &dirty {
                    let Some((_, lane)) = inbound_lanes.iter_mut().find(|(id, _)| *id == source)
                    else {
                        continue;
                    };
                    lane.acknowledge();
                    for _ in 0..MAX_INBOUND_BATCH {
                        if crypto_pool
                            .as_ref()
                            .is_some_and(|pool| !pool.has_queue_capacity(2))
                        {
                            break;
                        }
                        let Some(slot) = lane.try_peek() else { break };
                        let packet_phy = slot.packet_phy;
                        let bytes = match ifac_for(&ifacs, source) {
                            Some(entry) => {
                                let Some(clean_len) = entry
                                    .context
                                    .unmask_inbound(slot.frame(), &mut unmask_scratch)
                                else {
                                    lane.release();
                                    continue;
                                };
                                &mut unmask_scratch[..clean_len]
                            }
                            None => slot.frame_mut(),
                        };
                        retain_packet_phy(store.as_ref(), bytes, packet_phy);
                        if let Some(pool) = &crypto_pool {
                            if let Ok((header, payload)) = WirePacketHeader::parse(bytes) {
                                if header.packet_type == PacketType::Proof {
                                    if let Some(deferred) = engine.settle_receipt_proof_deferred(
                                        payload,
                                        &DestinationHash::from_address(header.address),
                                        now,
                                    ) {
                                        let settle = match deferred.ingest {
                                            ProofIngest::SendSinglePacketDelivered {
                                                id,
                                                delivered,
                                            } => Some((
                                                id,
                                                Settlement::SendSinglePacket(Ok(delivered)),
                                            )),
                                            ProofIngest::SendToLinkDelivered { id, delivered } => {
                                                Some((id, Settlement::SendToLink(Ok(delivered))))
                                            }
                                            _ => None,
                                        };
                                        if let Some((id, settlement)) = settle {
                                            pool.submit(CryptoJob::Verify(EngineVerifyJob {
                                                packet_hash: deferred.packet_hash,
                                                signing_key: deferred.signing_key,
                                                signature: deferred.signature,
                                                id,
                                                settlement,
                                            }));
                                        }
                                        lane.release();
                                        continue;
                                    }
                                }
                            }
                        }
                        let packet = InboundPacket {
                            arrived_at: now,
                            source_interface: source,
                            bytes,
                        };
                        let wake_schedules_delta = match &crypto_pool {
                            Some(pool) => {
                                let mut deferred_sign = None;
                                let mut deferred = DeferredCrypto::default();
                                let delta = engine.ingest_packet_into_deferring(
                                    packet,
                                    IngestIo {
                                        interfaces: interfaces.view(),
                                        now,
                                        fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                                        should_prove: &mut should_prove,
                                        should_accept_resource: &mut should_accept_resource,
                                        sink: &mut |reaction| {
                                            route_reaction(
                                                reaction,
                                                &mut egress,
                                                &ifacs,
                                                &mut pacers,
                                                &mut wire_scratch,
                                                now,
                                                &mut journaled_sink!(),
                                            )
                                        },
                                    },
                                    &mut deferred_sign,
                                    Some(&mut deferred),
                                );
                                if let Some(owed) = deferred_sign {
                                    pool.submit(CryptoJob::Sign(owed));
                                }
                                match deferred {
                                    DeferredCrypto::Empty => {}
                                    DeferredCrypto::Decrypt(owed) => {
                                        pool.submit(CryptoJob::Decrypt(owed));
                                    }
                                    DeferredCrypto::RatchetDecrypt(owed) => {
                                        pool.submit(CryptoJob::DecryptWithRatchets(Box::new(owed)));
                                    }
                                    DeferredCrypto::LinkProofVerify(owed) => {
                                        pool.submit(CryptoJob::VerifyLinkProof(owed));
                                    }
                                    DeferredCrypto::LinkProofSign(owed) => {
                                        pool.submit(CryptoJob::SignLinkProof(owed));
                                    }
                                    DeferredCrypto::AnnounceVerify(owed) => {
                                        pool.submit(CryptoJob::VerifyAnnounce(owed));
                                    }
                                }
                                delta
                            }
                            None => engine.ingest_packet_into(
                                packet,
                                IngestIo {
                                    interfaces: interfaces.view(),
                                    now,
                                    fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                                    should_prove: &mut should_prove,
                                    should_accept_resource: &mut should_accept_resource,
                                    sink: &mut |reaction| {
                                        route_reaction(
                                            reaction,
                                            &mut egress,
                                            &ifacs,
                                            &mut pacers,
                                            &mut wire_scratch,
                                            now,
                                            &mut journaled_sink!(),
                                        )
                                    },
                                },
                            ),
                        };
                        lane.release();
                        merge_wake_schedules_delta(
                            &mut wake_schedules,
                            wake_schedules_delta,
                            &engine,
                            interfaces.view(),
                        );
                        dispatch_owed_open_spans!();
                    }
                }
                dirty.retain(|source| {
                    inbound_lanes
                        .iter_mut()
                        .find(|(id, _)| id == source)
                        .is_some_and(|(_, lane)| lane.try_peek().is_some())
                });
            }};
        }
        tokio::select! {
            arrived = notify.recv() => {
                let Some(source) = arrived else { return };
                if !dirty.contains(&source) {
                    dirty.push(source);
                }
                while let Ok(more) = notify.try_recv() {
                    if !dirty.contains(&more) {
                        dirty.push(more);
                    }
                }
                process_dirty_lanes!();
            }
            _ = tokio::task::yield_now(), if !dirty.is_empty() => {
                while let Ok(more) = notify.try_recv() {
                    if !dirty.contains(&more) {
                        dirty.push(more);
                    }
                }
                process_dirty_lanes!();
            }
            _ = tokio::task::yield_now(), if dirty.is_empty() && engine.owed_staged_seal_link().is_some() => {
                if let Some(link_id) = engine.owed_staged_seal_link() {
                    match crypto_pool.as_ref() {
                        Some(pool) => {
                            if let Some(view) = engine.staged_seal_job_view(&link_id) {
                                let mut seal_iv = [0u8; 16];
                                host.fill_entropy(&mut seal_iv);
                                let mut salts = [[0u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP];
                                for salt in &mut salts {
                                    host.fill_entropy(salt);
                                }
                                let job = StagedSealJob {
                                    link_id,
                                    key: view.key.cloned(),
                                    sdu: view.sdu,
                                    nonce_prefixed_len: view.nonce_prefixed_len,
                                    plaintext: view.plaintext.to_vec(),
                                    seal_iv,
                                    salts,
                                };
                                engine.mark_staged_sealing(&link_id);
                                pool.submit(CryptoJob::SealStaged(Box::new(job)));
                            }
                        }
                        None => {
                            let now = host.now();
                            engine.seal_staged_continuation(
                                &link_id,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                        }
                    }
                }
            }
            issued = commands.recv() => {
                let Some(mut issued) = issued else { return };
                let now = host.now();
                let mut command_budget = MAX_COMMAND_BATCH;
                loop {
                let wake_schedules_delta = match issued {
                    HostCommand::Engine(issued) => {
                        let id = issued.id;
                        match (crypto_pool.as_ref(), issued.command) {
                            (Some(pool), EngineCommand::SendSinglePacket(send)) => {
                                defer_send_single_packet!(pool, id, send, now)
                            }
                            (_, command) => engine.ingest_command_into(
                                IssuedCommand { id, command },
                                interfaces.view(),
                                now,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            ),
                        }
                    }
                    HostCommand::AwaitedEngine { issued, completion } => {
                        let id = issued.id;
                        pending_completions.borrow_mut().insert(id, completion);
                        match (crypto_pool.as_ref(), issued.command) {
                            (Some(pool), EngineCommand::SendSinglePacket(send)) => {
                                defer_send_single_packet!(pool, id, send, now)
                            }
                            (_, command) => engine.ingest_command_into(
                                IssuedCommand { id, command },
                                interfaces.view(),
                                now,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            ),
                        }
                    }
                    HostCommand::SendResource(send) => engine.ingest_send_resource_into(
                        &ResourceSend {
                            id: send.id,
                            link_id: send.link_id,
                            body: ResourceBody {
                                data: send.data.as_slice(),
                                compressed_candidate: send
                                    .compressed_candidate
                                    .as_ref()
                                    .map(HostResourcePayload::as_slice),
                                metadata: send.metadata.as_engine(),
                            },
                            correlation: send
                                .request_id
                                .map_or(ResourceCorrelation::Unsolicited, ResourceCorrelation::Response),
                        },
                        now,
                        &mut |entropy| host.fill_entropy(entropy),
                        &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                    ),
                    HostCommand::SendResourceSegment(send) => {
                        pending_completions.borrow_mut().insert(send.id, send.completion);
                        engine.ingest_send_resource_segment_into(
                            &ResourceSend {
                                id: send.id,
                                link_id: send.link_id,
                                body: ResourceBody {
                                    data: send.data.as_slice(),
                                    compressed_candidate: send
                                        .compressed_candidate
                                        .as_ref()
                                        .map(HostResourcePayload::as_slice),
                                    metadata: send.metadata.as_engine(),
                                },
                                correlation: send
                                    .request_id
                                    .map_or(ResourceCorrelation::Unsolicited, ResourceCorrelation::Response),
                            },
                            ResourceSegment {
                                index: send.segment_index,
                                total_segments: send.total_segments,
                                total_data_size: send.total_data_size,
                            },
                            now,
                            &mut |entropy| host.fill_entropy(entropy),
                            &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                        )
                    }
                    HostCommand::RespondAny(respond) => {
                        let data = respond.data.as_slice();
                        let as_packet = engine
                            .response_fits_packet(&respond.link_id, data)
                            .then(|| RespondData::from_slice(data).ok())
                            .flatten();
                        match as_packet {
                            Some(data) => engine.ingest_command_into(
                                IssuedCommand {
                                    id: respond.id,
                                    command: EngineCommand::Respond(Respond {
                                        link_id: respond.link_id,
                                        request_id: respond.request_id,
                                        data,
                                    }),
                                },
                                interfaces.view(),
                                now,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            ),
                            None => engine.ingest_send_resource_into(
                                &ResourceSend {
                                    id: respond.id,
                                    link_id: respond.link_id,
                                    body: ResourceBody {
                                        data,
                                        compressed_candidate: respond
                                            .compressed_candidate
                                            .as_ref()
                                            .map(HostResourcePayload::as_slice),
                                        metadata: ResourceMetadata::None,
                                    },
                                    correlation: ResourceCorrelation::Response(respond.request_id),
                                },
                                now,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            ),
                        }
                    }
                    HostCommand::RequestAny(request) => {
                        let RequestAnyHostCommand {
                            id,
                            link_id,
                            path_hash,
                            data,
                            completion,
                        } = request;
                        pending_responses
                            .borrow_mut()
                            .insert(id, RequestPending { completion, data: None });
                        let payload = data.as_slice();
                        if engine.request_fits_packet(&link_id, payload) {
                            match SendRequestData::from_slice(payload) {
                                Ok(send_data) => engine.ingest_command_into(
                                    IssuedCommand {
                                        id,
                                        command: EngineCommand::SendRequest(SendRequest {
                                            link_id,
                                            path_hash,
                                            data: send_data,
                                        }),
                                    },
                                    interfaces.view(),
                                    now,
                                    &mut |entropy| host.fill_entropy(entropy),
                                    &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                ),
                                Err(_) => fail_request(&pending_responses, id),
                            }
                        } else {
                            let mut packed =
                                std::vec![0u8; REQUEST_WIRE_OVERHEAD + payload.len().max(1)];
                            match write_request_plaintext(now, &path_hash, payload, &mut packed) {
                                Ok(plain_len) => {
                                    let packed_request = &packed[..plain_len];
                                    let request_id = RequestId::of_request_data(packed_request);
                                    engine.ingest_send_resource_into(
                                        &ResourceSend {
                                            id,
                                            link_id,
                                            body: ResourceBody {
                                                data: packed_request,
                                                compressed_candidate: None,
                                                metadata: ResourceMetadata::None,
                                            },
                                            correlation: ResourceCorrelation::Request(request_id),
                                        },
                                        now,
                                        &mut |entropy| host.fill_entropy(entropy),
                                        &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                    )
                                }
                                Err(_) => fail_request(&pending_responses, id),
                            }
                        }
                    }
                    HostCommand::ProvideDecompressed(provide) => engine.provide_decompressed(
                        provide.link_id,
                        provide.hash,
                        provide.plaintext.as_slice(),
                        now,
                        &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                    ),
                    HostCommand::AddInterface(add) => {
                        let AddInterfaceCommand {
                            descriptor,
                            logical_interface,
                            inbound,
                            egress: egress_producer,
                            connection,
                            ifac,
                        } = add;
                        let id = descriptor.id;
                        if interfaces.view().descriptor_for(id).is_some() {
                            debug_assert!(
                                false,
                                "interface id collision (kind byte {}): two live channels produced the same channel tag — an interface returned a non-unique channel_tag",
                                id.as_bytes()[0],
                            );
                            drop((inbound, egress_producer));
                            WakeSchedules::UNCHANGED
                        } else {
                            let frame_cap = frame_cap_for(&descriptor);
                            pacers.push(InterfacePacer {
                                id,
                                #[cfg(feature = "runtime-metrics")]
                                logical_interface,
                                pacer: AnnouncePacer::new(
                                    descriptor.announce_bandwidth_cap,
                                    descriptor.bitrate,
                                ),
                            });
                            #[cfg(feature = "runtime-metrics")]
                            engine.attach_metrics_interface(id, logical_interface);
                            engine.interface_attached(id, now);
                            interfaces.push(descriptor);
                            inbound_lanes.push((id, inbound));
                            egress.add_lane(id, logical_interface, egress_producer, connection);
                            if let Some(context) = ifac {
                                ifacs.push(InterfaceIfac { id, context });
                            }
                            if frame_cap > scratch_cap {
                                scratch_cap = frame_cap;
                                unmask_scratch = std::vec![0u8; scratch_cap].into_boxed_slice();
                                wire_scratch.grow(scratch_cap);
                            }
                            wake_schedules = engine.wake_schedules(interfaces.view());
                            WakeSchedules::UNCHANGED
                        }
                    }
                    HostCommand::RemoveInterface { id, departure } => {
                        engine.interface_departed(id, departure, now);
                        interfaces.remove(id);
                        inbound_lanes.retain(|(lane_id, _)| *lane_id != id);
                        pacers.retain(|pacer| pacer.id != id);
                        ifacs.retain(|entry| entry.id != id);
                        egress.remove_lane(id);
                        wake_schedules = engine.wake_schedules(interfaces.view());
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::SynthesizeTunnel { interface } => {
                        let mut random_hash = [0u8; crate::routing::tunnel::RANDOM_HASH_LEN];
                        host.fill_entropy(&mut random_hash);
                        let mut buf = [0u8; 256];
                        if let Ok(len) =
                            engine.write_tunnel_synthesize(interface, &random_hash, &mut buf)
                        {
                            route_reaction(
                                EngineReaction::Directive(Directive::Send {
                                    target: interface,
                                    bytes: &buf[..len],
                                }),
                                &mut egress,
                                &ifacs,
                                &mut pacers,
                                &mut wire_scratch,
                                now,
                                &mut journaled_sink!(),
                            );
                        }
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::RegisterStreamReader {
                        link_id,
                        stream_id,
                        sink,
                        ready,
                    } => {
                        stream_readers
                            .borrow_mut()
                            .insert((link_id, stream_id), sink);
                        let _ = ready.send(());
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::RegisterResourceSink {
                        link_id,
                        sink,
                        ready,
                    } => {
                        resource_sinks.borrow_mut().insert(link_id, sink);
                        let _ = ready.send(());
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::SetResourceStrategy {
                        destination,
                        strategy,
                        ready,
                    } => {
                        let applied = engine.set_default_resource_strategy(&destination, strategy);
                        let _ = ready.send(applied);
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::SnapshotPersistedState { reply } => {
                        let mut routing_table = std::vec![
                            0u8;
                            crate::persistence::routing_table_snapshot_len(
                                engine.persisted_route_rows(),
                            )
                        ];
                        let mut tunnels = std::vec![
                            0u8;
                            crate::persistence::tunnels_snapshot_len(
                                engine.persisted_tunnel_rows().count(),
                            )
                        ];
                        let written_routes = crate::persistence::write_routing_table_snapshot(
                            engine.persisted_route_rows(),
                            &mut routing_table,
                        );
                        let written_tunnels = crate::persistence::write_tunnels_snapshot(
                            engine.persisted_tunnel_rows(),
                            &mut tunnels,
                        );
                        if let (Ok(routes_len), Ok(tunnels_len)) = (written_routes, written_tunnels)
                        {
                            routing_table.truncate(routes_len);
                            tunnels.truncate(tunnels_len);
                            let _ = reply.send(PersistedStateSnapshot {
                                routing_table,
                                tunnels,
                                taken_at: now,
                            });
                        }
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::SnapshotSelfRatchets { reply } => {
                        let mut blobs = std::vec::Vec::new();
                        for (destination, last_rotated, secrets) in
                            engine.persisted_self_ratchet_rows()
                        {
                            let mut sealed = Zeroizing::new(std::vec![
                                0u8;
                                crate::persistence::self_ratchets_snapshot_len(secrets.len())
                            ]);
                            if let Ok(written) = crate::persistence::write_self_ratchets_snapshot(
                                last_rotated,
                                secrets,
                                &mut sealed,
                            ) {
                                sealed.truncate(written);
                                blobs.push((destination, sealed));
                            }
                        }
                        let _ = reply.send(SelfRatchetsSnapshot { blobs });
                        WakeSchedules::UNCHANGED
                    }
                    #[cfg(feature = "runtime-metrics")]
                    HostCommand::SnapshotMetrics { reply } => {
                        let _ = reply.send(RuntimeMetricsSnapshot {
                            taken_at: now,
                            engine: engine.metrics_snapshot(),
                            egress: egress.metrics_snapshot(&pacers),
                            crypto: crypto_pool.as_ref().map(CryptoPool::metrics_snapshot),
                            reliability,
                        });
                        WakeSchedules::UNCHANGED
                    }
                };
                merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine, interfaces.view());
                command_budget -= 1;
                if command_budget == 0 {
                    break;
                }
                match commands.try_recv() {
                    Ok(next) => issued = next,
                    Err(_) => break,
                }
                }
            }
            () = &mut due_timer, if armed.is_some() => {
                if let Some((deadline, reason)) = armed.take() {
                    let now = host.now();
                    if deadline <= now {
                        let wake_schedules_delta = fire_due_reason(
                            &mut engine,
                            reason,
                            now,
                            interfaces.view(),
                            &mut |bytes| host.fill_entropy(bytes),
                            &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                        );
                        merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine, interfaces.view());
                    }
                }
            }
            () = &mut pacer_timer, if pacer_armed.is_some() => {
                pacer_armed = None;
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &mut egress, &ifacs);
            }
            verdict = crypto_rx.recv(), if crypto_pool.is_some() => {
                let mut next = verdict;
                let now = host.now();
                let mut seal_buf = [0u8; crate::wire::BROADCAST_MTU];
                while let Some(result) = next {
                    if let Some(pool) = crypto_pool.as_ref() {
                        #[cfg(feature = "runtime-metrics")]
                        pool.record_completed();
                        if result.settles_packet_verdict() {
                            pool.packet_verdict_settled();
                        }
                    }
                    match result {
                        CryptoResult::Verified { id, settlement, valid } => {
                            if valid && engine.settle_resolved(id).is_some() {
                                route_reaction(
                                    EngineReaction::Journaled(Journaled::CommandSettled {
                                        id,
                                        settlement,
                                    }),
                                    &mut egress,
                                    &ifacs,
                                    &mut pacers,
                                    &mut wire_scratch,
                                    now,
                                    &mut journaled_sink!(),
                                );
                            }
                        }
                        CryptoResult::Sealed {
                            owed,
                            ephemeral_public,
                            shared,
                        } => {
                            let delta = engine.complete_send_single_packet_deferred(
                                owed,
                                ephemeral_public,
                                shared,
                                interfaces.view(),
                                &mut seal_buf,
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces.view());
                        }
                        CryptoResult::Signed {
                            target,
                            packet_hash,
                            signature,
                        } => {
                            let mut proof = [0u8; IMPLICIT_PROOF_WIRE_LEN];
                            if let Ok(written) =
                                write_implicit_proof_wire_packet(&packet_hash, &signature, &mut proof)
                            {
                                route_reaction(
                                    EngineReaction::Directive(Directive::Send {
                                        target,
                                        bytes: &proof[..written],
                                    }),
                                    &mut egress,
                                    &ifacs,
                                    &mut pacers,
                                    &mut wire_scratch,
                                    now,
                                    &mut journaled_sink!(),
                                );
                            }
                        }
                        CryptoResult::Decrypted { owed, shared } => {
                            let mut deferred_sign = None;
                            engine.resume_decrypt(
                                owed,
                                shared,
                                interfaces.view(),
                                &mut should_prove,
                                &mut deferred_sign,
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            if let Some(deferred) = deferred_sign {
                                if let Some(pool) = crypto_pool.as_ref() {
                                    pool.submit(CryptoJob::Sign(deferred));
                                }
                            }
                        }
                        CryptoResult::RatchetDecrypted { owed, opened } => {
                            if let Some((opened_by, plaintext)) = opened {
                                let mut deferred_sign = None;
                                engine.resume_ratchet_decrypt(
                                    *owed,
                                    OpenedToken {
                                        opened_by,
                                        plaintext: &plaintext,
                                    },
                                    interfaces.view(),
                                    &mut should_prove,
                                    &mut deferred_sign,
                                    &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                );
                                if let Some(deferred) = deferred_sign {
                                    if let Some(pool) = crypto_pool.as_ref() {
                                        pool.submit(CryptoJob::Sign(deferred));
                                    }
                                }
                            }
                        }
                        CryptoResult::LinkProofVerified { owed, shared } => {
                            if let Some(shared) = shared {
                                let delta = engine.resume_link_proof(
                                    owed,
                                    shared,
                                    interfaces.view(),
                                    now,
                                    &mut |entropy| host.fill_entropy(entropy),
                                    &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                );
                                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces.view());
                            }
                        }
                        CryptoResult::LinkProofSigned { owed, responder_encryption, shared, signature } => {
                            let delta = engine.resume_link_proof_sign(
                                owed,
                                responder_encryption,
                                shared,
                                signature,
                                interfaces.view(),
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces.view());
                        }
                        CryptoResult::StagedSealed { link_id, stream_nonce, nonce_prefixed_len, transfer, names, outcome } => {
                            let sealed_len = outcome.map_or(0, |sealed| sealed.sealed_transfer_len);
                            let names_len = outcome.map_or(0, |sealed| sealed.part_count * MAP_HASH_LEN);
                            engine.apply_offloaded_staged_seal(
                                OffloadedStagedSeal {
                                    link_id,
                                    stream_nonce,
                                    nonce_prefixed_len,
                                    sealed_bytes: &transfer[..sealed_len],
                                    names: &names[..names_len],
                                    outcome,
                                },
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            engine.promote_staged_resource(
                                &link_id,
                                now,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            merge_wake_schedules_delta(
                                &mut wake_schedules,
                                WakeSchedules {
                                    resource_deadlines: engine.resource_deadlines_wake(),
                                    ..WakeSchedules::UNCHANGED
                                },
                                &engine,
                                interfaces.view(),
                            );
                        }
                        CryptoResult::AnnounceVerified { owed, valid } => {
                            if valid {
                                let delta = engine.resume_announce(
                                    owed,
                                    interfaces.view(),
                                    &mut |entropy| host.fill_entropy(entropy),
                                    &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                );
                                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces.view());
                            }
                        }
                        CryptoResult::SpanOpened { link_id, hash, span_start, state, bytes } => {
                            let delta = engine.apply_opened_span(
                                OffloadedOpenSpan {
                                    link_id,
                                    hash,
                                    span_start,
                                    state,
                                    bytes: &bytes,
                                },
                                now,
                                &mut |reaction| route_reaction(reaction, &mut egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces.view());
                            dispatch_owed_open_spans!();
                        }
                    }
                    next = crypto_rx.try_recv().ok();
                }
                dispatch_owed_open_spans!();
            }
            _ = tokio::task::yield_now(), if crypto_pool.as_ref().is_some_and(CryptoPool::awaits_packet_verdict) => {}
        }
        if let Some(store) = &store {
            let mut dirty_interfaces = engine.take_dirty_interfaces();
            let mut changed = false;
            dirty_interfaces.drain(|interface| {
                if interfaces.view().descriptor_for(interface).is_some() {
                    store.set(interface, engine.interface_counts(interface));
                } else {
                    store.forget(interface);
                }
                changed = true;
            });
            if changed {
                store.bump();
            }
        }
    }
}

#[cfg(feature = "runtime-metrics")]
type TokioAnnouncePacer = AnnouncePacer<HeapPacerQueue<AnnounceOrigin>, AnnounceOrigin>;
#[cfg(not(feature = "runtime-metrics"))]
type TokioAnnouncePacer = AnnouncePacer<HeapPacerQueue>;

struct InterfacePacer {
    id: InterfaceId,
    #[cfg(feature = "runtime-metrics")]
    logical_interface: InterfaceId,
    pacer: TokioAnnouncePacer,
}

/// Heap-parked wire scratch for every emission that can't land straight in a granted
/// slot: `emit` carries a discarded or pre-mask frame, `masked` the IFAC mask output.
/// Boxed once per reactor — wire-sized buffers never live on a task stack.
struct WireScratch {
    emit: std::boxed::Box<[u8]>,
    masked: std::boxed::Box<[u8]>,
}

impl WireScratch {
    fn new(cap: usize) -> Self {
        Self {
            emit: std::vec![0u8; cap].into_boxed_slice(),
            masked: std::vec![0u8; cap].into_boxed_slice(),
        }
    }

    fn grow(&mut self, cap: usize) {
        if self.emit.len() < cap {
            self.emit = std::vec![0u8; cap].into_boxed_slice();
            self.masked = std::vec![0u8; cap].into_boxed_slice();
        }
    }
}

fn route_reaction<A: FnMut(Journaled<'_>)>(
    reaction: EngineReaction<'_>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    scratch: &mut WireScratch,
    now: InstantMillis,
    app: &mut A,
) {
    match reaction {
        EngineReaction::Directive(Directive::Send { target, bytes }) => {
            enqueue_for_wire(egress, ifacs, target, bytes, &mut scratch.masked);
        }
        #[cfg(feature = "runtime-metrics")]
        EngineReaction::Directive(Directive::SendLocalAnnounce { target, bytes }) => {
            enqueue_announce_for_wire(
                egress,
                ifacs,
                target,
                bytes,
                &mut scratch.masked,
                AnnounceOrigin::Local,
            );
        }
        EngineReaction::Directive(Directive::SendAnnounce {
            target,
            bytes,
            hops,
            #[cfg(feature = "runtime-metrics")]
            origin,
        }) => {
            offer_to_pacer(
                pacers,
                target,
                PacedAnnounce {
                    bytes,
                    hops,
                    #[cfg(feature = "runtime-metrics")]
                    origin,
                },
                now,
                egress,
                ifacs,
            );
        }
        EngineReaction::Directive(Directive::EmitFrame {
            target,
            size_hint,
            fill,
        }) => {
            emit_for_wire(egress, ifacs, target, size_hint, fill, scratch);
        }
        EngineReaction::Directive(Directive::SendToFleet {
            supervisor,
            fan,
            bytes,
        }) => {
            for target in egress.broadcast_targets(supervisor, fan) {
                enqueue_for_wire(egress, ifacs, target, bytes, &mut scratch.masked);
            }
        }
        #[cfg(feature = "runtime-metrics")]
        EngineReaction::Directive(Directive::SendLocalAnnounceToFleet {
            supervisor,
            fan,
            bytes,
        }) => {
            for target in egress.broadcast_targets(supervisor, fan) {
                enqueue_announce_for_wire(
                    egress,
                    ifacs,
                    target,
                    bytes,
                    &mut scratch.masked,
                    AnnounceOrigin::Local,
                );
            }
        }
        EngineReaction::Directive(Directive::SendAnnounceToFleet {
            supervisor,
            fan,
            bytes,
            hops,
            #[cfg(feature = "runtime-metrics")]
            origin,
        }) => {
            for target in egress.broadcast_targets(supervisor, fan) {
                offer_to_pacer(
                    pacers,
                    target,
                    PacedAnnounce {
                        bytes,
                        hops,
                        #[cfg(feature = "runtime-metrics")]
                        origin,
                    },
                    now,
                    egress,
                    ifacs,
                );
            }
        }
        EngineReaction::Journaled(journaled) => app(journaled),
    }
}

/// Grant-first emission: with no IFAC in the way the engine seals straight into the granted
/// slot, zero copy. An IFAC'd target builds in scratch and masks into the slot (the mask is
/// the copy), and a full lane runs `fill` against scratch and discards, so the engine's
/// bookkeeping runs exactly once on every path.
fn emit_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    size_hint: usize,
    fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
    scratch: &mut WireScratch,
) {
    match ifac_for(ifacs, target) {
        Some(entry) => {
            if let Some(len) = fill(&mut scratch.emit) {
                if let Some(masked_len) = entry
                    .context
                    .mask_outbound(&scratch.emit[..len], &mut scratch.masked)
                {
                    egress.enqueue(target, &scratch.masked[..masked_len]);
                }
            }
        }
        None => egress.emit(target, size_hint, fill, &mut scratch.emit),
    }
}

fn ifac_for(ifacs: &[InterfaceIfac], id: InterfaceId) -> Option<&InterfaceIfac> {
    if ifacs.is_empty() {
        return None;
    }
    ifacs.iter().find(|entry| entry.id == id)
}

/// The one egress choke: a target with an access code never sees clean bytes on its
/// wire, and a frame the mask refuses (oversize) is dropped rather than leaked open.
fn enqueue_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
    masked: &mut [u8],
) {
    match ifac_for(ifacs, target) {
        Some(entry) => {
            if let Some(masked_len) = entry.context.mask_outbound(bytes, masked) {
                egress.enqueue(target, &masked[..masked_len]);
            }
        }
        None => {
            egress.enqueue(target, bytes);
        }
    }
}

#[cfg(feature = "runtime-metrics")]
fn enqueue_announce_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
    masked: &mut [u8],
    origin: AnnounceOrigin,
) {
    let (outcome, wire_len) = match ifac_for(ifacs, target) {
        Some(entry) => {
            let Some(masked_len) = entry.context.mask_outbound(bytes, masked) else {
                egress.record_announce(
                    target,
                    bytes.len(),
                    origin,
                    AnnounceEgressOutcome::IfacRejected,
                );
                return;
            };
            (egress.enqueue(target, &masked[..masked_len]), masked_len)
        }
        None => (egress.enqueue(target, bytes), bytes.len()),
    };
    let outcome = match outcome {
        EgressEnqueueOutcome::Enqueued => AnnounceEgressOutcome::Enqueued,
        EgressEnqueueOutcome::LaneFull => AnnounceEgressOutcome::LaneFull,
        EgressEnqueueOutcome::LaneMissing => AnnounceEgressOutcome::LaneMissing,
    };
    egress.record_announce(target, wire_len, origin, outcome);
}

/// A paced announce is broadcast-sized by construction, so its mask scratch fits on the
/// stack — the wire-sized [`WireScratch`] is reserved for the frame paths.
const PACED_MASK_LEN: usize = crate::wire::BROADCAST_MTU + crate::interfaces::ifac::IFAC_MAX_SIZE;

struct PacedAnnounce<'a> {
    bytes: &'a [u8],
    hops: u8,
    #[cfg(feature = "runtime-metrics")]
    origin: AnnounceOrigin,
}

#[cfg(feature = "runtime-metrics")]
fn offer_to_pacer(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    announce: PacedAnnounce<'_>,
    now: InstantMillis,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
) {
    if egress.skip_unavailable(target) {
        egress.record_announce(
            target,
            announce.bytes.len(),
            announce.origin,
            AnnounceEgressOutcome::InterfaceUnavailable,
        );
        return;
    }
    let offer = match pacers.iter_mut().find(|entry| entry.id == target) {
        Some(entry) => entry.pacer.offer_tagged(
            announce.bytes,
            announce.hops,
            now,
            announce.origin,
            |frame, frame_origin| {
                let mut masked = [0u8; PACED_MASK_LEN];
                enqueue_announce_for_wire(egress, ifacs, target, frame, &mut masked, frame_origin);
            },
        ),
        None => {
            let mut masked = [0u8; PACED_MASK_LEN];
            enqueue_announce_for_wire(
                egress,
                ifacs,
                target,
                announce.bytes,
                &mut masked,
                announce.origin,
            );
            PacerOffer::Sent
        }
    };
    if matches!(offer, PacerOffer::Rejected(_)) {
        egress.record_announce(
            target,
            announce.bytes.len(),
            announce.origin,
            AnnounceEgressOutcome::PacerRejected,
        );
    }
}

#[cfg(not(feature = "runtime-metrics"))]
fn offer_to_pacer(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    announce: PacedAnnounce<'_>,
    now: InstantMillis,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
) {
    if egress.skip_unavailable(target) {
        return;
    }
    match pacers.iter_mut().find(|entry| entry.id == target) {
        Some(entry) => {
            entry
                .pacer
                .offer(announce.bytes, announce.hops, now, |frame| {
                    let mut masked = [0u8; PACED_MASK_LEN];
                    enqueue_for_wire(egress, ifacs, target, frame, &mut masked);
                });
        }
        None => {
            let mut masked = [0u8; PACED_MASK_LEN];
            enqueue_for_wire(egress, ifacs, target, announce.bytes, &mut masked);
        }
    }
}

#[cfg(feature = "runtime-metrics")]
fn flush_due_pacers(
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
) {
    for entry in pacers.iter_mut() {
        let target = entry.id;
        entry.pacer.release_due_tagged(now, |frame, origin| {
            if egress.skip_unavailable(target) {
                egress.record_announce(
                    target,
                    frame.len(),
                    origin,
                    AnnounceEgressOutcome::InterfaceUnavailable,
                );
                return;
            }
            let mut masked = [0u8; PACED_MASK_LEN];
            enqueue_announce_for_wire(egress, ifacs, target, frame, &mut masked, origin);
        });
    }
}

#[cfg(not(feature = "runtime-metrics"))]
fn flush_due_pacers(
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
) {
    for entry in pacers.iter_mut() {
        let target = entry.id;
        entry.pacer.release_due(now, |frame| {
            if egress.skip_unavailable(target) {
                return;
            }
            let mut masked = [0u8; PACED_MASK_LEN];
            enqueue_for_wire(egress, ifacs, target, frame, &mut masked);
        });
    }
}

fn soonest_pacer_release(pacers: &[InterfacePacer]) -> Option<InstantMillis> {
    pacers
        .iter()
        .filter_map(|entry| entry.pacer.next_release())
        .min_by_key(|deadline| deadline.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        bytes_from_hex, pin_transport_id, TestStorageLayout, RNS_1_3_5_ANNOUNCE,
        RNS_1_3_5_RATCHETED_ANNOUNCE, TEST_TRANSPORT_ID,
    };
    use crate::engine::RouteRemovalCause;
    use crate::interfaces::{
        AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability,
        InterfaceCapabilities, InterfaceMode, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb,
        TransportCapability,
    };
    use crate::reactor::interface_seam::Interface;
    use crate::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
    use crate::wire::{PacketType, WirePacketHeader};

    #[tokio::test(start_paused = true)]
    async fn logical_time_saturates_at_the_numeric_limit() {
        let host = TokioHost::start_at(InstantMillis(u64::MAX - 5));
        tokio::time::advance(Duration::from_millis(10)).await;
        assert_eq!(host.now(), InstantMillis(u64::MAX));
    }

    #[tokio::test(start_paused = true)]
    async fn a_far_future_sleep_arms_without_overflowing_the_timer() {
        let host = TokioHost::new();
        let sleeping = host.sleep_until(InstantMillis(u64::MAX));
        tokio::pin!(sleeping);
        tokio::select! {
            () = &mut sleeping => panic!("the numeric limit is not immediately due"),
            () = tokio::time::sleep(Duration::from_millis(1)) => {}
        }
    }

    #[test]
    fn packet_phy_is_retained_under_the_wire_stable_packet_hash() {
        let store = InterfaceStore::new();
        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let packet_hash = PacketHash::of_wire_packet(&raw).expect("the fixture is a wire packet");
        let packet_phy = PacketPhyStats {
            rssi: Some(RssiDbm::new(-103)),
            snr: Some(SnrQuarterDb::new(-11)),
            quality: SignalQualityTenthsPercent::new(731),
        };

        retain_packet_phy(Some(&store), &raw, packet_phy);

        assert_eq!(store.packet_phy(packet_hash), Some(packet_phy));
    }

    #[test]
    fn crypto_backpressure_depth_is_bounded_across_worker_counts() {
        assert_eq!(crypto_backpressure_depth(1), MIN_CRYPTO_QUEUE_DEPTH);
        assert_eq!(crypto_backpressure_depth(16), MAX_CRYPTO_QUEUE_DEPTH);
        assert_eq!(
            crypto_backpressure_depth(usize::MAX),
            MAX_CRYPTO_QUEUE_DEPTH
        );
    }

    #[cfg(feature = "runtime-metrics")]
    #[test]
    fn egress_metrics_distinguish_enqueued_full_and_missing_lanes() {
        let id = InterfaceId::new([0x91; 8]);
        let missing = InterfaceId::new([0x92; 8]);
        let (producer, _consumer) = tokio_grant_lane(64, 1);
        let mut egress = Egress::new(std::vec![(id, producer)]);

        egress.enqueue(id, b"first");
        egress.enqueue(id, b"full");
        egress.enqueue(missing, b"missing");

        assert_eq!(
            egress.metrics_snapshot(&[]),
            EgressMetricsSnapshot {
                enqueued_frames: 1,
                full_lane_drops: 1,
                missing_lane_drops: 1,
                announces: crate::runtime::AnnounceEgressMetricsSnapshot {
                    interfaces: std::vec![crate::runtime::InterfaceAnnounceEgressMetricsSnapshot {
                        interface: id,
                        outcomes: Default::default(),
                        enqueued_bytes_by_origin: Default::default(),
                        pacer_queue_depth: 0,
                    },],
                    ..Default::default()
                },
                ..Default::default()
            }
        );
    }

    #[cfg(feature = "runtime-metrics")]
    #[test]
    fn announce_egress_metrics_preserve_origin_outcome_kind_and_bytes() {
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"announce-egress");
        let missing = InterfaceId::from_channel_tag(InterfaceKind::Udp, b"missing-egress");
        let (producer, _consumer) = tokio_grant_lane(64, 1);
        let mut egress = Egress::new(std::vec![(id, producer)]);
        let mut masked = [0u8; 64];

        enqueue_announce_for_wire(
            &mut egress,
            &[],
            id,
            b"accepted",
            &mut masked,
            AnnounceOrigin::Local,
        );
        enqueue_announce_for_wire(
            &mut egress,
            &[],
            id,
            b"full",
            &mut masked,
            AnnounceOrigin::Relay,
        );
        enqueue_announce_for_wire(
            &mut egress,
            &[],
            missing,
            b"missing",
            &mut masked,
            AnnounceOrigin::SharedClient,
        );

        let announces = egress.metrics_snapshot(&[]).announces;
        assert_eq!(
            announces
                .outcomes
                .get(AnnounceOrigin::Local, AnnounceEgressOutcome::Enqueued),
            1
        );
        assert_eq!(
            announces
                .outcomes
                .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::LaneFull),
            1
        );
        assert_eq!(
            announces.outcomes.get(
                AnnounceOrigin::SharedClient,
                AnnounceEgressOutcome::LaneMissing
            ),
            1
        );
        assert_eq!(
            announces
                .enqueued_by_interface_kind
                .get(InterfaceKind::TcpClient),
            1
        );
        assert_eq!(
            announces
                .enqueued_bytes_by_origin
                .get(AnnounceOrigin::Local),
            b"accepted".len() as u64
        );
    }

    #[cfg(feature = "runtime-metrics")]
    #[test]
    fn announce_egress_metrics_roll_fleet_members_into_their_logical_interface() {
        let logical = InterfaceId::from_channel_tag(InterfaceKind::TcpServer, b"server");
        let first = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"first");
        let second = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"second");
        let (first_producer, _first_consumer) = tokio_grant_lane(64, 1);
        let (second_producer, _second_consumer) = tokio_grant_lane(64, 1);
        let mut egress = Egress::new(std::vec![]);
        egress.add_lane(first, logical, first_producer, None);
        egress.add_lane(second, logical, second_producer, None);
        let mut masked = [0u8; 64];

        enqueue_announce_for_wire(
            &mut egress,
            &[],
            first,
            b"first",
            &mut masked,
            AnnounceOrigin::Relay,
        );
        enqueue_announce_for_wire(
            &mut egress,
            &[],
            second,
            b"second",
            &mut masked,
            AnnounceOrigin::Relay,
        );

        let announces = egress.metrics_snapshot(&[]).announces;
        assert_eq!(announces.interfaces.len(), 1);
        assert_eq!(announces.interfaces[0].interface, logical);
        assert_eq!(
            announces.interfaces[0]
                .outcomes
                .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
            2
        );
        assert_eq!(
            announces
                .enqueued_by_interface_kind
                .get(InterfaceKind::TcpServer),
            2
        );
    }

    #[cfg(feature = "runtime-metrics")]
    #[test]
    fn crypto_metrics_are_bounded_snapshots() {
        let (results, _result_rx) = mpsc::unbounded_channel();
        let pool = CryptoPool::spawn(1, results).expect("worker spawns");

        assert_eq!(bounded_u32(usize::MAX), u32::MAX);
        assert!(!pool.has_queue_capacity(usize::MAX));
        pool.record_completed();

        assert_eq!(
            pool.metrics_snapshot(),
            CryptoMetricsSnapshot {
                completed_jobs: 1,
                backpressure_deferrals: 1,
                ..CryptoMetricsSnapshot::default()
            }
        );
    }

    #[test]
    fn dropping_a_crypto_pool_joins_every_worker() {
        let (results, _result_rx) = mpsc::unbounded_channel();
        let pool = CryptoPool::spawn(2, results).expect("workers spawn");
        let queue = pool.queue.clone();
        drop(pool);
        assert!(queue.shutdown.load(Ordering::Acquire));
        assert_eq!(queue.len.load(Ordering::Acquire), 0);
        assert_eq!(Arc::strong_count(&queue), 1);
    }

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
            .expect("a synthesize request reached the reactor");
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

    use tokio::sync::mpsc;

    #[test]
    fn settle_fires_the_awaited_completion_and_suppresses_the_event() {
        let pending: RefCell<HashMap<CommandId, oneshot::Sender<Settlement>>> =
            RefCell::new(HashMap::new());
        let (completion, mut settled) = oneshot::channel();
        pending.borrow_mut().insert(CommandId(7), completion);

        let settlement = Settlement::SendSinglePacket(Ok(crate::engine::PacketReceiptDelivered {
            rtt: crate::units::RttMillis::new(9),
        }));
        let forwarded = settle_or_forward(
            &pending,
            Journaled::CommandSettled {
                id: CommandId(7),
                settlement: settlement.clone(),
            },
        );

        assert!(
            forwarded.is_none(),
            "an awaited settlement is consumed, not forwarded to the app"
        );
        assert_eq!(
            settled
                .try_recv()
                .expect("the awaiter received its settlement"),
            settlement
        );
        assert!(
            pending.borrow().is_empty(),
            "the awaiter is removed from the registry once fired"
        );
    }

    #[test]
    fn settle_forwards_a_settlement_nobody_awaits() {
        let pending: RefCell<HashMap<CommandId, oneshot::Sender<Settlement>>> =
            RefCell::new(HashMap::new());
        let forwarded = settle_or_forward(
            &pending,
            Journaled::CommandSettled {
                id: CommandId(3),
                settlement: Settlement::SendSinglePacket(Ok(
                    crate::engine::PacketReceiptDelivered {
                        rtt: crate::units::RttMillis::new(1),
                    },
                )),
            },
        );
        assert!(
            forwarded.is_some(),
            "a settlement with no awaiter passes through to on_event"
        );
    }

    const RES_LINK: LinkId = LinkId::new([0x44; 16]);

    fn resource_sink_registry() -> (
        RefCell<HashMap<LinkId, mpsc::UnboundedSender<ResourceInbound>>>,
        mpsc::UnboundedReceiver<ResourceInbound>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let sinks = RefCell::new(HashMap::new());
        sinks.borrow_mut().insert(RES_LINK, tx);
        (sinks, rx)
    }

    #[test]
    fn route_resource_routes_a_segment_and_keeps_the_sink() {
        let (sinks, mut rx) = resource_sink_registry();
        let forwarded = route_resource_or_forward(
            &sinks,
            Journaled::ResourceSegmentReceived {
                link_id: RES_LINK,
                original_hash: ResourceHash::new([1; 32]),
                segment_index: 1,
                total_segments: 2,
                metadata: None,
                data: b"first",
            },
        );
        assert!(
            forwarded.is_none(),
            "a routed segment is suppressed from the app event stream"
        );
        assert!(matches!(rx.try_recv(), Ok(ResourceInbound::Chunk(c)) if c == b"first"));
        assert!(
            sinks.borrow().contains_key(&RES_LINK),
            "the sink stays for the segments still to come"
        );
    }

    #[test]
    fn route_resource_completes_and_retires_on_assembly() {
        let (sinks, mut rx) = resource_sink_registry();
        let forwarded = route_resource_or_forward(
            &sinks,
            Journaled::ResourceAssembled {
                link_id: RES_LINK,
                original_hash: ResourceHash::new([2; 32]),
                total_size: 4096,
            },
        );
        assert!(forwarded.is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(ResourceInbound::Complete {
                total_size: 4096,
                ..
            })
        ));
        assert!(
            sinks.borrow().is_empty(),
            "an assembled resource retires its one-shot sink"
        );
    }

    #[test]
    fn route_resource_delivers_a_single_segment_then_retires() {
        let (sinks, mut rx) = resource_sink_registry();
        let forwarded = route_resource_or_forward(
            &sinks,
            Journaled::ResourceReceived {
                link_id: RES_LINK,
                hash: ResourceHash::new([3; 32]),
                metadata: None,
                data: b"whole",
            },
        );
        assert!(forwarded.is_none());
        assert!(matches!(rx.try_recv(), Ok(ResourceInbound::Chunk(c)) if c == b"whole"));
        assert!(matches!(
            rx.try_recv(),
            Ok(ResourceInbound::Complete { total_size: 5, .. })
        ));
        assert!(
            sinks.borrow().is_empty(),
            "a single-segment resource completes and retires in one go"
        );
    }

    #[test]
    fn route_resource_passes_through_an_unregistered_link() {
        let sinks: RefCell<HashMap<LinkId, mpsc::UnboundedSender<ResourceInbound>>> =
            RefCell::new(HashMap::new());
        let forwarded = route_resource_or_forward(
            &sinks,
            Journaled::ResourceSegmentReceived {
                link_id: RES_LINK,
                original_hash: ResourceHash::new([4; 32]),
                segment_index: 1,
                total_segments: 2,
                metadata: None,
                data: b"x",
            },
        );
        assert!(
            forwarded.is_some(),
            "with no sink registered the journal flows on to the app event stream"
        );
    }

    #[test]
    fn host_resource_payload_supports_owned_and_shared_prefix_bytes() {
        let owned: HostResourcePayload = std::vec![1, 2, 3].into();
        assert_eq!(owned.as_slice(), &[1, 2, 3]);
        assert_eq!(owned.len(), 3);

        let shared: Arc<[u8]> = std::vec![4, 5, 6, 7].into();
        let prefix = HostResourcePayload::shared_prefix(Arc::clone(&shared), 3).unwrap();
        assert_eq!(prefix.as_slice(), &[4, 5, 6]);
        assert_eq!(prefix.len(), 3);
        assert_eq!(
            HostResourcePayload::shared_prefix(shared, 5).unwrap_err(),
            HostResourcePayloadError::PrefixOutOfRange
        );
    }

    fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            bitrate: BitrateBps::guess(1_000_000_000),
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
        }
    }

    #[test]
    fn airtime_reads_none_until_published_then_round_trips() {
        let status =
            TokioInterfaceStatus::new(InterfaceId::new([0x5A; 8]), ConnectionState::Initializing);
        assert_eq!(status.airtime(), None);

        status.set_airtime(AirtimeUtilization {
            short_per_mille: 137,
            long_per_mille: 4,
        });
        assert_eq!(
            status.airtime(),
            Some(AirtimeUtilization {
                short_per_mille: 137,
                long_per_mille: 4,
            }),
        );
    }

    #[test]
    fn the_pacer_wiring_holds_then_releases_a_capped_burst() {
        let id = InterfaceId::new([0x5a; 8]);
        let mut pacers = std::vec![InterfacePacer {
            id,
            #[cfg(feature = "runtime-metrics")]
            logical_interface: id,
            pacer: TokioAnnouncePacer::new(
                AnnounceBandwidthCap::RNS_DEFAULT,
                BitrateBps::guess(5_000),
            ),
        }];
        let (tx, mut rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let mut egress = Egress::new(std::vec![(id, tx)]);

        offer_to_pacer(
            &mut pacers,
            id,
            PacedAnnounce {
                bytes: &[1; 10],
                hops: 1,
                #[cfg(feature = "runtime-metrics")]
                origin: AnnounceOrigin::Local,
            },
            InstantMillis(1_000),
            &mut egress,
            &[],
        );
        assert_eq!(rx.try_peek().unwrap().frame(), [1u8; 10].as_slice());
        rx.release();

        offer_to_pacer(
            &mut pacers,
            id,
            PacedAnnounce {
                bytes: &[2; 10],
                hops: 1,
                #[cfg(feature = "runtime-metrics")]
                origin: AnnounceOrigin::Relay,
            },
            InstantMillis(1_200),
            &mut egress,
            &[],
        );
        assert!(rx.try_peek().is_none(), "the second is held, not sent");
        assert_eq!(soonest_pacer_release(&pacers), Some(InstantMillis(1_800)));
        #[cfg(feature = "runtime-metrics")]
        {
            let announces = egress.metrics_snapshot(&pacers).announces;
            assert_eq!(announces.pacer_queue_depth, 1);
            assert_eq!(
                announces
                    .outcomes
                    .get(AnnounceOrigin::Local, AnnounceEgressOutcome::Enqueued),
                1
            );
            assert_eq!(
                announces
                    .outcomes
                    .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
                0
            );
        }

        flush_due_pacers(&mut pacers, InstantMillis(1_799), &mut egress, &[]);
        assert!(
            rx.try_peek().is_none(),
            "nothing releases before the window"
        );

        flush_due_pacers(&mut pacers, InstantMillis(1_800), &mut egress, &[]);
        assert_eq!(rx.try_peek().unwrap().frame(), [2u8; 10].as_slice());
        rx.release();
        assert_eq!(soonest_pacer_release(&pacers), None);
        #[cfg(feature = "runtime-metrics")]
        {
            let announces = egress.metrics_snapshot(&pacers).announces;
            assert_eq!(announces.pacer_queue_depth, 0);
            assert_eq!(
                announces
                    .outcomes
                    .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
                1
            );
        }
    }

    #[test]
    fn an_unavailable_interface_never_enters_the_pacer_or_lane() {
        let id = InterfaceId::new([0x6a; 8]);
        let status = TokioInterfaceStatus::new(id, ConnectionState::Disconnected);
        let mut pacers = std::vec![InterfacePacer {
            id,
            #[cfg(feature = "runtime-metrics")]
            logical_interface: id,
            pacer: TokioAnnouncePacer::new(
                AnnounceBandwidthCap::RNS_DEFAULT,
                BitrateBps::guess(5_000),
            ),
        }];
        let (tx, mut rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let mut egress = Egress::new(std::vec![]);
        egress.add_lane(id, id, tx, Some(ConnectionView::of(status.clone())));

        offer_to_pacer(
            &mut pacers,
            id,
            PacedAnnounce {
                bytes: &[1; 10],
                hops: 1,
                #[cfg(feature = "runtime-metrics")]
                origin: AnnounceOrigin::Relay,
            },
            InstantMillis(1_000),
            &mut egress,
            &[],
        );

        assert!(rx.try_peek().is_none());
        assert_eq!(soonest_pacer_release(&pacers), None);

        status.set_connection(ConnectionState::Connected);
        offer_to_pacer(
            &mut pacers,
            id,
            PacedAnnounce {
                bytes: &[2; 10],
                hops: 1,
                #[cfg(feature = "runtime-metrics")]
                origin: AnnounceOrigin::Relay,
            },
            InstantMillis(1_100),
            &mut egress,
            &[],
        );
        assert_eq!(rx.try_peek().unwrap().frame(), [2u8; 10].as_slice());
        rx.release();

        offer_to_pacer(
            &mut pacers,
            id,
            PacedAnnounce {
                bytes: &[3; 10],
                hops: 1,
                #[cfg(feature = "runtime-metrics")]
                origin: AnnounceOrigin::Relay,
            },
            InstantMillis(1_200),
            &mut egress,
            &[],
        );
        status.set_connection(ConnectionState::Disconnected);
        flush_due_pacers(&mut pacers, InstantMillis(10_000), &mut egress, &[]);
        assert!(rx.try_peek().is_none());
        assert_eq!(soonest_pacer_release(&pacers), None);

        #[cfg(feature = "runtime-metrics")]
        {
            let snapshot = egress.metrics_snapshot(&pacers);
            assert_eq!(snapshot.unavailable_frame_skips, 2);
            assert_eq!(snapshot.announces.pacer_queue_depth, 0);
            assert_eq!(
                snapshot.announces.outcomes.get(
                    AnnounceOrigin::Relay,
                    AnnounceEgressOutcome::InterfaceUnavailable
                ),
                2
            );
            assert_eq!(
                snapshot
                    .announces
                    .outcomes
                    .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
                1
            );
        }
    }

    /// An interface whose "wire" is two in-memory channels: the test plays received bytes onto
    /// `wire_in` and reads transmitted bytes off `wire_out`, exercising the [`InterfaceSeam`]
    /// in isolation.
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

    #[tokio::test]
    async fn a_filled_grant_is_read_in_place_without_a_copy() {
        let (mut producer, mut consumer) = tokio_grant_lane(512, 2);

        let granted = producer.grant().await;
        granted.fill(b"the frame is written once");
        let written_at = granted.bytes.as_ptr() as usize;
        producer.commit();

        let received = consumer.peek().await;
        assert_eq!(received.frame(), b"the frame is written once");
        assert_eq!(
            received.bytes.as_ptr() as usize,
            written_at,
            "the consumer reads the very slot the producer filled",
        );
        received.frame_mut()[0] ^= 0x20;
        assert_eq!(&received.frame()[..3], b"The");
        consumer.release();
    }

    #[test]
    fn a_burst_earns_one_announcement_until_the_consumer_acknowledges() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 8);

        producer.try_grant().expect("lane grants").fill(b"one");
        producer.commit();
        assert!(producer.needs_announce(), "the first commit announces");

        producer.try_grant().expect("lane grants").fill(b"two");
        producer.commit();
        assert!(
            !producer.needs_announce(),
            "a burst behind an unconsumed announcement stays silent",
        );

        consumer.acknowledge();
        while consumer.try_peek().is_some() {
            consumer.release();
        }

        producer.try_grant().expect("lane grants").fill(b"three");
        producer.commit();
        assert!(
            producer.needs_announce(),
            "a commit after the acknowledge announces again",
        );
    }

    #[tokio::test]
    async fn a_full_lane_refuses_grants_until_the_consumer_releases() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 1);

        producer
            .try_grant()
            .expect("an empty lane grants")
            .fill(b"one");
        producer.commit();
        assert!(producer.try_grant().is_none(), "a depth-one lane is full");

        consumer.try_peek().expect("the committed frame is there");
        consumer.release();
        assert!(
            producer.try_grant().is_some(),
            "the release frees the slot for the next grant",
        );
    }

    #[tokio::test]
    async fn a_loopback_frame_crosses_the_seam_and_the_rebroadcast_leaves_through_the_peer() {
        let source = InterfaceId::new([0xA1; 8]);
        let peer = InterfaceId::new([0xB2; 8]);
        let interfaces = std::vec![descriptor(source), descriptor(peer)];

        let mut engine = EngineState::<TestStorageLayout>::default();
        pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

        // One inbound funnel shared by both interfaces.
        let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);

        // The source interface: the test plays an announce onto its wire; its seam
        // funnels the frame to the reactor.
        let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_wire_out_tx, _source_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let source_iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_seam =
            TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

        // The peer interface: the rebroadcast must leave through *its* wire.
        let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_wire_out_tx, mut peer_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let peer_iface = LoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);

        drop(notify_tx);

        let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);

        let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<()>();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                let _ = heard_tx.send(());
            }
            Journaled::Delivered(_)
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::CommandSettled { .. }
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::LinkInterfaceMismatch { .. } => {}
        };

        tokio::spawn(run(
            engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces,
                ifacs: std::vec![],
                notify: notify_rx,
                inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
                commands: command_rx,
                egress,
            },
            app,
        ));
        tokio::spawn(source_iface.run(source_seam));
        tokio::spawn(peer_iface.run(peer_seam));

        // Idle: the loopback wires are quiet, the reactor journals and emits nothing.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            heard_rx.try_recv().is_err(),
            "an idle reactor journals nothing"
        );
        assert!(
            peer_wire_out_rx.try_recv().is_err(),
            "an idle interface transmits nothing"
        );

        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let original_hops = WirePacketHeader::parse(&raw)
            .expect("valid announce wire")
            .0
            .hops;
        // Play the announce onto the source interface's wire — it crosses the seam itself.
        source_wire_in_tx
            .send(raw)
            .expect("the source interface holds its wire");

        tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
            .await
            .expect("the deposited frame journals within the window")
            .expect("the reactor task is alive");

        // The rebroadcast leaves through the peer interface's wire: egress -> peer seam ->
        // next_outbound -> the peer's write.
        let bytes = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
            .await
            .expect("the rebroadcast reaches the peer's wire within the window")
            .expect("the peer interface task is alive");
        let (header, _) = WirePacketHeader::parse(&bytes).expect("valid rebroadcast wire");
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.hops,
            original_hops + 1,
            "the rebroadcast bumps the hop count"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_capped_link_holds_a_rebroadcast_burst_then_drains_it_over_time() {
        let source = InterfaceId::new([0xA1; 8]);
        let peer = InterfaceId::new([0xB2; 8]);
        let slow_peer = InterfaceDescriptor {
            bitrate: BitrateBps::guess(1_000),
            announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
            ..descriptor(peer)
        };
        let interfaces = std::vec![descriptor(source), slow_peer];

        let mut engine = EngineState::<TestStorageLayout>::default();
        pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

        let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);

        let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_wire_out_tx, _source_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let source_iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_seam =
            TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

        let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_wire_out_tx, mut peer_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let peer_iface = LoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);

        drop(notify_tx);

        let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();

        tokio::spawn(run(
            engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces,
                ifacs: std::vec![],
                notify: notify_rx,
                inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
                commands: command_rx,
                egress,
            },
            |_journaled: Journaled<'_>| {},
        ));
        tokio::spawn(source_iface.run(source_seam));
        tokio::spawn(peer_iface.run(peer_seam));

        source_wire_in_tx
            .send(bytes_from_hex(RNS_1_3_5_ANNOUNCE))
            .expect("the source interface holds its wire");
        source_wire_in_tx
            .send(bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE))
            .expect("the source interface holds its wire");

        let first = tokio::time::timeout(Duration::from_secs(5), peer_wire_out_rx.recv())
            .await
            .expect("the first rebroadcast leaves the idle link within the window")
            .expect("the peer task is alive");
        assert_eq!(
            WirePacketHeader::parse(&first).unwrap().0.packet_type,
            PacketType::Announce
        );

        assert!(
            tokio::time::timeout(Duration::from_secs(5), peer_wire_out_rx.recv())
                .await
                .is_err(),
            "the cap holds the second rebroadcast far short of its spacing window",
        );

        let second = tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
            .await
            .expect("the held rebroadcast drains once the spacing window passes")
            .expect("the peer task is alive");
        assert_eq!(
            WirePacketHeader::parse(&second).unwrap().0.packet_type,
            PacketType::Announce
        );
        assert_ne!(first, second, "the two rebroadcasts are distinct announces");
    }

    #[tokio::test(start_paused = true)]
    async fn the_reactor_re_emits_a_rebroadcast_once_more_then_retires_it() {
        let source = InterfaceId::new([0xA1; 8]);
        let peer = InterfaceId::new([0xB2; 8]);
        let interfaces = std::vec![descriptor(source), descriptor(peer)];

        let mut engine = EngineState::<TestStorageLayout>::default();
        pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

        let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);

        let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_wire_out_tx, _source_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let source_iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_seam =
            TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

        let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_wire_out_tx, mut peer_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let peer_iface = LoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);

        drop(notify_tx);

        let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();

        tokio::spawn(run(
            engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces,
                ifacs: std::vec![],
                notify: notify_rx,
                inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
                commands: command_rx,
                egress,
            },
            |_journaled: Journaled<'_>| {},
        ));
        tokio::spawn(source_iface.run(source_seam));
        tokio::spawn(peer_iface.run(peer_seam));

        source_wire_in_tx
            .send(bytes_from_hex(RNS_1_3_5_ANNOUNCE))
            .expect("the source interface holds its wire");

        let first = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
            .await
            .expect("the first emission leaves within the jitter window")
            .expect("the peer task is alive");

        assert!(
            tokio::time::timeout(Duration::from_secs(4), peer_wire_out_rx.recv())
                .await
                .is_err(),
            "the second emission waits the full retransmit interval",
        );

        let second = tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
            .await
            .expect("the reactor re-emits once the retransmit interval passes")
            .expect("the peer task is alive");
        assert_eq!(
            first, second,
            "the retransmit re-emits the same pinned announce, byte for byte",
        );

        assert!(
            tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
                .await
                .is_err(),
            "after two emissions the reactor retires the entry",
        );
    }

    #[tokio::test]
    async fn a_delivery_answers_with_a_proof_directive_on_the_arrival_lane() {
        use crate::crypto::X25519SecretKey;
        use crate::engine::RatchetPolicy;
        use crate::identity::in_memory::InMemoryNodeIdentity;
        use crate::identity::{IdentitySigner, RemoteIdentity, Zeroizing};
        use crate::routing::dedup::PacketHash;
        use crate::routing::proof::IMPLICIT_PROOF_WIRE_LEN;
        use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
        use crate::wire::{
            ContextFlag, DestinationType, IfacFlag, PropagationType, WireContext, BROADCAST_MTU,
        };

        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        let secret = Zeroizing::new(secret);

        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
        let mut engine = EngineState::<TestStorageLayout>::new(secret);
        let destination = engine
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the single destination");

        let remote = RemoteIdentity::from_public_keys(
            identity.encryption_public_key(),
            identity.signing_public_key(),
        );
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            address: destination.to_address(),
            context: WireContext::None,
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut wire).expect("writes the header");
        let sealed = remote
            .encrypt(
                &X25519SecretKey::new([0x77; 32]),
                &[0x88; 16],
                b"prove-through-the-stack",
                &mut wire[header_len..],
            )
            .expect("seals the payload");
        let raw = wire[..header_len + sealed].to_vec();
        let packet_hash = PacketHash::of_wire_packet(&raw).expect("hashes the wire packet");

        let mut expected_proof = std::vec::Vec::new();
        expected_proof.push(0x03);
        expected_proof.push(0x00);
        expected_proof.extend_from_slice(packet_hash.proof_destination().as_bytes());
        expected_proof.push(0x00);
        expected_proof.extend_from_slice(&identity.sign(packet_hash.as_bytes()).0);
        assert_eq!(expected_proof.len(), IMPLICIT_PROOF_WIRE_LEN);

        let source = InterfaceId::new([0xA1; 8]);
        let interfaces = std::vec![descriptor(source)];

        let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (mut source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (source_out_tx, mut source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let egress = Egress::new(std::vec![(source, source_out_tx)]);

        let (delivered_tx, mut delivered_rx) = mpsc::unbounded_channel::<()>();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::Delivered(_) => {
                let _ = delivered_tx.send(());
            }
            Journaled::AnnounceHeard { .. }
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::CommandSettled { .. }
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::LinkInterfaceMismatch { .. } => {}
        };

        tokio::spawn(run(
            engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces,
                ifacs: std::vec![],
                notify: notify_rx,
                inbound_lanes: std::vec![(source, source_in_rx)],
                commands: command_rx,
                egress,
            },
            app,
        ));

        source_in_tx
            .try_grant()
            .expect("an empty lane grants")
            .fill(&raw);
        source_in_tx.commit();
        notify_tx
            .send(source)
            .expect("the reactor task holds the receiver");

        tokio::time::timeout(Duration::from_secs(2), delivered_rx.recv())
            .await
            .expect("the delivery journals within the window")
            .expect("the reactor task is alive");

        let frame = tokio::time::timeout(Duration::from_secs(2), source_out_rx.peek())
            .await
            .expect("the owed proof is emitted within the window");
        assert_eq!(
            frame.frame(),
            expected_proof,
            "the proof is byte-identical to the RNS 1.3.5 implicit proof, on the arrival lane"
        );
    }

    #[tokio::test]
    async fn ifac_members_hear_each_other_and_strangers_stay_outside() {
        use crate::interfaces::ifac::IfacContext;
        use crate::wire::DestinationHash;

        let source = InterfaceId::new([0xA1; 8]);
        let peer = InterfaceId::new([0xB2; 8]);
        let interfaces = std::vec![descriptor(source), descriptor(peer)];
        let mut engine = EngineState::<TestStorageLayout>::default();
        pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

        let network = || {
            IfacContext::derive(
                Some("testnet"),
                Some("s3cret"),
                crate::interfaces::ifac::IfacSize::NARROW,
            )
            .unwrap()
        };
        let ifacs = std::vec![
            InterfaceIfac {
                id: source,
                context: network(),
            },
            InterfaceIfac {
                id: peer,
                context: network(),
            },
        ];

        let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_wire_out_tx, _source_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let source_iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_seam =
            TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

        let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_wire_out_tx, mut peer_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let peer_iface = LoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);
        drop(notify_tx);

        let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let app = move |journaled: Journaled<'_>| {
            if let Journaled::AnnounceHeard { destination, .. } = journaled {
                let _ = heard_tx.send(destination);
            }
        };

        tokio::spawn(run(
            engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces,
                ifacs,
                notify: notify_rx,
                inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
                commands: command_rx,
                egress,
            },
            app,
        ));
        tokio::spawn(source_iface.run(source_seam));
        tokio::spawn(peer_iface.run(peer_seam));

        let clean = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut member_wire = std::vec![0u8; MAX_WIRE_FRAME_LEN];
        let masked_len = network().mask_outbound(&clean, &mut member_wire).unwrap();
        source_wire_in_tx
            .send(member_wire[..masked_len].to_vec())
            .expect("the source interface holds its wire");

        let heard = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
            .await
            .expect("a member's masked announce is heard")
            .expect("the reactor task is alive");
        assert_eq!(
            heard.as_bytes(),
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314").as_slice(),
        );

        let rebroadcast = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
            .await
            .expect("the rebroadcast leaves through the peer")
            .expect("the peer task is alive");
        assert_eq!(
            rebroadcast[0] & 0x80,
            0x80,
            "the peer's wire only ever carries flagged, masked frames",
        );
        let mut recovered = std::vec![0u8; MAX_WIRE_FRAME_LEN];
        let clean_len = network()
            .unmask_inbound(&rebroadcast, &mut recovered)
            .expect("a member can open the rebroadcast");
        let (header, _) = WirePacketHeader::parse(&recovered[..clean_len]).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.hops, 1,
            "the relay bumped the hop count under the mask"
        );

        let stranger = IfacContext::derive(
            Some("testnet"),
            Some("wrong"),
            crate::interfaces::ifac::IfacSize::NARROW,
        )
        .unwrap();
        let mut stranger_wire = std::vec![0u8; MAX_WIRE_FRAME_LEN];
        let stranger_len = stranger
            .mask_outbound(
                &bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE),
                &mut stranger_wire,
            )
            .unwrap();
        source_wire_in_tx
            .send(stranger_wire[..stranger_len].to_vec())
            .expect("the source interface holds its wire");
        assert!(
            tokio::time::timeout(Duration::from_secs(1), heard_rx.recv())
                .await
                .is_err(),
            "a stranger's code opens nothing",
        );
    }

    #[tokio::test]
    async fn dynamic_ifac_state_arrives_and_leaves_with_its_interface() {
        use crate::interfaces::ifac::{IfacContext, IfacSize};
        use crate::wire::DestinationHash;

        let source = InterfaceId::new([0xD4; 8]);
        let mut engine = EngineState::<TestStorageLayout>::default();
        pin_transport_id(&mut engine, TEST_TRANSPORT_ID);
        let network =
            IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::NARROW).unwrap();

        let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let app = move |journaled: Journaled<'_>| {
            if let Journaled::AnnounceHeard { destination, .. } = journaled {
                let _ = heard_tx.send(destination);
            }
        };

        tokio::spawn(run(
            engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces: std::vec![],
                ifacs: std::vec![],
                notify: notify_rx,
                inbound_lanes: std::vec![],
                commands: command_rx,
                egress: Egress::new(std::vec![]),
            },
            app,
        ));

        let (mut protected_in, protected_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (protected_out, _protected_wire) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        command_tx
            .send(HostCommand::AddInterface(AddInterfaceCommand {
                descriptor: descriptor(source),
                logical_interface: source,
                inbound: protected_rx,
                egress: protected_out,
                connection: None,
                ifac: Some(network.clone()),
            }))
            .unwrap();
        tokio::task::yield_now().await;

        let clean = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut masked = std::vec![0u8; MAX_WIRE_FRAME_LEN];
        let masked_len = network.mask_outbound(&clean, &mut masked).unwrap();
        protected_in
            .try_grant()
            .unwrap()
            .fill(&masked[..masked_len]);
        protected_in.commit();
        notify_tx.send(source).unwrap();
        tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
            .await
            .unwrap()
            .unwrap();

        command_tx
            .send(HostCommand::RemoveInterface {
                id: source,
                departure: Departure::MayReturn,
            })
            .unwrap();
        let (mut open_in, open_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (open_out, _open_wire) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        command_tx
            .send(HostCommand::AddInterface(AddInterfaceCommand {
                descriptor: descriptor(source),
                logical_interface: source,
                inbound: open_rx,
                egress: open_out,
                connection: None,
                ifac: None,
            }))
            .unwrap();
        tokio::task::yield_now().await;

        let open = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        open_in.try_grant().unwrap().fill(&open);
        open_in.commit();
        notify_tx.send(source).unwrap();
        tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn the_reactor_culls_an_expired_route_at_its_deadline() {
        use crate::engine::{
            CommandId, EngineCommand, SendSinglePacket, SendSinglePacketFailure,
            SendSinglePacketPayload, SendSinglePacketRejection, Settlement,
        };
        use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
        use crate::wire::DestinationHash;

        let source = InterfaceId::new([0xA1; 8]);
        let interfaces = std::vec![descriptor(source)];
        let engine = EngineState::<TestStorageLayout>::default();

        let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (wire_in_tx, wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (wire_out_tx, _wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (out_tx, out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: wire_in_rx,
            wire_out: wire_out_tx,
        };
        let seam = TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), out_rx);
        drop(notify_tx);
        let egress = Egress::new(std::vec![(source, out_tx)]);
        let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();

        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let (expired_tx, mut expired_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { destination, .. } => {
                let _ = heard_tx.send(destination);
            }
            Journaled::RouteRemoved {
                destination,
                cause: RouteRemovalCause::Expired,
            } => {
                let _ = expired_tx.send(destination);
            }
            Journaled::CommandSettled { id, settlement } => {
                let _ = settled_tx.send((id, settlement));
            }
            Journaled::Delivered(_)
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::LinkInterfaceMismatch { .. } => {}
        };

        tokio::spawn(run(
            engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces,
                ifacs: std::vec![],
                notify: notify_rx,
                inbound_lanes: std::vec![(source, source_in_rx)],
                commands: command_rx,
                egress,
            },
            app,
        ));
        tokio::spawn(iface.run(seam));

        wire_in_tx
            .send(bytes_from_hex(RNS_1_3_5_ANNOUNCE))
            .expect("the interface holds its wire");
        let destination = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
            .await
            .expect("the announce is heard, so the route exists before the deadline")
            .expect("the reactor task is alive");

        tokio::time::sleep(Duration::from_millis(DEFAULT_ROUTE_EXPIRY_MILLIS + 10_000)).await;

        let expired = tokio::time::timeout(Duration::from_secs(2), expired_rx.recv())
            .await
            .expect("the cull journals the removal at the expiry deadline")
            .expect("the reactor task is alive");
        assert_eq!(
            expired, destination,
            "the expired route names its destination"
        );

        command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(3),
                command: EngineCommand::SendSinglePacket(SendSinglePacket {
                    destination,
                    payload: SendSinglePacketPayload::from_slice(b"late").expect("fits the MDU"),
                }),
            }))
            .expect("the reactor task holds the receiver");

        let (settled_id, settlement) =
            tokio::time::timeout(Duration::from_secs(2), settled_rx.recv())
                .await
                .expect("the late send settles")
                .expect("the reactor task is alive");
        assert_eq!(settled_id, CommandId(3));
        assert_eq!(
            settlement,
            Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Rejected(
                SendSinglePacketRejection::NoRouteToDestination
            ))),
            "the reactor woke at the route's expiry and culled it",
        );
    }

    #[tokio::test]
    async fn a_commanded_announce_fans_to_every_interface_and_settles() {
        use crate::engine::{
            AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, RatchetPolicy,
            Settlement,
        };
        use crate::identity::Zeroizing;
        use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};

        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        let mut engine = EngineState::<TestStorageLayout>::new(Zeroizing::new(secret));
        let node = engine.held_identity_hashes()[0];
        let destination = engine
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the single destination");

        let first = InterfaceId::new([0xA1; 8]);
        let second = InterfaceId::new([0xB2; 8]);
        let interfaces = std::vec![descriptor(first), descriptor(second)];

        let (_notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (first_out_tx, mut first_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (second_out_tx, mut second_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let egress = Egress::new(std::vec![(first, first_out_tx), (second, second_out_tx)]);

        let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::CommandSettled { id, settlement } => {
                let _ = settled_tx.send((id, settlement));
            }
            Journaled::AnnounceHeard { .. }
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::Delivered(_)
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::LinkInterfaceMismatch { .. } => {}
        };

        tokio::spawn(run(
            engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces,
                ifacs: std::vec![],
                notify: notify_rx,
                inbound_lanes: std::vec![],
                commands: command_rx,
                egress,
            },
            app,
        ));

        command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            }))
            .expect("the reactor task holds the receiver");

        let (settled_id, settlement) =
            tokio::time::timeout(Duration::from_secs(2), settled_rx.recv())
                .await
                .expect("the command settles within the window")
                .expect("the reactor task is alive");
        assert_eq!(settled_id, CommandId(7));
        assert_eq!(settlement, Settlement::AnnounceNow(Ok(())));

        for out_rx in [&mut first_out_rx, &mut second_out_rx] {
            let frame = tokio::time::timeout(Duration::from_secs(2), out_rx.peek())
                .await
                .expect("an announce fires on each interface");
            let (header, _) = WirePacketHeader::parse(frame.frame()).expect("valid announce wire");
            assert_eq!(header.packet_type, PacketType::Announce);
            assert_eq!(DestinationHash::from_address(header.address), destination);
        }

        #[cfg(feature = "runtime-metrics")]
        {
            let (reply, snapshot) = oneshot::channel();
            command_tx
                .send(HostCommand::SnapshotMetrics { reply })
                .expect("the reactor task holds the receiver");
            let snapshot = snapshot.await.expect("the reactor returns its metrics");
            assert_eq!(
                snapshot
                    .engine
                    .announces
                    .commands
                    .get(crate::engine::AnnounceCommandOutcome::Succeeded),
                1
            );
            assert_eq!(
                snapshot
                    .egress
                    .announces
                    .outcomes
                    .get(AnnounceOrigin::Local, AnnounceEgressOutcome::Enqueued),
                2
            );
        }
    }

    #[tokio::test]
    async fn a_link_establishes_and_carries_data_across_two_live_reactors() {
        use crate::engine::test_support::{personal_node_destination, second_secret_key};
        use crate::engine::{
            AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EstablishLink,
            LinkEstablished, RatchetPolicy, SendToLink, SendToLinkFailure, SendToLinkPayload,
            Settlement,
        };
        use crate::routing::delivery::Delivery;
        use crate::routing::links::LinkId;
        use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};

        let initiator_iface = InterfaceId::new([0xA1; 8]);
        let responder_iface = InterfaceId::new([0xB2; 8]);

        let (a_to_b_tx, a_to_b_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (b_to_a_tx, b_to_a_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();

        let initiator_engine = EngineState::<TestStorageLayout>::new(second_secret_key());
        let (a_notify_tx, a_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (a_in_tx, a_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (a_out_tx, a_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let a_iface = LoopbackInterface {
            descriptor: descriptor(initiator_iface),
            wire_in: b_to_a_rx,
            wire_out: a_to_b_tx,
        };
        let a_seam = TokioInterfaceSeam::new(initiator_iface, a_in_tx, a_notify_tx, a_out_rx);
        let a_egress = Egress::new(std::vec![(initiator_iface, a_out_tx)]);
        let (a_command_tx, a_command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (a_heard_tx, mut a_heard_rx) = mpsc::unbounded_channel::<()>();
        let (a_settled_tx, mut a_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let (a_delivered_tx, mut a_delivered_rx) =
            mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
        let a_app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                let _ = a_heard_tx.send(());
            }
            Journaled::CommandSettled { id, settlement } => {
                let _ = a_settled_tx.send((id, settlement));
            }
            Journaled::Delivered(Delivery::Link(link)) => {
                let _ = a_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
            }
            _ => {}
        };

        let responder_engine = {
            use crate::engine::test_support::fixed_secret_key;
            let mut engine: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
            let node = engine.held_identity_hashes()[0];
            engine
                .register_single_destination(
                    &node,
                    "personal",
                    &["node"],
                    b"hello-personal",
                    ProofStrategy::ProveAll,
                    LinkRequestPolicy::AcceptAll,
                    RatchetPolicy::NoRatchets,
                )
                .expect("registers the proving destination");
            engine
        };
        let (b_notify_tx, b_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (b_in_tx, b_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (b_out_tx, b_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let b_iface = LoopbackInterface {
            descriptor: descriptor(responder_iface),
            wire_in: a_to_b_rx,
            wire_out: b_to_a_tx,
        };
        let b_seam = TokioInterfaceSeam::new(responder_iface, b_in_tx, b_notify_tx, b_out_rx);
        let b_egress = Egress::new(std::vec![(responder_iface, b_out_tx)]);
        let (b_command_tx, b_command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (b_established_tx, mut b_established_rx) = mpsc::unbounded_channel::<LinkEstablished>();
        let (b_settled_tx, mut b_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let (b_delivered_tx, mut b_delivered_rx) =
            mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
        let b_app = move |journaled: Journaled<'_>| match journaled {
            Journaled::LinkEstablished(established) => {
                let _ = b_established_tx.send(established);
            }
            Journaled::CommandSettled { id, settlement } => {
                let _ = b_settled_tx.send((id, settlement));
            }
            Journaled::Delivered(Delivery::Link(link)) => {
                let _ = b_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
            }
            _ => {}
        };

        tokio::spawn(run(
            initiator_engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces: std::vec![descriptor(initiator_iface)],
                ifacs: std::vec![],
                notify: a_notify_rx,
                inbound_lanes: std::vec![(initiator_iface, a_in_rx)],
                commands: a_command_rx,
                egress: a_egress,
            },
            a_app,
        ));
        tokio::spawn(run(
            responder_engine,
            TokioHost::new(),
            ReactorWiring {
                interfaces: std::vec![descriptor(responder_iface)],
                ifacs: std::vec![],
                notify: b_notify_rx,
                inbound_lanes: std::vec![(responder_iface, b_in_rx)],
                commands: b_command_rx,
                egress: b_egress,
            },
            b_app,
        ));
        tokio::spawn(a_iface.run(a_seam));
        tokio::spawn(b_iface.run(b_seam));

        b_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(1),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            }))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), a_heard_rx.recv())
            .await
            .expect("the announce crosses the wire")
            .expect("the initiator reactor is alive");

        a_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::EstablishLink(EstablishLink {
                    destination: personal_node_destination(),
                }),
            }))
            .unwrap();

        let (settled_id, settlement) =
            tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
                .await
                .expect("the link settles within the window")
                .expect("the initiator reactor is alive");
        assert_eq!(settled_id, CommandId(7));
        let Settlement::EstablishLink(Ok(established)) = settlement else {
            panic!("the command must settle established, got {settlement:?}");
        };

        let responder_side = tokio::time::timeout(Duration::from_secs(5), b_established_rx.recv())
            .await
            .expect("the responder journals the link up")
            .expect("the responder reactor is alive");
        assert_eq!(
            responder_side.link_id, established.link_id,
            "one link, two ends",
        );
        assert!(
            responder_side.rtt_ms >= established.rtt_ms,
            "the responder takes max(measured, reported)",
        );

        a_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(8),
                command: EngineCommand::SendToLink(SendToLink {
                    link_id: established.link_id,
                    payload: SendToLinkPayload::from_slice(b"ping over the live link").unwrap(),
                }),
            }))
            .unwrap();
        let delivered = tokio::time::timeout(Duration::from_secs(5), b_delivered_rx.recv())
            .await
            .expect("the responder journals the delivery")
            .expect("the responder reactor is alive");
        assert_eq!(
            delivered,
            (established.link_id, b"ping over the live link".to_vec()),
        );
        let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
            .await
            .expect("the initiator's send settles")
            .expect("the initiator reactor is alive");
        assert_eq!(sent_id, CommandId(8));
        let Settlement::SendToLink(Ok(_delivered_receipt)) = sent else {
            panic!("the ProveAll responder's proof settles the send Delivered, got {sent:?}");
        };

        b_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(2),
                command: EngineCommand::SendToLink(SendToLink {
                    link_id: established.link_id,
                    payload: SendToLinkPayload::from_slice(b"pong right back").unwrap(),
                }),
            }))
            .unwrap();
        let sent = loop {
            let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), b_settled_rx.recv())
                .await
                .expect("the responder's send settles")
                .expect("the responder reactor is alive");
            if sent_id == CommandId(2) {
                break sent;
            }
        };
        assert_eq!(
            sent,
            Settlement::SendToLink(Err(SendToLinkFailure::Timeout)),
            "the initiator's side never proves, so the responder's send times out — parity",
        );
        let delivered = tokio::time::timeout(Duration::from_secs(5), a_delivered_rx.recv())
            .await
            .expect("the initiator journals the delivery")
            .expect("the initiator reactor is alive");
        assert_eq!(
            delivered,
            (established.link_id, b"pong right back".to_vec()),
        );
    }
}
