use core::cell::RefCell;
use prns_core::interfaces::{AttachedInterfaces, IndexedAttachedInterfaces};
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
};
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{
    AirtimeUtilization, ConnectionState, InboundPacket, InterfaceDescriptor, InterfaceId,
    InterfaceKind, InterfaceStatus, TransferRates,
};
use crate::reactor::announce_pacer::{AnnouncePacer, HeapPacerQueue};
use crate::reactor::driver::{fire_due_reason, merge_wake_schedules_delta, wait_for_pacer};
use crate::reactor::interface_seam::{
    frame_cap_for, InterfaceSeam, BROADCAST_WIRE_FRAME_LEN, MAX_WIRE_FRAME_LEN,
};
use crate::reactor::Host;
use crate::routing::announce::Announce;
use crate::routing::dedup::PacketHash;
use crate::routing::ingress::MAX_RATCHET_DECRYPT_PAYLOAD_LEN;
use crate::routing::links::channel::byte_stream::{self, StreamId, STREAM_DATA_TYPE};
use crate::routing::links::handshake::{
    link_proof_signature_valid, link_proof_signed_data, LinkProofSignOwed, LinkProofVerifyOwed,
};
use crate::routing::links::request::{write_request_plaintext, RequestId, REQUEST_WIRE_OVERHEAD};
use crate::routing::links::resources::{
    ResourceBody, ResourceCorrelation, ResourceHash, ResourceMetadata, ResourceSegment,
    ResourceSend, ResourceStrategy,
};
use crate::routing::links::LinkId;
use crate::routing::proof::IMPLICIT_PROOF_WIRE_LEN;
use crate::routing::request_handlers::RequestPathHash;
use crate::runtime::InterfaceStore;
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::units::RttMillis;
use crate::wire::{DestinationHash, PacketType, WirePacketHeader};
use heapless::Vec as HeaplessVec;

pub struct TokioHost {
    base: Instant,
}

impl TokioHost {
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
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
        InstantMillis(self.base.elapsed().as_millis() as u64)
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        tokio::time::sleep_until(self.base + Duration::from_millis(deadline.0)).await;
    }

    #[allow(clippy::expect_used)]
    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        getrandom::getrandom(bytes).expect("OS CSPRNG must provide reactor entropy");
    }
}

/// One interface's grant lane on tokio: each slot is a heap-backed frame buffer recycled
/// through a pair of bounded channels, so granting, committing, and releasing move the slot by
/// value (a pointer and a length) while the frame bytes stay where they were written.
pub fn tokio_grant_lane(slot_cap: usize, depth: usize) -> (TokioGrantProducer, TokioGrantConsumer) {
    let (filled_tx, filled_rx) = tokio::sync::mpsc::channel(depth.max(1));
    let (free_tx, free_rx) = tokio::sync::mpsc::channel(depth.max(1));
    for _ in 0..depth.max(1) {
        let _ = free_tx.try_send(HeapFrameSlot::empty(slot_cap));
    }
    (
        TokioGrantProducer {
            free: free_rx,
            filled: filled_tx,
            granted: None,
        },
        TokioGrantConsumer {
            filled: filled_rx,
            free: free_tx,
            peeked: None,
        },
    )
}

/// A heap-backed wire frame slot. Its buffer grows to the frame it holds and keeps that
/// capacity across reuse, so a deep host lane costs only the frames actually in flight rather
/// than depth times the MTU ceiling. The slot moves by value through the lane's channels
/// (moving a `Vec` carries its allocation, not a copy); `len` mirrors `bytes.len()` for the seam's frame view.
pub struct HeapFrameSlot {
    pub len: usize,
    pub bytes: Vec<u8>,
}

impl HeapFrameSlot {
    fn empty(_cap: usize) -> Self {
        Self {
            len: 0,
            bytes: Vec::new(),
        }
    }

    pub fn fill(&mut self, frame: &[u8]) {
        self.bytes.clear();
        self.bytes.extend_from_slice(frame);
        self.len = frame.len();
    }

    pub fn frame(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn frame_mut(&mut self) -> &mut [u8] {
        let len = self.len;
        &mut self.bytes[..len]
    }
}

pub struct TokioGrantProducer {
    free: tokio::sync::mpsc::Receiver<HeapFrameSlot>,
    filled: tokio::sync::mpsc::Sender<HeapFrameSlot>,
    granted: Option<HeapFrameSlot>,
}

impl TokioGrantProducer {
    pub fn try_grant(&mut self) -> Option<&mut HeapFrameSlot> {
        if self.granted.is_none() {
            self.granted = Some(self.free.try_recv().ok()?);
        }
        self.granted.as_mut()
    }

    pub async fn grant(&mut self) -> &mut HeapFrameSlot {
        loop {
            if let Some(slot) = self.granted.take() {
                return self.granted.insert(slot);
            }
            match self.free.recv().await {
                Some(slot) => self.granted = Some(slot),
                None => core::future::pending().await,
            }
        }
    }

    pub fn commit(&mut self) {
        if let Some(slot) = self.granted.take() {
            let _ = self.filled.try_send(slot);
        }
    }
}

pub struct TokioGrantConsumer {
    filled: tokio::sync::mpsc::Receiver<HeapFrameSlot>,
    free: tokio::sync::mpsc::Sender<HeapFrameSlot>,
    peeked: Option<HeapFrameSlot>,
}

impl TokioGrantConsumer {
    pub fn try_peek(&mut self) -> Option<&mut HeapFrameSlot> {
        if self.peeked.is_none() {
            self.peeked = Some(self.filled.try_recv().ok()?);
        }
        self.peeked.as_mut()
    }

    pub async fn peek(&mut self) -> &mut HeapFrameSlot {
        loop {
            if let Some(slot) = self.peeked.take() {
                return self.peeked.insert(slot);
            }
            match self.filled.recv().await {
                Some(slot) => self.peeked = Some(slot),
                None => core::future::pending().await,
            }
        }
    }

    pub fn release(&mut self) {
        if let Some(slot) = self.peeked.take() {
            let _ = self.free.try_send(slot);
        }
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
    async fn next_inbound(&mut self, frame: &[u8]) {
        self.inbound.grant().await.fill(frame);
        self.inbound.commit();
        let _ = self.notify.send(self.id);
    }

    async fn next_outbound(&mut self) -> &[u8] {
        self.outbound.release();
        self.outbound.peek().await.frame()
    }

    async fn request_tunnel_synthesis(&mut self) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(HostCommand::SynthesizeTunnel { interface: self.id });
        }
    }
}

/// The reactor's egress: it routes each engine `Directive::Send` to the target
/// interface's outbound queue. The senders are cheap clones, so `enqueue` is `&self`
/// and the copy-into-frame is synchronous — exactly what the lent-buffer borrow demands.
pub struct Egress {
    lanes: std::vec::Vec<(InterfaceId, std::sync::Mutex<TokioGrantProducer>)>,
}

impl Egress {
    #[must_use]
    pub fn new(lanes: std::vec::Vec<(InterfaceId, TokioGrantProducer)>) -> Self {
        Self {
            lanes: lanes
                .into_iter()
                .map(|(id, producer)| (id, std::sync::Mutex::new(producer)))
                .collect(),
        }
    }

    fn enqueue(&self, target: InterfaceId, bytes: &[u8]) {
        for (id, producer) in &self.lanes {
            if *id == target {
                let Ok(mut producer) = producer.lock() else {
                    return;
                };
                if let Some(slot) = producer.try_grant() {
                    slot.fill(bytes);
                    producer.commit();
                }
                return;
            }
        }
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
            .map(|(id, _)| *id)
            .filter(|id| id.kind() == member)
            .filter(|id| match fan {
                FanTarget::All => true,
                FanTarget::Only(only) => *id == only,
                FanTarget::AllExcept(except) => *id != except,
            })
            .collect()
    }

    /// Grant-first: size the target lane's next free slot to the engine's `size_hint` and let `fill`
    /// seal the frame in place — sealed once, never staged and copied. A full lane, an unknown
    /// target, or a poisoned lane still runs `fill` once against `discard` and drops the result — the
    /// `EmitFrame` contract — so the engine's bookkeeping never depends on lane luck.
    fn emit(
        &self,
        target: InterfaceId,
        size_hint: usize,
        fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
        discard: &mut [u8],
    ) {
        for (id, producer) in &self.lanes {
            if *id == target {
                let Ok(mut producer) = producer.lock() else {
                    let _ = fill(discard);
                    return;
                };
                match producer.try_grant() {
                    Some(slot) => {
                        slot.bytes.resize(size_hint.clamp(1, MAX_WIRE_FRAME_LEN), 0);
                        if let Some(len) = fill(&mut slot.bytes) {
                            slot.len = len.min(slot.bytes.len());
                            slot.bytes.truncate(slot.len);
                            producer.commit();
                        }
                    }
                    None => {
                        let _ = fill(discard);
                    }
                }
                return;
            }
        }
        let _ = fill(discard);
    }

    fn add_lane(&mut self, id: InterfaceId, producer: TokioGrantProducer) {
        self.lanes.push((id, std::sync::Mutex::new(producer)));
    }

    fn remove_lane(&mut self, id: InterfaceId) {
        self.lanes.retain(|(lane_id, _)| *lane_id != id);
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
    pub inbound: TokioGrantConsumer,
    pub egress: TokioGrantProducer,
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
/// parked `request()` stashes its bytes; the matching `SendRequest` settlement then fires
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
    run_with_proof_decider(engine, host, wiring, on_journaled, |_: &ProofRequest| false).await
}

struct EngineVerifyJob {
    packet_hash: PacketHash,
    signing_key: IdentitySigningPublicKey,
    signature: Ed25519Signature,
    id: CommandId,
    settlement: Settlement,
}

/// One unit of asymmetric crypto handed off the engine thread (one saturated pool, not one per
/// op): `Verify` is an inbound proof's Ed25519 check, `SealScalars` the two X25519 scalar
/// mults an outbound single's seal needs, `Sign` the Ed25519 signature an inbound delivery's
/// proof owes. The seal job carries the whole obligation so the reactor keeps no per-in-flight
/// side table; it is transient and its large variant is the common one, so it is not boxed.
#[allow(clippy::large_enum_variant)]
enum CryptoJob {
    Verify(EngineVerifyJob),
    SealScalars(EncryptOwed),
    Sign(DeferredProofSign),
    Decrypt(DecryptOwed),
    DecryptWithRatchets(Box<RatchetDecryptOwed>),
    VerifyLinkProof(LinkProofVerifyOwed),
    SignLinkProof(LinkProofSignOwed),
    VerifyAnnounce(AnnounceVerifyOwed),
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
}

struct CryptoQueue {
    jobs: Mutex<VecDeque<CryptoJob>>,
    len: AtomicUsize,
    ready: Condvar,
}

struct CryptoPool {
    queue: Arc<CryptoQueue>,
}

impl CryptoPool {
    fn spawn(workers: usize, results: UnboundedSender<CryptoResult>) -> Self {
        let queue = Arc::new(CryptoQueue {
            jobs: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
            ready: Condvar::new(),
        });
        for _ in 0..workers.max(1) {
            let queue = queue.clone();
            let results = results.clone();
            std::thread::spawn(move || crypto_worker(&queue, &results));
        }
        Self { queue }
    }

    fn submit(&self, job: CryptoJob) {
        let queue = &*self.queue;
        if let Ok(mut jobs) = queue.jobs.lock() {
            jobs.push_back(job);
            queue.len.fetch_add(1, Ordering::Release);
            drop(jobs);
            queue.ready.notify_one();
        }
    }
}

fn run_crypto_job(job: CryptoJob) -> CryptoResult {
    match job {
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

pub async fn run_with_proof_decider<S, H, J, P>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring,
    on_journaled: J,
    should_prove: P,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
{
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        should_prove,
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
        |_: &ProofRequest| false,
        Some(store),
        crypto_pool_config,
    )
    .await
}

async fn run_inner<S, H, J, P>(
    mut engine: EngineState<S>,
    mut host: H,
    wiring: ReactorWiring,
    mut on_journaled: J,
    mut should_prove: P,
    store: Option<InterfaceStore>,
    crypto_pool_config: CryptoPoolConfig,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
{
    let ReactorWiring {
        interfaces,
        ifacs,
        mut notify,
        mut inbound_lanes,
        mut commands,
        mut egress,
    } = wiring;
    let mut interfaces = IndexedAttachedInterfaces::from(interfaces);
    for descriptor in interfaces.descriptors() {
        engine.interface_attached(descriptor.id, host.now());
    }
    let mut wake_schedules = engine.wake_schedules(interfaces.view());
    let mut pacers: std::vec::Vec<InterfacePacer> = interfaces
        .descriptors()
        .iter()
        .map(|descriptor| InterfacePacer {
            id: descriptor.id,
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
    macro_rules! journaled_sink {
        () => {
            |journaled: Journaled<'_>| {
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
                        &egress,
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
                        &egress,
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
        CryptoPoolConfig::Pooled { workers } => Some(CryptoPool::spawn(
            workers.resolve().get(),
            crypto_tx.clone(),
        )),
    };
    let _crypto_tx = crypto_tx;
    let mut dirty: std::vec::Vec<InterfaceId> = std::vec::Vec::new();
    let timer_base = Instant::now();
    let wall_base = host.now();
    let due_timer = tokio::time::sleep_until(timer_base);
    tokio::pin!(due_timer);
    let mut armed: Option<(InstantMillis, WakeReason)> = None;
    const CRYPTO_HOT_WINDOW: Duration = Duration::from_millis(5);
    let mut last_pool_activity: Option<std::time::Instant> = None;
    loop {
        let pacer_wake = soonest_pacer_release(&pacers);
        match wake_schedules.soonest(host.now()) {
            NextWake::Idle => armed = None,
            NextWake::Due(reason) => {
                due_timer.as_mut().reset(Instant::now());
                armed = Some((InstantMillis(0), reason));
            }
            NextWake::At { at, reason } => {
                if armed.map(|(deadline, _)| deadline) != Some(at) {
                    due_timer.as_mut().reset(
                        timer_base + Duration::from_millis(at.0.saturating_sub(wall_base.0)),
                    );
                }
                armed = Some((at, reason));
            }
        }
        tokio::select! {
            arrived = notify.recv() => {
                let Some(source) = arrived else { return };
                dirty.clear();
                dirty.push(source);
                while let Ok(more) = notify.try_recv() {
                    if !dirty.contains(&more) {
                        dirty.push(more);
                    }
                }
                let now = host.now();
                for &source in &dirty {
                    let Some((_, lane)) = inbound_lanes.iter_mut().find(|(id, _)| *id == source)
                    else {
                        continue;
                    };
                    for _ in 0..MAX_INBOUND_BATCH {
                        let Some(slot) = lane.try_peek() else { break };
                        let bytes = match ifac_for(&ifacs, source) {
                            Some(entry) => {
                                let Some(clean_len) =
                                    entry.context.unmask_inbound(slot.frame(), &mut unmask_scratch)
                                else {
                                    lane.release();
                                    continue;
                                };
                                &mut unmask_scratch[..clean_len]
                            }
                            None => slot.frame_mut(),
                        };
                        if let Some(pool) = &crypto_pool {
                            if let Ok((header, payload)) = WirePacketHeader::parse(bytes) {
                                if header.packet_type == PacketType::Proof {
                                    if let Some(deferred) = engine.settle_receipt_proof_deferred(
                                        payload,
                                        &DestinationHash::from_address(header.address),
                                        now,
                                    ) {
                                        let settle = match deferred.ingest {
                                            ProofIngest::SendSinglePacketDelivered { id, delivered } => {
                                                Some((id, Settlement::SendSinglePacket(Ok(delivered))))
                                            }
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
                                            last_pool_activity = Some(std::time::Instant::now());
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
                                        sink: &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                    },
                                    &mut deferred_sign,
                                    Some(&mut deferred),
                                );
                                let deferred_job = match deferred {
                                    DeferredCrypto::Empty => None,
                                    DeferredCrypto::Decrypt(owed) => Some(CryptoJob::Decrypt(owed)),
                                    DeferredCrypto::RatchetDecrypt(owed) => {
                                        Some(CryptoJob::DecryptWithRatchets(Box::new(owed)))
                                    }
                                    DeferredCrypto::LinkProofVerify(owed) => {
                                        Some(CryptoJob::VerifyLinkProof(owed))
                                    }
                                    DeferredCrypto::LinkProofSign(owed) => {
                                        Some(CryptoJob::SignLinkProof(owed))
                                    }
                                    DeferredCrypto::AnnounceVerify(owed) => {
                                        Some(CryptoJob::VerifyAnnounce(owed))
                                    }
                                };
                                let jobs = [deferred_sign.map(CryptoJob::Sign), deferred_job];
                                for job in jobs.into_iter().flatten() {
                                    pool.submit(job);
                                    last_pool_activity = Some(std::time::Instant::now());
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
                                    sink: &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                },
                            ),
                        };
                        lane.release();
                        merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine, interfaces.view());
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
                                last_pool_activity = Some(std::time::Instant::now());
                                defer_send_single_packet!(pool, id, send, now)
                            }
                            (_, command) => engine.ingest_command_into(
                                IssuedCommand { id, command },
                                interfaces.view(),
                                now,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            ),
                        }
                    }
                    HostCommand::AwaitedEngine { issued, completion } => {
                        let id = issued.id;
                        pending_completions.borrow_mut().insert(id, completion);
                        match (crypto_pool.as_ref(), issued.command) {
                            (Some(pool), EngineCommand::SendSinglePacket(send)) => {
                                last_pool_activity = Some(std::time::Instant::now());
                                defer_send_single_packet!(pool, id, send, now)
                            }
                            (_, command) => engine.ingest_command_into(
                                IssuedCommand { id, command },
                                interfaces.view(),
                                now,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
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
                        &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
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
                            &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
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
                                &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
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
                                &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
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
                                    &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
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
                                        &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
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
                        &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                    ),
                    HostCommand::AddInterface(add) => {
                        let AddInterfaceCommand {
                            descriptor,
                            inbound,
                            egress: egress_producer,
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
                                pacer: AnnouncePacer::new(
                                    descriptor.announce_bandwidth_cap,
                                    descriptor.bitrate,
                                ),
                            });
                            engine.interface_attached(id, now);
                            interfaces.push(descriptor);
                            inbound_lanes.push((id, inbound));
                            egress.add_lane(id, egress_producer);
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
                                &egress,
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
                if let Some((_, reason)) = armed.take() {
                    let now = host.now();
                    let wake_schedules_delta = fire_due_reason(
                        &mut engine,
                        reason,
                        now,
                        interfaces.view(),
                        &mut |bytes| host.fill_entropy(bytes),
                        &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                    );
                    merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine, interfaces.view());
                }
            }
            _ = wait_for_pacer(&host, pacer_wake) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &egress, &ifacs);
            }
            verdict = crypto_rx.recv(), if crypto_pool.is_some() => {
                let mut next = verdict;
                let now = host.now();
                let mut seal_buf = [0u8; crate::wire::BROADCAST_MTU];
                while let Some(result) = next {
                    match result {
                        CryptoResult::Verified { id, settlement, valid } => {
                            if valid && engine.settle_resolved(id).is_some() {
                                route_reaction(
                                    EngineReaction::Journaled(Journaled::CommandSettled {
                                        id,
                                        settlement,
                                    }),
                                    &egress,
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
                                &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
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
                                    &egress,
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
                                &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            if let Some(deferred) = deferred_sign {
                                if let Some(pool) = crypto_pool.as_ref() {
                                    pool.submit(CryptoJob::Sign(deferred));
                                    last_pool_activity = Some(std::time::Instant::now());
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
                                    &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                );
                                if let Some(deferred) = deferred_sign {
                                    if let Some(pool) = crypto_pool.as_ref() {
                                        pool.submit(CryptoJob::Sign(deferred));
                                        last_pool_activity = Some(std::time::Instant::now());
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
                                    &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
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
                                &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces.view());
                        }
                        CryptoResult::AnnounceVerified { owed, valid } => {
                            if valid {
                                let delta = engine.resume_announce(
                                    owed,
                                    interfaces.view(),
                                    &mut |entropy| host.fill_entropy(entropy),
                                    &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                );
                                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces.view());
                            }
                        }
                    }
                    next = crypto_rx.try_recv().ok();
                }
            }
            _ = tokio::task::yield_now(), if last_pool_activity.is_some_and(|t| t.elapsed() < CRYPTO_HOT_WINDOW) => {}
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

struct InterfacePacer {
    id: InterfaceId,
    pacer: AnnouncePacer<HeapPacerQueue>,
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
    egress: &Egress,
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
        EngineReaction::Directive(Directive::SendAnnounce {
            target,
            bytes,
            hops,
        }) => {
            offer_to_pacer(pacers, target, bytes, hops, now, egress, ifacs);
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
        EngineReaction::Directive(Directive::SendAnnounceToFleet {
            supervisor,
            fan,
            bytes,
            hops,
        }) => {
            for target in egress.broadcast_targets(supervisor, fan) {
                offer_to_pacer(pacers, target, bytes, hops, now, egress, ifacs);
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
    egress: &Egress,
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
    egress: &Egress,
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
        None => egress.enqueue(target, bytes),
    }
}

/// A paced announce is broadcast-sized by construction, so its mask scratch fits on the
/// stack — the wire-sized [`WireScratch`] is reserved for the frame paths.
const PACED_MASK_LEN: usize = crate::wire::BROADCAST_MTU + crate::interfaces::ifac::IFAC_MAX_SIZE;

fn offer_to_pacer(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    bytes: &[u8],
    hops: u8,
    now: InstantMillis,
    egress: &Egress,
    ifacs: &[InterfaceIfac],
) {
    match pacers.iter_mut().find(|entry| entry.id == target) {
        Some(entry) => {
            entry.pacer.offer(bytes, hops, now, |frame| {
                let mut masked = [0u8; PACED_MASK_LEN];
                enqueue_for_wire(egress, ifacs, target, frame, &mut masked);
            });
        }
        None => {
            let mut masked = [0u8; PACED_MASK_LEN];
            enqueue_for_wire(egress, ifacs, target, bytes, &mut masked);
        }
    }
}

fn flush_due_pacers(
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    egress: &Egress,
    ifacs: &[InterfaceIfac],
) {
    for entry in pacers.iter_mut() {
        let target = entry.id;
        entry.pacer.release_due(now, |frame| {
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
        InterfaceCapabilities, InterfaceMode, TransportCapability,
    };
    use crate::reactor::interface_seam::Interface;
    use crate::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
    use crate::wire::{PacketType, WirePacketHeader};

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
            pacer: AnnouncePacer::<HeapPacerQueue>::new(
                AnnounceBandwidthCap::RNS_DEFAULT,
                BitrateBps::guess(5_000),
            ),
        }];
        let (tx, mut rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let egress = Egress::new(std::vec![(id, tx)]);

        offer_to_pacer(
            &mut pacers,
            id,
            &[1; 10],
            1,
            InstantMillis(1_000),
            &egress,
            &[],
        );
        assert_eq!(rx.try_peek().unwrap().frame(), [1u8; 10].as_slice());
        rx.release();

        offer_to_pacer(
            &mut pacers,
            id,
            &[2; 10],
            1,
            InstantMillis(1_200),
            &egress,
            &[],
        );
        assert!(rx.try_peek().is_none(), "the second is held, not sent");
        assert_eq!(soonest_pacer_release(&pacers), Some(InstantMillis(1_800)));

        flush_due_pacers(&mut pacers, InstantMillis(1_799), &egress, &[]);
        assert!(
            rx.try_peek().is_none(),
            "nothing releases before the window"
        );

        flush_due_pacers(&mut pacers, InstantMillis(1_800), &egress, &[]);
        assert_eq!(rx.try_peek().unwrap().frame(), [2u8; 10].as_slice());
        rx.release();
        assert_eq!(soonest_pacer_release(&pacers), None);
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
            | Journaled::CommandSettled { .. }
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
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
        use crate::routing::upstream_app_destinations::ProofStrategy;
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
            | Journaled::CommandSettled { .. }
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
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

        let network = || IfacContext::derive(Some("testnet"), Some("s3cret"), 8).unwrap();
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

        let stranger = IfacContext::derive(Some("testnet"), Some("wrong"), 8).unwrap();
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
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
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
        use crate::routing::upstream_app_destinations::ProofStrategy;

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
            | Journaled::Delivered(_)
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
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
        use crate::routing::upstream_app_destinations::ProofStrategy;

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
