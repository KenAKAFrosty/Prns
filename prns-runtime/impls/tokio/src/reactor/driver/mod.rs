use core::cell::{Cell, RefCell};
use prns_core::interfaces::IndexedAttachedInterfaces;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::crypto::{
    ed25519_sign, x25519_diffie_hellman, x25519_keys_for_seal, Ed25519Signature, Ed25519Verifier,
    X25519PublicKey, X25519SharedSecret,
};
use crate::engine::{
    AnnounceVerifyOwed, ClassifiedInboundPacket, CommandId, DecryptOwed, DeferredCrypto,
    DeferredProofSign, Departure, Directive, EncryptOwed, EngineCommand, EngineReaction,
    EngineState, IngestIo, InstantMillis, IssuedCommand, Journaled, NextWake, ProofIngest,
    ProofRequest, RatchetDecryptOwed, Respond, RespondData, SendRequest, SendRequestData,
    SendRequestFailure, SendSinglePacketEntropy, SendSinglePacketFailure, SendSinglePacketPrepared,
    SendSinglePacketWriteError, Settlement, WakeReason, WakeSchedules,
};
use crate::identity::{
    decrypt_token_in_place_with_ratchets, IdentitySigningPublicKey, OpenedBy, OpenedToken,
    Zeroizing,
};
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{
    ConnectionView, InboundPacket, InterfaceDescriptor, InterfaceId, PacketPhyStats,
};
use crate::reactor::interface_seam::{frame_cap_for, BROADCAST_WIRE_FRAME_LEN};
use crate::reactor::kernel::{fire_due_reason, merge_wake_schedules_delta};
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
use crate::routing::proof::EXPLICIT_PROOF_WIRE_LEN;
use crate::routing::request_handlers::RequestPathHash;
use crate::runtime::node_introspection::{AnnounceRateHistory, NodeIntrospectionRequest};
use crate::runtime::{
    apply_destination_identity_retention_command, apply_identity_blackhole_command,
    ClearAnnounceQueuesOutcome, DestinationIdentityRetentionHostCommand, DropRouteOutcome,
    DropRoutesViaOutcome, IdentityBlackholeHostCommand, InterfaceStore,
};
#[cfg(feature = "runtime-metrics")]
use crate::runtime::{CryptoMetricsSnapshot, ReliabilityMetricsSnapshot, RuntimeMetricsSnapshot};
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::units::RttMillis;
use crate::wire::{DestinationHash, TransportId};
use heapless::Vec as HeaplessVec;

mod egress;
mod host;
mod interface_seam;
mod interface_status;

pub use super::grant_lane::{
    tokio_grant_lane, HeapFrameSlot, TokioGrantConsumer, TokioGrantProducer,
};
pub use egress::Egress;
pub use host::TokioHost;
pub use interface_seam::TokioInterfaceSeam;
pub use interface_status::TokioInterfaceStatus;

#[cfg(all(test, feature = "runtime-metrics"))]
use egress::enqueue_announce_for_wire;
use egress::{
    clear_announce_queues, flush_due_pacers, ifac_for, route_reaction, soonest_pacer_release,
    InterfacePacer, WireScratch,
};
#[cfg(test)]
use egress::{offer_to_pacer, PacedAnnounce, TokioAnnouncePacer};
use host::bounded_timer_deadline;

fn retain_packet_phy(
    store: Option<&InterfaceStore>,
    packet_hash: PacketHash,
    packet_phy: PacketPhyStats,
) {
    if packet_phy.is_empty() {
        return;
    }
    let Some(store) = store else {
        return;
    };
    store.remember_packet_phy(packet_hash, packet_phy);
}

/// What a std host feeds the reactor: the queueable engine commands, plus the owned-bytes verbs that deliberately never ride `EngineCommand`. A resource payload can reach a mebibyte, so the std host owns or shares the heap allocation and the engine borrows it for exactly one call; an embedded host calls the borrow-taking entry points directly instead.
#[allow(clippy::large_enum_variant)]
pub enum HostCommand {
    Engine(IssuedCommand),
    /// An engine command whose settlement a caller is awaiting: the `completion` rides the command to the single reactor task, which stashes it keyed by `issued.id` and fires it when the matching `CommandSettled` is journaled, so the await-correlation registry has one owner and needs no lock.
    AwaitedEngine {
        issued: IssuedCommand,
        completion: oneshot::Sender<Settlement>,
    },
    SendResource(SendResourceHostCommand),
    SendResourceSegment(SendResourceSegmentHostCommand),
    RespondAny(RespondAnyHostCommand),
    RequestAny(RequestAnyHostCommand),
    ProvideDecompressed(ProvideDecompressedHostCommand),
    /// Register a runtime-added interface's descriptor and lanes so the reactor routes to it; its run loop is driven separately on the runtime's interface driver, only the `Send` lane halves cross.
    AddInterface(AddInterfaceCommand),
    /// Deregister the interface with this id: drop its descriptor and lanes so the reactor stops routing to it (its run loop is stopped separately on the interface driver).
    RemoveInterface {
        id: InterfaceId,
        departure: Departure,
    },
    DropRoute {
        destination: DestinationHash,
        reply: oneshot::Sender<DropRouteOutcome>,
    },
    DropRoutesVia {
        transport: TransportId,
        reply: oneshot::Sender<DropRoutesViaOutcome>,
    },
    ClearAnnounceQueues {
        reply: oneshot::Sender<ClearAnnounceQueuesOutcome>,
    },
    IdentityBlackhole(IdentityBlackholeHostCommand),
    DestinationIdentityRetention(DestinationIdentityRetentionHostCommand),
    NodeIntrospection(NodeIntrospectionRequest),
    SynthesizeTunnel {
        interface: InterfaceId,
    },
    /// Register a byte-stream reader's inbound sink: the run loop routes matching channel messages to it, suppressed from the app event stream. `ready` fires once the sink is in the routing table, so the opener can hold back the reader until no arriving chunk can slip past; awaited, not raced.
    RegisterStreamReader {
        link_id: LinkId,
        stream_id: StreamId,
        sink: UnboundedSender<StreamInbound>,
        ready: oneshot::Sender<()>,
    },
    /// Register a sink for the next inbound resource on this link: the run loop routes the resource's chunks to it and signals completion, suppressed from the app event stream. `ready` fires once registered, so a segment arriving the instant after cannot slip past to the app.
    RegisterResourceSink {
        link_id: LinkId,
        sink: UnboundedSender<ResourceInbound>,
        ready: oneshot::Sender<()>,
    },
    /// Set the default resource strategy for `destination`. `ready` carries back whether the destination was held, so a caller learns a misaddressed strategy rather than having it silently dropped.
    SetResourceStrategy {
        destination: DestinationHash,
        strategy: ResourceStrategy,
        ready: oneshot::Sender<bool>,
    },
    /// Serialize every persisted region on the reactor — the one place a consistent view exists — and hand the sealed images back; the caller owns the store IO, so flush cadence stays host policy.
    SnapshotPersistedState {
        reply: oneshot::Sender<PersistedStateSnapshot>,
    },
    /// Seal every tracked destination's self-ratchet record. Secrets ride these blobs, so they go to the caller's identity vault, never a `PersistedStore`.
    SnapshotSelfRatchets {
        reply: oneshot::Sender<SelfRatchetsSnapshot>,
    },
    SnapshotSelfRatchet {
        destination: DestinationHash,
        reply: oneshot::Sender<Option<SelfRatchetSnapshot>>,
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

pub struct SelfRatchetSnapshot {
    pub destination: DestinationHash,
    pub sealed: Zeroizing<std::vec::Vec<u8>>,
}

fn seal_self_ratchet(
    last_rotated: crate::crypto::ratchets::LastRotated,
    secrets: &[crate::crypto::X25519SecretKey],
) -> Option<Zeroizing<std::vec::Vec<u8>>> {
    let mut sealed = Zeroizing::new(std::vec![
        0u8;
        crate::persistence::self_ratchets_snapshot_len(secrets.len())
    ]);
    let written =
        crate::persistence::write_self_ratchets_snapshot(last_rotated, secrets, &mut sealed)
            .ok()?;
    sealed.truncate(written);
    Some(sealed)
}

/// The sealed region images of one snapshot pass and the engine instant they were taken at — the timebase high-water a flush of these images should record.
pub struct PersistedStateSnapshot {
    pub routing_table: std::vec::Vec<u8>,
    pub tunnels: std::vec::Vec<u8>,
    pub destination_identities: std::vec::Vec<u8>,
    pub taken_at: InstantMillis,
}

/// One inbound stream chunk handed from the run loop's demux to a `ByteStreamReader`'s sink: the payload bytes (owned, copied out of the borrowed reaction) with the eof and compressed flags.
pub struct StreamInbound {
    pub payload: std::vec::Vec<u8>,
    pub eof: bool,
    pub compressed: bool,
}

/// One inbound resource event handed from the run loop to a `receive_resource` future's sink: a chunk of bytes, the completion marker carrying the assembled identity and total size, or a failure. Owned, copied out of the borrowed reaction.
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

/// The payload of [`HostCommand::AddInterface`]: a runtime-added interface's descriptor and the reactor's halves of its grant lanes. All `Send`, so the reactor stays `Send`; the interface's `!Send` run future is driven on the runtime's interface driver, not here.
pub struct AddInterfaceCommand {
    pub descriptor: InterfaceDescriptor,
    pub logical_interface: InterfaceId,
    pub inbound: TokioGrantConsumer,
    pub egress: TokioGrantProducer,
    pub connection: Option<ConnectionView>,
    pub ifac: Option<crate::interfaces::ifac::IfacContext>,
}

/// Fire an awaited command's settlement to the caller parked on it, or pass the event through. The reactor owns `pending` (a single-task `RefCell` map, no `Arc`/`Mutex`): a `CommandSettled` whose id a caller awaits is handed to that caller's [`oneshot`] and dropped from the event stream; every other journaled event — including a settlement nobody awaited — is returned for the app.
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

/// A `request()` parked on its answer: the bytes land first (stashed here), then the round trip arrives with the settlement.
struct RequestPending {
    completion: oneshot::Sender<Result<(std::vec::Vec<u8>, RttMillis), SendRequestFailure>>,
    data: Option<std::vec::Vec<u8>>,
}

/// The outbound-request demux, mirror of [`settle_or_forward`]: a `ResponseReceived` with a parked `request()` stashes its bytes (a split response's segments append, in the arrival order the engine's gate enforces); the matching `SendRequest` settlement then fires `(data, rtt)` or the typed failure to the awaiter and drops the events from the app stream.
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

/// Resolve a parked `request()` with a typed failure — for the request-forming paths the size yardstick already rules out, so the awaiter never hangs even on the unreachable branch.
fn fail_request(
    pending: &RefCell<HashMap<CommandId, RequestPending>>,
    id: CommandId,
) -> WakeSchedules {
    if let Some(entry) = pending.borrow_mut().remove(&id) {
        let _ = entry.completion.send(Err(SendRequestFailure::WriteFailed));
    }
    WakeSchedules::UNCHANGED
}

/// The byte-stream demux: a `ChannelMessageReceived` of the reserved stream type whose `(link, id)` has a registered reader is parsed, pushed to that reader's sink, and dropped from the event stream; a reader whose receiver is gone is pruned. Everything else is returned for the app.
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

/// The resource mirror of [`route_stream_or_forward`]: an inbound resource journal on a link with a registered `receive_resource` sink is routed to it and suppressed from the app event stream. The sink is one-shot: it retires on the resource's completion or failure, leaving the link free to register the next.
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

/// One segment of a resource send, awaited: the `completion` rides the command to the reactor, which stashes it keyed by `id` and fires it when the segment's proof settles — so a host `send_resource` loop drains its source one segment at a time, sending the next only once the last is proven. The engine threads the chain's original hash across segments; the host carries only the bytes.
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

/// Answer a request with `data` of any length: the engine picks the rung, a single RESPONSE packet when it fits the link MDU, an outgoing resource (named back to `request_id`) when it doesn't. Host-held payload, since a large answer never rides an enum; the request router's verb.
pub struct RespondAnyHostCommand {
    pub id: CommandId,
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub data: HostResourcePayload,
    pub compressed_candidate: Option<HostResourcePayload>,
}

/// Make a request of `path_hash` with `data` of any length: the reactor picks the rung and fires `completion` with the response bytes and the round trip once the answer settles. The payload is host-held like every owned-bytes verb.
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

/// How the host runtime runs the engine's asymmetric crypto. `Pooled` offloads verify/seal/sign/decrypt to worker threads and keeps the reactor hot; `Inline` runs them on the reactor thread (the embedded shape, and the mobile default).
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
    /// `Pooled`/`Auto` on a host that benefits; `Inline` on mobile targets, where the reactor stays single-threaded to protect battery.
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

    pub(crate) fn resolved_worker_count(self) -> Option<NonZeroUsize> {
        match self.with_env_override() {
            Self::Inline => None,
            Self::Pooled { workers } => Some(workers.resolve()),
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

/// Everything the reactor is wired to for one run: the interface topology snapshot, per-interface IFAC state, the wake and command channels, the inbound grant lanes, and the egress fan-out.
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

/// A deferred staged-continuation seal: everything [`seal_staged_resource`] needs, copied off the engine so the row can park as `StagedSealing` while a worker runs the crypto. The salts are pre-drawn because workers carry no entropy source; [`SALT_REROLL_CAP`] of them is exactly the attempt budget.
struct StagedSealJob {
    link_id: LinkId,
    key: LinkKey,
    sdu: usize,
    nonce_prefixed_len: usize,
    plaintext: Vec<u8>,
    seal_iv: [u8; 16],
    salts: [[u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP],
}

/// One deferred chew of an incoming transfer's newly-contiguous span: the streamed-open state rides with a copy of the ciphertext, decrypts it in place on the worker, and both come home in the verdict. No key travels — the state already carries every midstate the chew needs.
struct OpenSpanJob {
    link_id: LinkId,
    hash: ResourceHash,
    span_start: usize,
    state: StreamedOpen,
    bytes: Vec<u8>,
}

/// One unit of asymmetric crypto handed off the engine thread (one saturated pool, not one per op): `Verify` is an inbound proof's Ed25519 check, `SealScalars` the two X25519 scalar mults an outbound single's seal needs, `Sign` the Ed25519 signature an inbound delivery's proof owes. The seal job carries the whole obligation so the reactor keeps no per-in-flight side table; it is transient and its large variant is the common one, so it is not boxed.
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

/// What a worker hands back to the reactor thread, where the engine-state mutation it gates runs: `Verified` settles the proof's receipt, `Sealed` finishes the single's frame + receipt track from the returned scalars.
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
    /// In-flight packet-path jobs whose verdicts keep the reactor's yield arm active. Staged seals are excluded because packet processing does not wait for them.
    packet_verdicts_owed: Cell<usize>,
    /// Stamped at every packet-verdict submit and settle; the yield arm stays hot for [`Self::PACKET_VERDICT_LINGER`] past the newest stamp.
    last_packet_verdict_event: Cell<Option<std::time::Instant>>,
}

impl CryptoPool {
    /// Bridges transient zero-owed gaps in a saturated verdict stream without retaining the former long hot window.
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
        .map(|descriptor| InterfacePacer::from_descriptor(descriptor, descriptor.id))
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
    let mut announce_rate_history = AnnounceRateHistory::default();
    #[cfg(feature = "runtime-metrics")]
    let mut reliability = ReliabilityMetricsSnapshot::default();
    macro_rules! journaled_sink {
        () => {
            |journaled: Journaled<'_>| {
                if let Journaled::AnnounceHeard {
                    observation,
                    rate_accounting,
                } = &journaled
                {
                    announce_rate_history.record(
                        observation.destination,
                        observation.arrived_at,
                        *rate_accounting,
                    );
                }
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
    let crypto_pool = crypto_pool_config
        .resolved_worker_count()
        .and_then(|workers| CryptoPool::spawn(workers.get(), crypto_tx.clone()));
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
        // Retaining lanes with queued frames prevents a batch tail from becoming stranded after its notification is consumed.
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
                        let packet = ClassifiedInboundPacket::classify(InboundPacket {
                            arrived_at: now,
                            source_interface: source,
                            bytes,
                        });
                        if let Some(packet_hash) = packet.packet_hash() {
                            retain_packet_phy(store.as_ref(), packet_hash, packet_phy);
                        }
                        if let Some(pool) = &crypto_pool {
                            if let Some((address, payload)) = packet.proof() {
                                if let Some(deferred) = engine.settle_receipt_proof_deferred(
                                    payload,
                                    &DestinationHash::from_address(address),
                                    now,
                                ) {
                                    let settle = match deferred.ingest {
                                        ProofIngest::SendSinglePacketDelivered {
                                            id,
                                            delivered,
                                        } => {
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
                                    }
                                    lane.release();
                                    continue;
                                }
                            }
                        }
                        let wake_schedules_delta = match &crypto_pool {
                            Some(pool) => {
                                let mut deferred_sign = None;
                                let mut deferred = DeferredCrypto::default();
                                let delta = engine.ingest_classified_into_deferring(
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
                            None => engine.ingest_classified_into(
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
                            pacers.push(InterfacePacer::from_descriptor(
                                &descriptor,
                                logical_interface,
                            ));
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
                    HostCommand::DropRoute { destination, reply } => {
                        let outcome = match engine.drop_route(&destination) {
                            Some(removed) => {
                                (journaled_sink!())(Journaled::RouteRemoved {
                                    destination: removed.destination,
                                    cause: removed.cause,
                                });
                                wake_schedules = engine.wake_schedules(interfaces.view());
                                DropRouteOutcome::Dropped
                            }
                            None => DropRouteOutcome::NotFound,
                        };
                        let _ = reply.send(outcome);
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::DropRoutesVia { transport, reply } => {
                        let dropped = engine.drop_routes_via(transport, &mut |removed| {
                            (journaled_sink!())(Journaled::RouteRemoved {
                                destination: removed.destination,
                                cause: removed.cause,
                            });
                        });
                        if dropped != 0 {
                            wake_schedules = engine.wake_schedules(interfaces.view());
                        }
                        let _ = reply.send(DropRoutesViaOutcome {
                            dropped_routes: u32::try_from(dropped).unwrap_or(u32::MAX),
                        });
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::ClearAnnounceQueues { reply } => {
                        let dropped = clear_announce_queues(&mut pacers);
                        let _ = reply.send(ClearAnnounceQueuesOutcome {
                            dropped_announces: u32::try_from(dropped).unwrap_or(u32::MAX),
                        });
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::IdentityBlackhole(command) => apply_identity_blackhole_command(
                        &mut engine,
                        command,
                        &mut |removed| {
                            (journaled_sink!())(Journaled::RouteRemoved {
                                destination: removed.destination,
                                cause: removed.cause,
                            });
                        },
                    ),
                    HostCommand::DestinationIdentityRetention(command) => {
                        apply_destination_identity_retention_command(&mut engine, command, now)
                    }
                    HostCommand::NodeIntrospection(request) => {
                        match request {
                            NodeIntrospectionRequest::LinkCount { reply } => {
                                let _ = reply.send(engine.link_count());
                            }
                            NodeIntrospectionRequest::AnnounceRates { reply } => {
                                let mut snapshots = std::vec::Vec::new();
                                engine.visit_announce_rate_states(|state| {
                                    snapshots.push(announce_rate_history.snapshot(state));
                                });
                                let _ = reply.send(snapshots);
                            }
                            NodeIntrospectionRequest::Routes { reply } => {
                                let mut snapshots = std::vec::Vec::new();
                                engine.visit_route_snapshots(interfaces.view(), |snapshot| {
                                    snapshots.push(snapshot);
                                });
                                let _ = reply.send(snapshots);
                            }
                            NodeIntrospectionRequest::Route { destination, reply } => {
                                let _ = reply.send(
                                    engine.route_snapshot(destination, interfaces.view()),
                                );
                            }
                        }
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
                        let mut destination_identities = std::vec![
                            0u8;
                            crate::persistence::destination_identities_snapshot_len(
                                engine.destination_identities(),
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
                        let written_destination_identities =
                            crate::persistence::write_destination_identities_snapshot(
                                engine.destination_identities(),
                                &mut destination_identities,
                            );
                        if let (Ok(routes_len), Ok(tunnels_len), Ok(destination_identities_len)) = (
                            written_routes,
                            written_tunnels,
                            written_destination_identities,
                        )
                        {
                            routing_table.truncate(routes_len);
                            tunnels.truncate(tunnels_len);
                            destination_identities.truncate(destination_identities_len);
                            let _ = reply.send(PersistedStateSnapshot {
                                routing_table,
                                tunnels,
                                destination_identities,
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
                            if let Some(sealed) = seal_self_ratchet(last_rotated, secrets) {
                                blobs.push((destination, sealed));
                            }
                        }
                        let _ = reply.send(SelfRatchetsSnapshot { blobs });
                        WakeSchedules::UNCHANGED
                    }
                    HostCommand::SnapshotSelfRatchet { destination, reply } => {
                        let snapshot = engine
                            .persisted_self_ratchet_row(&destination)
                            .and_then(|(last_rotated, secrets)| {
                                seal_self_ratchet(last_rotated, secrets)
                            })
                            .map(|sealed| SelfRatchetSnapshot {
                                destination,
                                sealed,
                            });
                        let _ = reply.send(snapshot);
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
                            let mut proof = [0u8; EXPLICIT_PROOF_WIRE_LEN];
                            if let Ok(written) =
                                engine.write_signed_proof(&packet_hash, &signature, &mut proof)
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

#[cfg(test)]
mod tests;
