use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::crypto::ratchets::SeedSelfRatchetsOutcome;
use crate::engine::{
    AnnounceNow, AnnounceNowFailure, CloseLink, CommandId, Departure,
    DestinationIdentitySeedOutcome, EngineCommand, EstablishLink, EstablishLinkFailure,
    IssuedCommand, PacketReceiptDelivered, SendRequestFailure, SendResourceFailure,
    SendSinglePacket, SendSinglePacketFailure, SendSinglePacketPayload, SetTransportIdentityError,
    Settlement,
};
use crate::engine::{InstantMillis, Journaled};
use crate::identity::held::HoldIdentityError;
use crate::identity::vault::{IdentityLabel, IdentityVault};
use crate::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::ifac::IfacContext;
use crate::interfaces::{
    ConnectionView, InterfaceId, InterfaceKind, InterfaceOriginKind, InterfaceSnapshot, Membership,
    PacketPhyStats, ReportsStatus, StatusView,
};
use crate::node_introspection::{
    AnnounceRateSnapshot, InterfaceIfacSnapshot, InterfaceInventoryEntry, NodeIntrospection,
    NodeIntrospectionRequest, RouteSnapshot,
};
use crate::persistence::{
    read_destination_identities_snapshot, read_routing_table_snapshot, read_self_ratchets_snapshot,
    read_timebase_snapshot, read_tunnels_snapshot, snapshot_fingerprint, write_timebase_snapshot,
    PersistedStore, SnapshotFingerprint, SnapshotRegion, TIMEBASE_SNAPSHOT_LEN,
};
use crate::reactor::compression;
use crate::reactor::driver::{
    self as reactor_driver, tokio_grant_lane, AddInterfaceCommand, CryptoPoolConfig, Egress,
    HostCommand, HostResourceMetadata, HostResourcePayload, PersistedStateSnapshot,
    ProvideDecompressedHostCommand, RequestAnyHostCommand, ResourceInbound, RespondAnyHostCommand,
    SelfRatchetSnapshot, SelfRatchetsSnapshot, SendResourceSegmentHostCommand, TokioHost,
    TokioInterfaceSeam,
};
use crate::reactor::interface_seam::{frame_cap_for, Interface};
use crate::reactor::Host;
use crate::routing::announce::AnnounceObservation;
use crate::routing::blackhole::BlackholeTable;
use crate::routing::dedup::PacketHash;
use crate::routing::links::data::LINK_MDU;
use crate::routing::links::request::{RequestId, RESPONSE_WIRE_OVERHEAD};
use crate::routing::links::resources::{
    ResourceHash, ResourceStrategy, MAX_EFFICIENT_SIZE, METADATA_PREFIX_LEN,
};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestHandlerError, RequestPathHash};
use crate::routing::tunnel::SeedTunnelOutcome;
use crate::routing::{BlackholeIdentityOutcome, BlackholeInsertFailure, BlackholedIdentity};
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, TransportId};

use super::byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};
use super::identity_blackhole::{settle_control, settle_source};
use super::request_router::{RespondToken, RouteSet};
use super::request_runner::{run_router, RunnerRequest, REQUEST_QUEUE_DEPTH};
use super::settle_destination_identity_retention;
#[cfg(feature = "runtime-metrics")]
use super::RuntimeMetricsSnapshot;
use super::{
    ClearAnnounceQueuesOutcome, DestinationIdentityRetentionControl,
    DestinationIdentityRetentionControlError, DestinationIdentityRetentionHostCommand,
    DropRouteOutcome, DropRoutesViaOutcome, IdentityBlackholeControl,
    IdentityBlackholeControlError, IdentityBlackholeHostCommand, IdentityBlackholeSource,
    IdentityBlackholeSourceError, InterfaceStore, Manual, Message, PreConfiguredDestination,
    PrnsEvent, PrnsNodeRecipe, RoutingControl, RoutingControlError, SendError,
};
use prns_runtime::runtime::{
    assemble_node, configure_preconfigured_destination, AssembledNode,
    ConfigurePreconfiguredDestinationError,
};

/// How many frames a host lane holds in flight. RNS resource transfer bursts a whole window of parts at once (`Resource.WINDOW_MAX_FAST` is 75, plus its flexibility), so a lane carrying a transfer must be deeper than that window or it sheds parts and the transfer stalls; the old byte-budget collapsed a fat-MTU lane to a handful of slots, exactly that failure. Growable slots (`HeapFrameSlot`) cost only the frames actually in flight, so the depth is generous.
const HOST_LANE_DEPTH: usize = 256;

fn lane_depth_for(_slot_cap: usize) -> usize {
    HOST_LANE_DEPTH
}

/// The largest response that still fits a single link packet on the base-MDU link RNS 1.3.5 negotiates from. At or below it a response is always a packet, never a resource, so building a bz2 candidate would only waste the codec on an answer that never carries it. Above it the answer may ride a resource (the reactor decides against the live MDU), so it is worth compressing; the floor sits under every live packet ceiling, so no resource-bound response goes uncompressed.
const RESPONSE_PACKET_CEILING: usize = LINK_MDU - RESPONSE_WIRE_OVERHEAD;

/// A cloneable, `Send` handle to a running node: the proactive surface. Every [`CommandId`] is minted from one counter, so a fire-and-forget [`issue`](Self::issue) can never collide with an awaited [`send_single_packet`](Self::send_single_packet) or a runner's respond.
#[derive(Clone)]
pub struct PrnsNodeHandle {
    commands: UnboundedSender<HostCommand>,
    ids: Arc<AtomicU64>,
    notify_tx: UnboundedSender<InterfaceId>,
    iface_build: UnboundedSender<DriverMsg>,
    interfaces: Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    store: InterfaceStore,
}

#[derive(Clone)]
struct RuntimeIfac {
    context: IfacContext,
    network_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceAttachmentMetadata {
    pub name: Option<String>,
    pub origin: InterfaceOriginKind,
}

#[derive(Clone, Copy)]
struct InterfacePlacement {
    membership: Membership,
    origin: InterfaceOriginKind,
}

impl RuntimeIfac {
    fn snapshot(&self) -> InterfaceIfacSnapshot {
        InterfaceIfacSnapshot {
            signature: self.context.ifac_signature(),
            size: self.context.ifac_size(),
            network_name: self.network_name.clone(),
        }
    }
}

/// Why a [`send_resource`](PrnsNodeHandle::send_resource) stream did not complete.
#[derive(Debug)]
pub enum ResourceSendError {
    /// Reading `source` failed before the whole resource was sent.
    Source(std::io::Error),
    UnrepresentableLength,
    /// A segment was refused, timed out, or rejected by the peer.
    Rejected(SendResourceFailure),
    /// The node's reactor has stopped.
    NodeStopped,
}

/// RNS 1.3.5 `Resource.AUTO_COMPRESS_MAX_SIZE`
pub const AUTO_COMPRESS_MAX_LEN: u64 = 64 * 1024 * 1024;

const fn resource_segment_decompression_bound(uncompressed_data_len: u64) -> u64 {
    if uncompressed_data_len < MAX_EFFICIENT_SIZE as u64 {
        uncompressed_data_len
    } else {
        MAX_EFFICIENT_SIZE as u64
    }
}

const INFLATE_QUEUE_PER_WORKER: usize = 4;
const MAX_INFLATE_PARALLELISM: usize = 8;

fn inflate_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .clamp(1, MAX_INFLATE_PARALLELISM)
}

const MAX_BOOT_RECORD_LEN: usize = 64 * 1024 * 1024;

fn try_zeroed_buffer(len: usize) -> Option<Vec<u8>> {
    if len > MAX_BOOT_RECORD_LEN {
        return None;
    }
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(len).ok()?;
    buffer.resize(len, 0);
    Some(buffer)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentCompression {
    Attempt { up_to_byte_len: u64 },
    Never,
}

impl SegmentCompression {
    /// The reference's `auto_compress=True`: attempt up to [`AUTO_COMPRESS_MAX_LEN`].
    pub const AUTO: Self = Self::Attempt {
        up_to_byte_len: AUTO_COMPRESS_MAX_LEN,
    };
}

/// What a completed [`receive_resource`](PrnsNodeHandle::receive_resource) yields: the assembled resource's identity, total size, and any metadata that traveled. The bytes themselves were streamed to the caller's sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceReceipt {
    pub original_hash: ResourceHash,
    pub total_size: u64,
    /// The transfer's packed metadata (msgpack the app unpacks), when one traveled.
    pub metadata: Option<std::vec::Vec<u8>>,
}

/// Why a [`receive_resource`](PrnsNodeHandle::receive_resource) stream did not complete.
#[derive(Debug)]
pub enum ResourceReceiveError {
    /// Writing to `sink` failed before the whole resource arrived.
    Sink(std::io::Error),
    /// The transfer failed at the receiver — a bad segment, a vanished link, or a refused offer.
    Failed,
    /// The node's reactor has stopped.
    NodeStopped,
}

impl PrnsNodeHandle {
    #[cfg(test)]
    pub(crate) fn over(commands: UnboundedSender<HostCommand>) -> Self {
        let (notify_tx, _notify_rx) = mpsc::unbounded_channel();
        let (iface_build, _iface_build_rx) = mpsc::unbounded_channel();
        Self {
            commands,
            ids: Arc::new(AtomicU64::new(0)),
            notify_tx,
            iface_build,
            interfaces: Arc::new(Mutex::new(HashMap::new())),
            store: InterfaceStore::new(),
        }
    }

    fn mint(&self) -> CommandId {
        CommandId(self.ids.fetch_add(1, Ordering::Relaxed))
    }

    #[must_use]
    pub fn interface_store(&self) -> InterfaceStore {
        self.store.clone()
    }

    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        let id = self.mint();
        self.commands
            .send(HostCommand::Engine(IssuedCommand { id, command }))
            .ok()?;
        Some(id)
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.command.send_single_packet",
            level = "debug",
            skip_all,
            fields(bytes = data.len(), destination = ?destination.as_bytes()),
            err(Debug)
        )
    )]
    pub async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        let payload =
            SendSinglePacketPayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        match self
            .settle(EngineCommand::SendSinglePacket(SendSinglePacket {
                destination,
                payload,
            }))
            .await
        {
            Some(Settlement::SendSinglePacket(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    /// Make a request of `path_hash` with `data` of any length and await the response. The runtime picks the rung (a single REQUEST packet within the link MDU, or a resource that rides past it), so a consumer never meets a size limit; the answer carries the measured round trip.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.request",
            level = "debug",
            skip_all,
            fields(bytes = data.len(), link_id = ?link_id.as_bytes(), path_hash = ?path_hash),
            err(Debug)
        )
    )]
    pub async fn request(
        &self,
        link_id: LinkId,
        path_hash: RequestPathHash,
        data: &[u8],
    ) -> Result<(std::vec::Vec<u8>, RttMillis), SendError<SendRequestFailure>> {
        let id = self.mint();
        let (completion, settled) = oneshot::channel();
        self.commands
            .send(HostCommand::RequestAny(RequestAnyHostCommand {
                id,
                link_id,
                path_hash,
                data: data.to_vec().into(),
                completion,
            }))
            .map_err(|_| SendError::NodeStopped)?;
        match settled.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(failure)) => Err(SendError::Failed(failure)),
            Err(_) => Err(SendError::NodeStopped),
        }
    }

    /// Stream a resource of `total_len` bytes to a peer over an active link, draining `source` one segment at a time: the engine holds the transferring segment plus the next one staged (sealed early, advertised at the proof), and the host prepares one more behind those, so at most three segments are ever held, never the whole payload. The length is explicit because every segment advertises the total up front; a payload at or under one segment crosses unsplit.
    pub async fn send_resource(
        &self,
        link_id: LinkId,
        total_len: u64,
        source: impl AsyncRead + Unpin,
    ) -> Result<(), ResourceSendError> {
        self.send_resource_streaming(
            link_id,
            total_len,
            source,
            None,
            SegmentCompression::AUTO,
            None,
        )
        .await
    }

    /// [`send_resource`](Self::send_resource) with the compression posture explicit, the RNS 1.3.5 `auto_compress` parameter: [`SegmentCompression::Never`] ships every segment uncompressed where the default attempts bz2 per segment.
    pub async fn send_resource_with_compression(
        &self,
        link_id: LinkId,
        total_len: u64,
        source: impl AsyncRead + Unpin,
        compression: SegmentCompression,
    ) -> Result<(), ResourceSendError> {
        self.send_resource_streaming(link_id, total_len, source, None, compression, None)
            .await
    }

    /// [`send_resource`](Self::send_resource) with RNS 1.3.5 resource metadata: `packed_metadata` is msgpack the peer's app unpacks, opaque all the way down. The block rides ahead of the data in segment one's stream and inside the advertised total, so segment one carries that much less data and a payload near the segment boundary may split one segment sooner.
    pub async fn send_resource_with_metadata(
        &self,
        link_id: LinkId,
        total_len: u64,
        source: impl AsyncRead + Unpin,
        packed_metadata: &[u8],
    ) -> Result<(), ResourceSendError> {
        self.send_resource_streaming(
            link_id,
            total_len,
            source,
            Some(packed_metadata.into()),
            SegmentCompression::AUTO,
            None,
        )
        .await
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.resource.send",
            level = "debug",
            skip_all,
            fields(bytes = total_len, link_id = ?link_id.as_bytes()),
            err(Debug)
        )
    )]
    async fn send_resource_streaming(
        &self,
        link_id: LinkId,
        total_len: u64,
        mut source: impl AsyncRead + Unpin,
        packed_metadata: Option<Arc<[u8]>>,
        compression: SegmentCompression,
        answers_request: Option<RequestId>,
    ) -> Result<(), ResourceSendError> {
        let segment_size = MAX_EFFICIENT_SIZE as u64;
        let block_len = match packed_metadata.as_ref() {
            None => 0,
            Some(packed) => u64::try_from(packed.len())
                .ok()
                .and_then(|len| len.checked_add(METADATA_PREFIX_LEN as u64))
                .ok_or(ResourceSendError::UnrepresentableLength)?,
        };
        let stream_total_len = total_len
            .checked_add(block_len)
            .ok_or(ResourceSendError::UnrepresentableLength)?;
        let total_segments = stream_total_len.div_ceil(segment_size).max(1);
        let mut remaining = total_len;
        let mut in_flight: VecDeque<oneshot::Receiver<Settlement>> =
            VecDeque::with_capacity(ENGINE_SEGMENT_LANES);
        for segment_index in 1..=total_segments {
            let capacity = if segment_index == 1 {
                segment_size.saturating_sub(block_len)
            } else {
                segment_size
            };
            let this_segment = remaining.min(capacity);
            remaining -= this_segment;
            let mut chunk = std::vec![0u8; this_segment as usize];
            source
                .read_exact(&mut chunk)
                .await
                .map_err(ResourceSendError::Source)?;
            let first_segment_block = (segment_index == 1)
                .then(|| packed_metadata.clone())
                .flatten();
            let segment_payload_len = if segment_index == 1 {
                block_len + this_segment
            } else {
                this_segment
            };
            let attempt = match compression {
                SegmentCompression::Attempt {
                    up_to_byte_len: up_to,
                } => segment_payload_len <= up_to,
                SegmentCompression::Never => false,
            };
            let (chunk, compressed_candidate) = if attempt {
                tokio::task::spawn_blocking(move || {
                    let candidate = match &first_segment_block {
                        Some(packed) => {
                            let mut composite = Vec::with_capacity(
                                METADATA_PREFIX_LEN + packed.len() + chunk.len(),
                            );
                            composite.extend_from_slice(&(packed.len() as u32).to_be_bytes()[1..]);
                            composite.extend_from_slice(packed);
                            composite.extend_from_slice(&chunk);
                            compression::compress_if_smaller(&composite)
                                .map(HostResourcePayload::from)
                        }
                        None => {
                            compression::compress_if_smaller(&chunk).map(HostResourcePayload::from)
                        }
                    };
                    (chunk, candidate)
                })
                .await
                .map_err(|_| ResourceSendError::NodeStopped)?
            } else {
                (chunk, None)
            };
            let metadata = match (&packed_metadata, segment_index) {
                (None, _) => HostResourceMetadata::None,
                (Some(packed), 1) => HostResourceMetadata::Packed(packed.clone().into()),
                (Some(packed), _) => HostResourceMetadata::SentInFirstSegment {
                    packed_len: packed.len() as u32,
                },
            };
            if in_flight.len() == ENGINE_SEGMENT_LANES {
                if let Some(settled) = in_flight.pop_front() {
                    settle_sent_segment(settled).await?;
                }
            }
            let id = self.mint();
            let (completion, settled) = oneshot::channel();
            self.commands
                .send(HostCommand::SendResourceSegment(
                    SendResourceSegmentHostCommand {
                        id,
                        link_id,
                        data: chunk.into(),
                        compressed_candidate,
                        metadata,
                        request_id: answers_request,
                        segment_index,
                        total_segments,
                        total_data_size: total_len,
                        completion,
                    },
                ))
                .map_err(|_| ResourceSendError::NodeStopped)?;
            in_flight.push_back(settled);
        }
        for settled in in_flight {
            settle_sent_segment(settled).await?;
        }
        Ok(())
    }

    /// Receive the next inbound resource on `link_id`, streaming it into `sink`: the mirror of [`send_resource`](Self::send_resource). Registers the sink before yielding, so a segment arriving the instant after cannot reach the app event stream instead; resolves with the assembled identity and size.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.resource.receive",
            level = "debug",
            skip_all,
            fields(link_id = ?link_id.as_bytes()),
            err(Debug)
        )
    )]
    pub async fn receive_resource(
        &self,
        link_id: LinkId,
        mut sink: impl AsyncWrite + Unpin,
    ) -> Result<ResourceReceipt, ResourceReceiveError> {
        let (chunks, mut inbound) = mpsc::unbounded_channel();
        let (ready, registered) = oneshot::channel();
        self.commands
            .send(HostCommand::RegisterResourceSink {
                link_id,
                sink: chunks,
                ready,
            })
            .map_err(|_| ResourceReceiveError::NodeStopped)?;
        registered
            .await
            .map_err(|_| ResourceReceiveError::NodeStopped)?;
        let mut metadata = None;
        loop {
            match inbound.recv().await {
                Some(ResourceInbound::Metadata(packed)) => metadata = Some(packed),
                Some(ResourceInbound::Chunk(bytes)) => {
                    sink.write_all(&bytes)
                        .await
                        .map_err(ResourceReceiveError::Sink)?;
                }
                Some(ResourceInbound::Complete {
                    original_hash,
                    total_size,
                }) => {
                    sink.flush().await.map_err(ResourceReceiveError::Sink)?;
                    return Ok(ResourceReceipt {
                        original_hash,
                        total_size,
                        metadata,
                    });
                }
                Some(ResourceInbound::Failed) => return Err(ResourceReceiveError::Failed),
                None => return Err(ResourceReceiveError::NodeStopped),
            }
        }
    }

    /// Set the default resource strategy for one of this node's destinations, returning whether the node holds it. The recipe's `resource_strategy` sets this at construction; this is the runtime counterpart, for a destination re-tuned while the node runs.
    pub async fn set_resource_strategy(
        &self,
        destination: DestinationHash,
        strategy: ResourceStrategy,
    ) -> bool {
        let (ready, applied) = oneshot::channel();
        if self
            .commands
            .send(HostCommand::SetResourceStrategy {
                destination,
                strategy,
                ready,
            })
            .is_err()
        {
            return false;
        }
        applied.await.unwrap_or(false)
    }

    /// A consistent image of every persisted region, serialized on the reactor — the one place a consistent view exists — with the engine instant it was taken at. `None` once the node has stopped.
    pub async fn snapshot_persisted_state(&self) -> Option<PersistedStateSnapshot> {
        let (reply, snapshot) = oneshot::channel();
        if self
            .commands
            .send(HostCommand::SnapshotPersistedState { reply })
            .is_err()
        {
            return None;
        }
        snapshot.await.ok()
    }

    #[cfg(feature = "runtime-metrics")]
    pub async fn metrics_snapshot(&self) -> Option<RuntimeMetricsSnapshot> {
        let (reply, snapshot) = oneshot::channel();
        if self
            .commands
            .send(HostCommand::SnapshotMetrics { reply })
            .is_err()
        {
            return None;
        }
        snapshot.await.ok()
    }

    /// One full flush, unconditionally rewriting every region — right for a shutdown handler; an interval loop wants [`flush_changed_to_store`](Self::flush_changed_to_store).
    pub async fn flush_to_store<P: PersistedStore>(
        &self,
        store: &mut P,
    ) -> Result<InstantMillis, FlushError<P::Error>> {
        self.prepare_flush()
            .await
            .map_err(|PrepareFlushError::NodeStopped| FlushError::NodeStopped)?
            .commit_to_store(store, &mut FlushMark::default())
            .map(|report| report.high_water)
    }

    pub async fn prepare_flush(&self) -> Result<PreparedFlush, PrepareFlushError> {
        self.snapshot_persisted_state()
            .await
            .map(|snapshot| PreparedFlush { snapshot })
            .ok_or(PrepareFlushError::NodeStopped)
    }

    /// The interval-cadence flush: a region whose sealed image fingerprints the same as `mark`'s last landed flush is skipped, so a quiet node's tick writes 22 bytes of timebase and nothing else. The timebase always writes — it is the restored timeline's rollback floor and it advances with uptime even while the tables sit still — and it lands before the region images it stamps: a crash between them leaves a newer high-water over older rows, which only over-ages the restored timeline, where the reverse order could strand rows in a wall-less boot's future, never expiring.
    #[allow(clippy::expect_used)]
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "prns.persistence.flush", level = "debug", skip_all)
    )]
    pub async fn flush_changed_to_store<P: PersistedStore>(
        &self,
        store: &mut P,
        mark: &mut FlushMark,
    ) -> Result<FlushReport, FlushError<P::Error>> {
        self.prepare_flush()
            .await
            .map_err(|PrepareFlushError::NodeStopped| FlushError::NodeStopped)?
            .commit_to_store(store, mark)
    }

    /// Every tracked destination's sealed self-ratchet record, serialized on the reactor. `None` once the node has stopped.
    pub async fn snapshot_self_ratchets(&self) -> Option<SelfRatchetsSnapshot> {
        let (reply, snapshot) = oneshot::channel();
        if self
            .commands
            .send(HostCommand::SnapshotSelfRatchets { reply })
            .is_err()
        {
            return None;
        }
        snapshot.await.ok()
    }

    pub async fn snapshot_self_ratchet(
        &self,
        destination: DestinationHash,
    ) -> Result<Option<SelfRatchetSnapshot>, PrepareFlushError> {
        let (reply, snapshot) = oneshot::channel();
        self.commands
            .send(HostCommand::SnapshotSelfRatchet { destination, reply })
            .map_err(|_| PrepareFlushError::NodeStopped)?;
        snapshot.await.map_err(|_| PrepareFlushError::NodeStopped)
    }

    /// Flush every destination's self-ratchet record to `vault`, returning how many landed. Ratchet secrets never touch a [`PersistedStore`]: the vault is where the identity secret itself lives, so the record inherits its protections.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "prns.persistence.flush_ratchets", level = "debug", skip_all)
    )]
    pub async fn flush_ratchets_to_vault<V: IdentityVault>(
        &self,
        vault: &mut V,
    ) -> Result<u32, FlushError<V::Error>> {
        let snapshot = self
            .snapshot_self_ratchets()
            .await
            .ok_or(FlushError::NodeStopped)?;
        snapshot.store_into(vault).map_err(FlushError::Store)
    }

    pub async fn flush_ratchet_to_vault<V: IdentityVault>(
        &self,
        destination: DestinationHash,
        vault: &mut V,
    ) -> Result<bool, FlushError<V::Error>> {
        let snapshot = self
            .snapshot_self_ratchet(destination)
            .await
            .map_err(|PrepareFlushError::NodeStopped| FlushError::NodeStopped)?;
        match snapshot {
            Some(snapshot) => snapshot
                .store_into(vault)
                .map(|()| true)
                .map_err(FlushError::Store),
            None => Ok(false),
        }
    }

    /// Bring a link up to `destination` and await it: `Ok(LinkId)` once the peer's proof validates, or the typed reason it never established. The resolved id is the handle every link-scoped verb takes.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.command.establish_link",
            level = "debug",
            skip_all,
            fields(destination = ?destination.as_bytes()),
            err(Debug)
        )
    )]
    pub async fn establish_link(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        match self
            .settle(EngineCommand::EstablishLink(EstablishLink { destination }))
            .await
        {
            Some(Settlement::EstablishLink(result)) => result
                .map(|established| established.link_id)
                .map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub async fn announce_now(
        &self,
        announce: AnnounceNow,
    ) -> Result<(), SendError<AnnounceNowFailure>> {
        match self.settle(EngineCommand::AnnounceNow(announce)).await {
            Some(Settlement::AnnounceNow(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub(crate) async fn settle(&self, command: EngineCommand) -> Option<Settlement> {
        let id = self.mint();
        let (completion, settled) = oneshot::channel();
        self.commands
            .send(HostCommand::AwaitedEngine {
                issued: IssuedCommand { id, command },
                completion,
            })
            .ok()?;
        settled.await.ok()
    }

    async fn introspect<T>(
        &self,
        request: impl FnOnce(oneshot::Sender<T>) -> NodeIntrospectionRequest,
    ) -> Option<T> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(HostCommand::NodeIntrospection(request(reply)))
            .ok()?;
        response.await.ok()
    }

    fn send_response(
        &self,
        responder: RespondToken,
        data: HostResourcePayload,
    ) -> Option<RttMillis> {
        let id = self.mint();
        if data.len() <= RESPONSE_PACKET_CEILING {
            return self
                .commands
                .send(HostCommand::RespondAny(RespondAnyHostCommand {
                    id,
                    link_id: responder.link_id,
                    request_id: responder.request_id,
                    data,
                    compressed_candidate: None,
                }))
                .ok()
                .map(|()| responder.rtt);
        }
        if self.commands.is_closed() {
            return None;
        }
        if data.len() > MAX_EFFICIENT_SIZE {
            let handle = self.clone();
            let link_id = responder.link_id;
            let request_id = responder.request_id;
            tokio::spawn(async move {
                let total_len = data.len() as u64;
                let _ = handle
                    .send_resource_streaming(
                        link_id,
                        total_len,
                        std::io::Cursor::new(data),
                        None,
                        SegmentCompression::AUTO,
                        Some(request_id),
                    )
                    .await;
            });
            return Some(responder.rtt);
        }
        let commands = self.commands.clone();
        let link_id = responder.link_id;
        let request_id = responder.request_id;
        tokio::spawn(async move {
            let Ok((data, compressed_candidate)) = tokio::task::spawn_blocking(move || {
                let candidate = compression::compress_if_smaller(data.as_slice())
                    .map(HostResourcePayload::from);
                (data, candidate)
            })
            .await
            else {
                return;
            };
            let _ = commands.send(HostCommand::RespondAny(RespondAnyHostCommand {
                id,
                link_id,
                request_id,
                data,
                compressed_candidate,
            }));
        });
        Some(responder.rtt)
    }

    /// Answer a request via its token, returning the link's round trip (the request arrived over it) — or `None` if the node has stopped before the answer could be queued.
    pub fn respond(&self, responder: RespondToken, body: &[u8]) -> Option<RttMillis> {
        self.send_response(responder, body.to_vec().into())
    }

    pub fn respond_owned(
        &self,
        responder: RespondToken,
        body: std::vec::Vec<u8>,
    ) -> Option<RttMillis> {
        self.send_response(responder, body.into())
    }

    /// Open a byte-stream reader on this link and stream id. Awaits the run loop's acknowledgement that the sink is live before yielding the reader, so a chunk arriving the instant the link opens is buffered for the reader, never forwarded past it to the app.
    pub async fn byte_stream_reader(
        &self,
        link_id: LinkId,
        stream_id: StreamId,
    ) -> ByteStreamReader {
        let (sink, inbound) = mpsc::unbounded_channel();
        let (ready, registered) = oneshot::channel();
        let _ = self.commands.send(HostCommand::RegisterStreamReader {
            link_id,
            stream_id,
            sink,
            ready,
        });
        let _ = registered.await;
        ByteStreamReader::new(inbound)
    }

    /// Open a byte-stream writer on this link and stream id: an `AsyncWrite` framing each write as a stream-data channel send.
    pub fn byte_stream_writer(&self, link_id: LinkId, stream_id: StreamId) -> ByteStreamWriter {
        ByteStreamWriter::new(self.clone(), link_id, stream_id)
    }

    /// Open a bidirectional byte stream: a reader on `rx` and a writer on `tx` over one link's channel, RNS's `create_bidirectional_buffer`. Awaits the reader's registration (see [`byte_stream_reader`](Self::byte_stream_reader)) so the read half is live before either is handed back.
    pub async fn byte_stream(
        &self,
        link_id: LinkId,
        rx: StreamId,
        tx: StreamId,
    ) -> (ByteStreamReader, ByteStreamWriter) {
        (
            self.byte_stream_reader(link_id, rx).await,
            self.byte_stream_writer(link_id, tx),
        )
    }

    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.issue(EngineCommand::CloseLink(CloseLink { link_id }))
            .is_some()
    }

    /// Attach an interface to the running node and get a handle to tear it back down. Grab any per-interface control handle (`.status()`, a radio's own controls) before calling this, since it takes the interface by value.
    ///
    /// `I: Send` is the host's bargain: the interface rides to the `run` task inside a `Send` builder closure which mints its run future there, so the future itself never has to be `Send` (what keeps `!Send` interface bodies legal) and the reactor stays `Send` and spawnable.
    pub fn add_interface<I>(&self, interface: I) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_with_metadata(
            interface,
            InterfaceAttachmentMetadata {
                name: None,
                origin: InterfaceOriginKind::Configured,
            },
        )
    }

    pub fn add_interface_with_ifac<I>(&self, interface: I, ifac: IfacContext) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_with_ifac_name(interface, ifac, None)
    }

    pub fn add_interface_with_ifac_name<I>(
        &self,
        interface: I,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_with_metadata_and_ifac_name(
            interface,
            InterfaceAttachmentMetadata {
                name: None,
                origin: InterfaceOriginKind::Configured,
            },
            ifac,
            network_name,
        )
    }

    pub fn add_interface_with_metadata<I>(
        &self,
        interface: I,
        metadata: InterfaceAttachmentMetadata,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_access(interface, metadata, None)
    }

    pub fn add_interface_with_metadata_and_ifac_name<I>(
        &self,
        interface: I,
        metadata: InterfaceAttachmentMetadata,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_access(
            interface,
            metadata,
            Some(RuntimeIfac {
                context: ifac,
                network_name,
            }),
        )
    }

    fn add_interface_access<I>(
        &self,
        interface: I,
        metadata: InterfaceAttachmentMetadata,
        ifac: Option<RuntimeIfac>,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        let InterfaceAttachmentMetadata { name, origin } = metadata;
        let placement = InterfacePlacement {
            membership: Membership::Independent,
            origin,
        };
        let view = interface.status_view();
        let connection = interface.connection_view();
        let attached = attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            placement,
            connection,
            ifac.as_ref().map(|access| access.context.clone()),
        );
        register_status(
            &self.interfaces,
            attached.id(),
            view,
            placement,
            ifac.as_ref().map(RuntimeIfac::snapshot),
            name,
        );
        attached
    }

    #[must_use]
    pub fn interface_inventory(&self) -> std::vec::Vec<InterfaceInventoryEntry> {
        let Ok(map) = self.interfaces.lock() else {
            return std::vec::Vec::new();
        };
        map.values()
            .flat_map(|registered| {
                let placement = registered.placement;
                let ifac = registered.ifac.clone();
                let name = registered.name.clone();
                (registered.view)().into_iter().map(move |vitals| {
                    let counts = self.store.counts(vitals.id);
                    InterfaceInventoryEntry {
                        name: name.clone(),
                        origin: placement.origin,
                        snapshot: InterfaceSnapshot {
                            id: vitals.id,
                            connection: vitals.connection,
                            failure_reason: vitals.failure_reason,
                            rx_bytes: vitals.rx_bytes,
                            tx_bytes: vitals.tx_bytes,
                            transfer_rates: vitals.transfer_rates,
                            destinations: counts.destinations,
                            links: counts.links,
                            transported_links: counts.transported_links,
                            membership: placement.membership,
                        },
                        ifac: ifac.clone(),
                    }
                })
            })
            .collect()
    }

    #[must_use]
    pub fn set_interface_name(&self, id: InterfaceId, name: impl Into<String>) -> bool {
        let Ok(mut interfaces) = self.interfaces.lock() else {
            return false;
        };
        let Some(interface) = interfaces.get_mut(&id) else {
            return false;
        };
        interface.name = Some(name.into());
        true
    }

    /// Every interface attached through this handle, as a complete [`InterfaceSnapshot`]: live vitals read at call time joined with the engine counts and fleet position. The raw fleet an inspection face can project for its own presentation, with no app-side bookkeeping.
    #[must_use]
    pub fn interfaces(&self) -> std::vec::Vec<InterfaceSnapshot> {
        self.interface_inventory()
            .into_iter()
            .map(|entry| entry.snapshot)
            .collect()
    }

    /// Attach an interface supervisor: a node that owns no wire of its own but stands up a fleet member per validated connection through the [`Fleet`] handle it is given. The supervisor is no engine interface (no descriptor, no lanes); each member is an ordinary flat interface recorded under it, so teardown cascades to the whole fleet.
    pub fn supervise<S>(&self, supervisor: S) -> AttachedSupervisor
    where
        S: InterfaceSupervisor + ReportsStatus + Send + 'static,
    {
        self.supervise_access(supervisor, None)
    }

    pub fn supervise_with_ifac<S>(&self, supervisor: S, ifac: IfacContext) -> AttachedSupervisor
    where
        S: InterfaceSupervisor + ReportsStatus + Send + 'static,
    {
        self.supervise_with_ifac_name(supervisor, ifac, None)
    }

    pub fn supervise_with_ifac_name<S>(
        &self,
        supervisor: S,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedSupervisor
    where
        S: InterfaceSupervisor + ReportsStatus + Send + 'static,
    {
        self.supervise_access(
            supervisor,
            Some(RuntimeIfac {
                context: ifac,
                network_name,
            }),
        )
    }

    fn supervise_access<S>(&self, supervisor: S, ifac: Option<RuntimeIfac>) -> AttachedSupervisor
    where
        S: InterfaceSupervisor + ReportsStatus + Send + 'static,
    {
        let id = InterfaceId::from_channel_tag(S::KIND, supervisor.channel_tag());
        let placement = InterfacePlacement {
            membership: Membership::Independent,
            origin: InterfaceOriginKind::Configured,
        };
        let view = supervisor.status_view();
        let ifac_status = ifac.as_ref().map(RuntimeIfac::snapshot);
        let fleet = Fleet {
            supervisor_id: id,
            commands: self.commands.clone(),
            iface_build: self.iface_build.clone(),
            notify_tx: self.notify_tx.clone(),
            interfaces: self.interfaces.clone(),
            ifac,
        };
        let build: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send> =
            Box::new(move || Box::pin(supervisor.run(fleet)));
        let _ = self.iface_build.send(DriverMsg::Add {
            id,
            supervisor: None,
            build,
        });
        register_status(&self.interfaces, id, view, placement, ifac_status, None);
        AttachedSupervisor {
            id,
            iface_build: self.iface_build.clone(),
        }
    }

    /// Detach the interface with this id (the inverse of [`add_interface`](Self::add_interface)): deregister its lanes on the reactor and stop its run future on the driver. For a supervisor, the driver cascades the stop to every member of its fleet. The routes learned through it stay warm for the departure grace, so a same-identity re-attach (a radio toggled off and on, a retune switched back) restores them; [`forget_interface`](Self::forget_interface) is the detach that drops them at once.
    pub fn remove_interface(&self, id: InterfaceId) {
        let _ = self.commands.send(HostCommand::RemoveInterface {
            id,
            departure: Departure::MayReturn,
        });
        let _ = self.iface_build.send(DriverMsg::Stop { id });
    }

    /// Detach like [`remove_interface`](Self::remove_interface) and drop the routes learned through the interface at once, instead of holding them warm for a return.
    pub fn forget_interface(&self, id: InterfaceId) {
        let _ = self.commands.send(HostCommand::RemoveInterface {
            id,
            departure: Departure::Forgotten,
        });
        let _ = self.iface_build.send(DriverMsg::Stop { id });
    }

    /// Attach anything from the interface menu and get back its kind's attachment handle — the one verb over [`add_interface`](Self::add_interface) and [`supervise`](Self::supervise).
    pub fn attach<A: Attachable>(&self, attachable: A) -> A::Attached {
        attachable.attach_to(self)
    }

    pub fn attach_with_ifac<A: Attachable>(&self, attachable: A, ifac: IfacContext) -> A::Attached {
        attachable.attach_to_with_ifac(self, ifac, None)
    }

    pub fn attach_with_ifac_name<A: Attachable>(
        &self,
        attachable: A,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> A::Attached {
        attachable.attach_to_with_ifac(self, ifac, network_name)
    }
}

/// One registration story per menu type: the type itself encodes whether it joins as a single wire (`add_interface`) or a discovery fleet (`supervise`), so no callsite has to know.
pub trait Attachable {
    type Attached;
    fn attach_to(self, handle: &PrnsNodeHandle) -> Self::Attached;
    fn attach_to_with_ifac(
        self,
        handle: &PrnsNodeHandle,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> Self::Attached;
}

/// The recipe's `interfaces` answer: [`Manual`] says the app attaches through the handle itself, a closure over the handle is the inline shopping list, prefabs compose the common cases.
pub trait AttachIntent {
    fn attach(self, handle: &PrnsNodeHandle);
}

impl AttachIntent for Manual {
    fn attach(self, _handle: &PrnsNodeHandle) {}
}

impl<F: FnOnce(&PrnsNodeHandle)> AttachIntent for F {
    fn attach(self, handle: &PrnsNodeHandle) {
        self(handle)
    }
}

async fn settle_routing_control<T, F>(
    commands: UnboundedSender<HostCommand>,
    build: F,
) -> Result<T, RoutingControlError>
where
    T: Send,
    F: FnOnce(oneshot::Sender<T>) -> HostCommand + Send,
{
    let (reply, settled) = oneshot::channel();
    commands
        .send(build(reply))
        .map_err(|_| RoutingControlError::NodeStopped)?;
    settled.await.map_err(|_| RoutingControlError::NodeStopped)
}

impl RoutingControl for PrnsNodeHandle {
    fn drop_route(
        &self,
        destination: DestinationHash,
    ) -> impl Future<Output = Result<DropRouteOutcome, RoutingControlError>> + Send {
        settle_routing_control(self.commands.clone(), move |reply| HostCommand::DropRoute {
            destination,
            reply,
        })
    }

    fn drop_routes_via(
        &self,
        transport: TransportId,
    ) -> impl Future<Output = Result<DropRoutesViaOutcome, RoutingControlError>> + Send {
        settle_routing_control(self.commands.clone(), move |reply| {
            HostCommand::DropRoutesVia { transport, reply }
        })
    }

    fn clear_announce_queues(
        &self,
    ) -> impl Future<Output = Result<ClearAnnounceQueuesOutcome, RoutingControlError>> + Send {
        settle_routing_control(self.commands.clone(), |reply| {
            HostCommand::ClearAnnounceQueues { reply }
        })
    }
}

impl DestinationIdentityRetentionControl for PrnsNodeHandle {
    fn mark_destination_used(
        &self,
        destination: DestinationHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::MarkDestinationUsedOutcome,
            DestinationIdentityRetentionControlError,
        >,
    > + Send {
        settle_destination_identity_retention(self.commands.clone(), move |reply| {
            DestinationIdentityRetentionHostCommand::MarkUsed { destination, reply }
        })
    }

    fn retain_destination(
        &self,
        destination: DestinationHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::RetainDestinationOutcome,
            DestinationIdentityRetentionControlError,
        >,
    > + Send {
        settle_destination_identity_retention(self.commands.clone(), move |reply| {
            DestinationIdentityRetentionHostCommand::RetainDestination { destination, reply }
        })
    }

    fn release_destination(
        &self,
        destination: DestinationHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::ReleaseDestinationOutcome,
            DestinationIdentityRetentionControlError,
        >,
    > + Send {
        settle_destination_identity_retention(self.commands.clone(), move |reply| {
            DestinationIdentityRetentionHostCommand::ReleaseDestination { destination, reply }
        })
    }

    fn retain_identity(
        &self,
        identity: crate::identity::IdentityHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::RetainIdentityOutcome,
            DestinationIdentityRetentionControlError,
        >,
    > + Send {
        settle_destination_identity_retention(self.commands.clone(), move |reply| {
            DestinationIdentityRetentionHostCommand::RetainIdentity { identity, reply }
        })
    }
}

impl IdentityBlackholeSource for PrnsNodeHandle {
    type Reason = String;
    type Entries = std::vec::Vec<BlackholedIdentity<String>>;

    fn blackholed_identities(
        &self,
    ) -> impl Future<Output = Result<Self::Entries, IdentityBlackholeSourceError>> + Send {
        settle_source(self.commands.clone(), |reply| {
            IdentityBlackholeHostCommand::ReadAll { reply }
        })
    }

    fn is_blackholed(
        &self,
        identity: crate::identity::IdentityHash,
    ) -> impl Future<Output = Result<bool, IdentityBlackholeSourceError>> + Send {
        settle_source(self.commands.clone(), move |reply| {
            IdentityBlackholeHostCommand::IsBlackholed { identity, reply }
        })
    }
}

impl IdentityBlackholeControl for PrnsNodeHandle {
    fn blackhole_identity<'a>(
        &'a self,
        entry: BlackholedIdentity<&'a str>,
    ) -> impl Future<
        Output = Result<crate::routing::BlackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send
           + 'a {
        let entry = BlackholedIdentity {
            identity: entry.identity,
            source: entry.source,
            expiry: entry.expiry,
            reason: entry.reason.map(String::from),
        };
        settle_control(self.commands.clone(), move |reply| {
            IdentityBlackholeHostCommand::Blackhole { entry, reply }
        })
    }

    fn unblackhole_identity(
        &self,
        identity: crate::identity::IdentityHash,
    ) -> impl Future<
        Output = Result<crate::routing::UnblackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send {
        settle_control(self.commands.clone(), move |reply| {
            IdentityBlackholeHostCommand::Unblackhole { identity, reply }
        })
    }
}

impl NodeIntrospection for PrnsNodeHandle {
    fn interface_inventory(&self) -> std::vec::Vec<InterfaceInventoryEntry> {
        PrnsNodeHandle::interface_inventory(self)
    }

    async fn link_count(&self) -> u32 {
        self.introspect(|reply| NodeIntrospectionRequest::LinkCount { reply })
            .await
            .unwrap_or_default()
    }

    fn packet_phy(&self, packet_hash: PacketHash) -> Option<PacketPhyStats> {
        self.store.packet_phy(packet_hash)
    }

    async fn announce_rates(&self) -> std::vec::Vec<AnnounceRateSnapshot> {
        self.introspect(|reply| NodeIntrospectionRequest::AnnounceRates { reply })
            .await
            .unwrap_or_default()
    }

    async fn routes(&self) -> std::vec::Vec<RouteSnapshot> {
        self.introspect(|reply| NodeIntrospectionRequest::Routes { reply })
            .await
            .unwrap_or_default()
    }

    async fn route(&self, destination: DestinationHash) -> Option<RouteSnapshot> {
        self.introspect(|reply| NodeIntrospectionRequest::Route { destination, reply })
            .await
            .flatten()
    }
}

impl super::PrnsNodeApi for PrnsNodeHandle {
    fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        self.issue(command)
    }

    async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        self.send_single_packet(destination, data).await
    }

    fn respond(&self, responder: RespondToken, body: &[u8]) -> bool {
        self.respond(responder, body).is_some()
    }

    fn close_link(&self, link_id: LinkId) -> bool {
        self.close_link(link_id)
    }
}

/// A handle to one interface attached at runtime: its minted id and the lever to detach it. Dropping the handle leaves the interface running; only [`teardown`](Self::teardown) (or [`PrnsNodeHandle::remove_interface`]) takes it down.
pub struct AttachedInterface {
    id: InterfaceId,
    commands: UnboundedSender<HostCommand>,
    iface_build: UnboundedSender<DriverMsg>,
}

impl AttachedInterface {
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// Detach the interface: deregister its lanes on the reactor and stop its run future. Its routes stay warm for the departure grace, so a same-identity re-attach restores them.
    pub fn teardown(self) {
        let _ = self.commands.send(HostCommand::RemoveInterface {
            id: self.id,
            departure: Departure::MayReturn,
        });
        let _ = self.iface_build.send(DriverMsg::Stop { id: self.id });
    }
}

/// A handle to a supervisor attached through [`PrnsNodeHandle::supervise`]. Teardown is a single stop on the driver, ending its discovery loop and cascading to its whole fleet; dropping the handle leaves it running.
pub struct AttachedSupervisor {
    id: InterfaceId,
    iface_build: UnboundedSender<DriverMsg>,
}

impl AttachedSupervisor {
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// Detach the supervisor: stop its discovery loop and cascade teardown to its whole fleet.
    pub fn teardown(self) {
        let _ = self.iface_build.send(DriverMsg::Stop { id: self.id });
    }
}

/// The engine's outgoing lanes per link: the transferring segment plus one staged continuation, so a dispatch window this deep keeps the next seal overlapping the wire.
const ENGINE_SEGMENT_LANES: usize = 2;

/// A dispatched segment's settlement, awaited only once the dispatch window is full, so segment `n+1`'s read, compress, and engine seal all overlap segment `n`'s flight instead of following it.
async fn settle_sent_segment(
    settled: oneshot::Receiver<Settlement>,
) -> Result<(), ResourceSendError> {
    match settled.await {
        Ok(Settlement::SendResource(Ok(()))) => Ok(()),
        Ok(Settlement::SendResource(Err(failure))) => Err(ResourceSendError::Rejected(failure)),
        Ok(_) | Err(_) => Err(ResourceSendError::NodeStopped),
    }
}

/// Wire one interface onto the running node: build its grant lanes + seam, hand the reactor the `Send` lane halves, and hand the driver the `Send` builder that mints its run future. `supervisor` records it as a fleet member so the driver cascades teardown.
fn attach_interface<I>(
    commands: &UnboundedSender<HostCommand>,
    iface_build: &UnboundedSender<DriverMsg>,
    notify_tx: &UnboundedSender<InterfaceId>,
    interface: I,
    placement: InterfacePlacement,
    connection: Option<ConnectionView>,
    ifac: Option<IfacContext>,
) -> AttachedInterface
where
    I: Interface + Send + 'static,
{
    let descriptor = interface.descriptor();
    let id = descriptor.id;
    let supervisor = match placement.membership {
        Membership::Independent => None,
        Membership::FleetMember { supervisor_id } => Some(supervisor_id),
    };
    let logical_interface = supervisor.unwrap_or(id);
    let slot_cap = frame_cap_for(&descriptor);
    let depth = lane_depth_for(slot_cap);
    let (in_producer, in_consumer) = tokio_grant_lane(slot_cap, depth);
    let (out_producer, out_consumer) = tokio_grant_lane(slot_cap, depth);
    let seam = TokioInterfaceSeam::new(id, in_producer, notify_tx.clone(), out_consumer)
        .with_origin(placement.origin)
        .with_commands(commands.clone());
    let build: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send> =
        Box::new(move || Box::pin(interface.run(seam)));
    let _ = commands.send(HostCommand::AddInterface(AddInterfaceCommand {
        descriptor,
        logical_interface,
        inbound: in_consumer,
        egress: out_producer,
        connection,
        ifac,
    }));
    let _ = iface_build.send(DriverMsg::Add {
        id,
        supervisor,
        build,
    });
    AttachedInterface {
        id,
        commands: commands.clone(),
        iface_build: iface_build.clone(),
    }
}

/// A supervisor's lever to stand up fleet members. Each [`add`](Self::add) registers a flat engine interface recorded as this supervisor's member; the supervisor typically holds the returned [`AttachedInterface`] to detach that member when its link drops.
pub struct Fleet {
    supervisor_id: InterfaceId,
    commands: UnboundedSender<HostCommand>,
    iface_build: UnboundedSender<DriverMsg>,
    notify_tx: UnboundedSender<InterfaceId>,
    interfaces: Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    ifac: Option<RuntimeIfac>,
}

impl Fleet {
    /// Stand up a fleet member under this supervisor — identical to [`PrnsNodeHandle::add_interface`] except the member is recorded as this supervisor's, so a supervisor teardown takes it with it.
    pub fn add<I>(&self, interface: I) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        let view = interface.status_view();
        let connection = interface.connection_view();
        let placement = InterfacePlacement {
            membership: Membership::FleetMember {
                supervisor_id: self.supervisor_id,
            },
            origin: InterfaceOriginKind::Configured,
        };
        let attached = attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            placement,
            connection,
            self.ifac.as_ref().map(|access| access.context.clone()),
        );
        register_status(
            &self.interfaces,
            attached.id(),
            view,
            placement,
            self.ifac.as_ref().map(RuntimeIfac::snapshot),
            None,
        );
        attached
    }

    /// A [`Fleet`] wired to no reactor: member builds and host commands flow into the returned [`DetachedFleet`] tail and go nowhere. For driving a supervisor by hand (unit tests, a bench harness).
    #[must_use]
    pub fn detached(supervisor_id: InterfaceId) -> (Self, DetachedFleet) {
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let (iface_build, iface_build_rx) = mpsc::unbounded_channel();
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let fleet = Fleet {
            supervisor_id,
            commands,
            iface_build,
            notify_tx,
            interfaces: Arc::new(Mutex::new(HashMap::new())),
            ifac: None,
        };
        let tail = DetachedFleet {
            _commands: commands_rx,
            _iface_build: iface_build_rx,
            _notify: notify_rx,
        };
        (fleet, tail)
    }
}

/// The unplugged end of [`Fleet::detached`]: holds the channel tails so the fleet's sends stay deliverable while a hand-driven harness runs. Drop it and sends start failing, like a runtime whose reactor exited.
pub struct DetachedFleet {
    _commands: UnboundedReceiver<HostCommand>,
    _iface_build: UnboundedReceiver<DriverMsg>,
    _notify: UnboundedReceiver<InterfaceId>,
}

/// An interface supervisor: a node that owns no wire of its own but runs a discovery loop and stands up a fleet member per validated connection. Attached with [`PrnsNodeHandle::supervise`].
#[allow(async_fn_in_trait)]
pub trait InterfaceSupervisor {
    /// The medium this supervisor stands for — the namespace root of its id.
    const KIND: InterfaceKind;

    /// The bytes that uniquely tag this supervisor, typically config-derived (the group it serves); the same rules as [`channel_tag`](crate::reactor::interface_seam::Interface::channel_tag) apply.
    fn channel_tag(&self) -> &[u8];

    async fn run(self, fleet: Fleet);
}

/// A message to the interface driver: a new interface to start driving, or a request to stop one. The driver lives on the `!Send` `run` task, so an interface's `!Send` run future never has to cross a thread — only the `Send` builder closure does.
enum DriverMsg {
    Add {
        id: InterfaceId,
        supervisor: Option<InterfaceId>,
        build: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send>,
    },
    Stop {
        id: InterfaceId,
    },
}

/// Drive every interface run future — the recipe's initial set, plus any added through the handle at runtime — on the `run` task. Each runtime-added interface is wrapped with a stop signal so [`PrnsNodeHandle::remove_interface`] can drop it mid-flight; the initial set runs for the node's life.
async fn drive_interfaces(
    initial: std::vec::Vec<Pin<Box<dyn Future<Output = ()>>>>,
    mut messages: UnboundedReceiver<DriverMsg>,
    commands: UnboundedSender<HostCommand>,
    interfaces: Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
) {
    let mut futures: FuturesUnordered<Pin<Box<dyn Future<Output = Option<InterfaceId>>>>> = initial
        .into_iter()
        .map(
            |run| -> Pin<Box<dyn Future<Output = Option<InterfaceId>>>> {
                Box::pin(async move {
                    run.await;
                    None
                })
            },
        )
        .collect();
    let mut stops: HashMap<InterfaceId, oneshot::Sender<()>> = HashMap::new();
    let mut supervisor_of: HashMap<InterfaceId, InterfaceId> = HashMap::new();
    let mut open = true;
    loop {
        if !open && futures.is_empty() {
            return;
        }
        tokio::select! {
            message = messages.recv(), if open => match message {
                Some(DriverMsg::Add { id, supervisor, build }) => {
                    if let Some(supervisor_id) = supervisor {
                        let _ = supervisor_of.insert(id, supervisor_id);
                    }
                    let run = build();
                    let (stop_tx, stop_rx) = oneshot::channel();
                    futures.push(Box::pin(async move {
                        tokio::select! {
                            () = run => {}
                            _ = stop_rx => {}
                        }
                        Some(id)
                    }));
                    stops.insert(id, stop_tx);
                }
                Some(DriverMsg::Stop { id }) => {
                    stop_interface(&mut stops, id);
                    supervisor_of.remove(&id);
                    forget_status(&interfaces, id);
                    let cascaded: std::vec::Vec<InterfaceId> = supervisor_of
                        .iter()
                        .filter(|(_, supervisor_id)| **supervisor_id == id)
                        .map(|(member, _)| *member)
                        .collect();
                    for member in cascaded {
                        stop_interface(&mut stops, member);
                        supervisor_of.remove(&member);
                        forget_status(&interfaces, member);
                        let _ = commands.send(HostCommand::RemoveInterface {
                            id: member,
                            departure: Departure::MayReturn,
                        });
                    }
                }
                None => open = false,
            },
            // An interface whose run future ended on its own (a dropped connection, no reconnect) deregisters itself: its descriptor must not outlive its wire. A future ended by a `Stop` already had its id pulled from `stops`, so the `stops.remove` here is what distinguishes a natural completion from a deliberate one.
            done = futures.next(), if !futures.is_empty() => {
                if let Some(Some(id)) = done {
                    if stops.remove(&id).is_some() {
                        supervisor_of.remove(&id);
                        forget_status(&interfaces, id);
                        let _ = commands.send(HostCommand::RemoveInterface {
                            id,
                            departure: Departure::MayReturn,
                        });
                    }
                }
            }
        }
    }
}

/// A status view the runtime tracks centrally, tagged with where its interface sits in the fleet. `interfaces()` joins each with the engine's count store to mint an `InterfaceSnapshot`.
struct RegisteredInterface {
    view: StatusView,
    placement: InterfacePlacement,
    ifac: Option<InterfaceIfacSnapshot>,
    name: Option<String>,
}

fn register_status(
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    id: InterfaceId,
    view: Option<StatusView>,
    placement: InterfacePlacement,
    ifac: Option<InterfaceIfacSnapshot>,
    name: Option<String>,
) {
    if let (Some(view), Ok(mut map)) = (view, interfaces.lock()) {
        map.insert(
            id,
            RegisteredInterface {
                view,
                placement,
                ifac,
                name,
            },
        );
    }
}

fn forget_status(
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    id: InterfaceId,
) {
    if let Ok(mut map) = interfaces.lock() {
        map.remove(&id);
    }
}

fn stop_interface(stops: &mut HashMap<InterfaceId, oneshot::Sender<()>>, id: InterfaceId) {
    if let Some(stop) = stops.remove(&id) {
        let _ = stop.send(());
    }
}

type AcceptedAnnounceObserver = Box<dyn for<'a> FnMut(AnnounceObservation<'a>) + Send>;

fn notify_accepted_announce(
    observer: &mut Option<AcceptedAnnounceObserver>,
    journaled: &Journaled<'_>,
) {
    if let Journaled::AnnounceHeard { observation, .. } = journaled {
        if let Some(observer) = observer.as_mut() {
            observer(*observation);
        }
    }
}

/// A node on the tokio host. Built from a [`PrnsNodeRecipe`] with [`new`](Self::new) (synchronous: it wires the engine and spawns each interface), then driven by [`run`](Self::run). Hold [`handle`](Self::handle) clones to drive it from other tasks/threads while `run` owns the loop.
pub struct PrnsNode<St, R, F, S: StorageLayout> {
    handle: PrnsNodeHandle,
    host: TokioHost,
    node: AssembledNode<St, R, F, S>,
    notify_rx: UnboundedReceiver<InterfaceId>,
    command_rx: UnboundedReceiver<HostCommand>,
    iface_build_rx: UnboundedReceiver<DriverMsg>,
    accepted_announce_observer: Option<AcceptedAnnounceObserver>,
    crypto_pool: CryptoPoolConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonRoutingIdentityError {
    Hold(HoldIdentityError),
    Configure(SetTransportIdentityError),
}

pub type SharedInstanceIdentityError = NonRoutingIdentityError;

impl<St, R, F, S: StorageLayout> PrnsNode<St, R, F, S>
where
    R: RouteSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
{
    /// Stand a node up from `recipe` on the storage layout it names: assemble the engine (transport role, destinations, the routes' request handlers), then let the recipe's `interfaces` intent attach the node's edges through its own handle. Only [`run`](Self::run) awaits.
    pub fn new<'a, D, I>(recipe: PrnsNodeRecipe<D, St, R, F, I, S>) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'a>>,
        I: AttachIntent,
    {
        Self::new_with_handle(|_| recipe)
    }

    pub fn new_with_handle<'a, D, I, B>(build_recipe: B) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'a>>,
        I: AttachIntent,
        B: FnOnce(PrnsNodeHandle) -> PrnsNodeRecipe<D, St, R, F, I, S>,
    {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (iface_build_tx, iface_build_rx) = mpsc::unbounded_channel();

        let handle = PrnsNodeHandle {
            commands: command_tx,
            ids: Arc::new(AtomicU64::new(0)),
            notify_tx,
            iface_build: iface_build_tx,
            interfaces: Arc::new(Mutex::new(HashMap::new())),
            store: InterfaceStore::new(),
        };
        let (node, interfaces) = assemble_node(build_recipe(handle.clone()));
        interfaces.attach(&handle);

        PrnsNode {
            handle,
            host: TokioHost::start_at(wall_clock_timeline_origin()),
            node,
            notify_rx,
            command_rx,
            iface_build_rx,
            accepted_announce_observer: None,
            crypto_pool: CryptoPoolConfig::host_default(),
        }
    }

    pub fn with_non_routing_identity(
        mut self,
        secret: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    ) -> Result<Self, NonRoutingIdentityError> {
        let identity = self
            .node
            .engine
            .hold_identity(secret)
            .map_err(NonRoutingIdentityError::Hold)?;
        self.node
            .engine
            .set_non_routing_identity(&identity)
            .map_err(NonRoutingIdentityError::Configure)?;
        Ok(self)
    }

    pub fn with_shared_instance_identity(
        self,
        secret: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    ) -> Result<Self, SharedInstanceIdentityError> {
        self.with_non_routing_identity(secret)
    }

    #[must_use]
    pub fn with_protocol_policy(mut self, policy: crate::engine::EngineProtocolPolicy) -> Self {
        self.node.engine.set_protocol_policy(policy);
        self
    }

    pub fn register_preconfigured_destination<'a>(
        &mut self,
        destination: PreConfiguredDestination<'a>,
    ) -> Result<DestinationHash, ConfigurePreconfiguredDestinationError> {
        configure_preconfigured_destination::<St, R, S>(&mut self.node.engine, destination)
    }

    pub fn allow_requester(
        &mut self,
        destination: &DestinationHash,
        path: &str,
        identity: IdentityHash,
    ) -> Result<(), RequestHandlerError> {
        self.node
            .engine
            .allow_requester(destination, path, identity)
    }

    /// Must precede [`seed_routes_from_store`](Self::seed_routes_from_store) so restored rows sit in this boot's past.
    #[must_use]
    pub fn with_timeline_origin(mut self, origin: InstantMillis) -> Self {
        self.host = TokioHost::start_at(origin);
        self
    }

    #[must_use]
    pub fn with_accepted_announce_observer(
        mut self,
        observer: impl for<'a> FnMut(AnnounceObservation<'a>) + Send + 'static,
    ) -> Self {
        self.accepted_announce_observer = Some(Box::new(observer));
        self
    }

    #[must_use]
    pub fn clock(&self) -> TokioHost {
        self.host.clone()
    }

    pub fn seed_blackholed_identities<Reason: AsRef<str>>(
        &mut self,
        entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
    ) -> BlackholeSeedReport {
        let mut report = BlackholeSeedReport::default();
        let now = self.host.now();
        for entry in entries {
            if entry.expiry.is_expired_at(now) {
                report.refused_count += 1;
                continue;
            }
            let reason = entry.reason.as_ref().map(AsRef::as_ref);
            match self.node.engine.blackhole_identity(
                BlackholedIdentity {
                    identity: entry.identity,
                    source: entry.source,
                    expiry: entry.expiry,
                    reason,
                },
                &mut |_| {},
            ) {
                Ok(BlackholeIdentityOutcome::Added) => report.seeded_count += 1,
                Ok(BlackholeIdentityOutcome::AlreadyPresent) => report.dropped_count += 1,
                Err(error) => {
                    match <S::Blackholes as BlackholeTable>::classify_insert_error(error) {
                        BlackholeInsertFailure::CapacityExhausted
                        | BlackholeInsertFailure::ReasonTooLong => report.dropped_count += 1,
                    }
                }
            }
        }
        report
    }

    /// Boot-restore before [`run`](Self::run): every stored row re-verifies its signature and address binding before landing, and lands with the departed grace on its interface. Refusals and drops are counted, never fatal — a damaged snapshot costs rows, not the boot.
    pub fn seed_routes_from_store(&mut self, store: &impl PersistedStore) -> RouteSeedReport {
        self.seed_routes_from_store_reporting(store, |_| {})
    }

    pub fn seed_routes_from_store_reporting(
        &mut self,
        store: &impl PersistedStore,
        progress: impl FnMut(RouteSeedProgress),
    ) -> RouteSeedReport {
        let mut report = RouteSeedReport::default();
        let Ok(Some(stored_len)) = store.stored_len(SnapshotRegion::RoutingTable) else {
            return report;
        };
        let Some(mut buf) = try_zeroed_buffer(stored_len) else {
            report.refused_count = report.refused_count.saturating_add(1);
            return report;
        };
        let Ok(Some(bytes)) = store.load(SnapshotRegion::RoutingTable, &mut buf) else {
            return report;
        };
        let Ok(rows) = read_routing_table_snapshot(bytes) else {
            report.refused_count += 1;
            return report;
        };
        let now = self.host.now();
        let workers = self.crypto_pool.resolved_worker_count();
        super::route_restore::seed_persisted_routes(
            &mut self.node.engine,
            rows,
            now,
            workers,
            progress,
        )
        .into_report()
    }

    pub fn seed_destination_identities_from_store(
        &mut self,
        store: &impl PersistedStore,
    ) -> DestinationIdentitySeedReport {
        let mut report = DestinationIdentitySeedReport::default();
        let Ok(Some(stored_len)) = store.stored_len(SnapshotRegion::DestinationIdentities) else {
            return report;
        };
        let Some(mut buf) = try_zeroed_buffer(stored_len) else {
            report.refused_count = report.refused_count.saturating_add(1);
            return report;
        };
        let Ok(Some(bytes)) = store.load(SnapshotRegion::DestinationIdentities, &mut buf) else {
            return report;
        };
        let Ok(rows) = read_destination_identities_snapshot(bytes) else {
            report.refused_count += 1;
            return report;
        };
        let now = self.host.now();
        for row in rows {
            let Ok(row) = row else {
                report.refused_count += 1;
                break;
            };
            match self.node.engine.seed_destination_identity(row, now) {
                DestinationIdentitySeedOutcome::Seeded => report.seeded_count += 1,
                DestinationIdentitySeedOutcome::RefusedPublicKeyChanged => {
                    report.refused_count += 1;
                }
                DestinationIdentitySeedOutcome::Replaced
                | DestinationIdentitySeedOutcome::Expired
                | DestinationIdentitySeedOutcome::CapacityExhausted => report.dropped_count += 1,
            }
        }
        report
    }

    /// Boot-restore before [`run`](Self::run): each seeded tunnel warms its stored interface's routes until the peer's next synthesize, which repoints them onto the live connection — this is what re-claims routes whose interface id never comes back, an ephemeral client's reconnect being the canonical case. Refusals are counted, never fatal.
    pub fn seed_tunnels_from_store(&mut self, store: &impl PersistedStore) -> TunnelSeedReport {
        let mut report = TunnelSeedReport::default();
        let Ok(Some(stored_len)) = store.stored_len(SnapshotRegion::Tunnels) else {
            return report;
        };
        let Some(mut buf) = try_zeroed_buffer(stored_len) else {
            report.refused_count = report.refused_count.saturating_add(1);
            return report;
        };
        let Ok(Some(bytes)) = store.load(SnapshotRegion::Tunnels, &mut buf) else {
            return report;
        };
        let Ok(rows) = read_tunnels_snapshot(bytes) else {
            report.refused_count += 1;
            return report;
        };
        for row in rows {
            match self.node.engine.seed_tunnel(row) {
                SeedTunnelOutcome::Seeded => report.seeded_count += 1,
                SeedTunnelOutcome::AlreadyPresent | SeedTunnelOutcome::TableFull => {
                    report.dropped_count += 1;
                }
            }
        }
        report
    }

    /// Boot-restore before [`run`](Self::run): each ratcheted destination the recipe registered reloads its rotation clock and retained secrets from the vault, so singles peers encrypted toward pre-reboot ratchets decrypt again. Refusals are counted, never fatal.
    pub fn seed_self_ratchets_from_vault<V: IdentityVault>(
        &mut self,
        vault: &V,
    ) -> RatchetSeedReport {
        let mut report = RatchetSeedReport::default();
        let destinations: Vec<DestinationHash> = self
            .node
            .engine
            .persisted_self_ratchet_rows()
            .map(|(destination, _, _)| destination)
            .collect();
        for destination in destinations {
            let label = ratchet_label(&destination);
            let Ok(Some(stored_len)) = vault.stored_blob_len(&label) else {
                continue;
            };
            let Some(buf) = try_zeroed_buffer(stored_len) else {
                report.refused_count = report.refused_count.saturating_add(1);
                continue;
            };
            let mut buf = Zeroizing::new(buf);
            let Ok(Some(bytes)) = vault.load_blob(&label, &mut buf) else {
                continue;
            };
            let Ok(record) = read_self_ratchets_snapshot(bytes) else {
                report.refused_count += 1;
                continue;
            };
            match self.node.engine.seed_self_ratchets(
                &destination,
                record.last_rotated,
                record
                    .secrets_newest_first()
                    .collect::<Vec<_>>()
                    .into_iter(),
            ) {
                SeedSelfRatchetsOutcome::Seeded => report.seeded_count += 1,
                SeedSelfRatchetsOutcome::AlreadyMinted | SeedSelfRatchetsOutcome::Untracked => {
                    report.dropped_count += 1;
                }
            }
        }
        report
    }

    /// Override how this node runs its asymmetric crypto. Defaults to `CryptoPoolConfig::host_default` (pooled on capable hosts, inline on mobile).
    #[must_use]
    pub fn with_crypto_pool(mut self, crypto_pool: CryptoPoolConfig) -> Self {
        self.crypto_pool = crypto_pool;
        self
    }

    /// A `Send + Clone` handle for other tasks/threads to drive the node while [`run`](Self::run) owns the loop.
    #[must_use]
    pub fn handle(&self) -> PrnsNodeHandle {
        self.handle.clone()
    }

    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        self.handle.issue(command)
    }

    pub async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        self.handle.send_single_packet(destination, data).await
    }

    pub async fn establish_link(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        self.handle.establish_link(destination).await
    }

    pub fn respond(&self, responder: RespondToken, body: &[u8]) -> Option<RttMillis> {
        self.handle.respond(responder, body)
    }

    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.handle.close_link(link_id)
    }

    /// Drive the node until it stops (in practice forever). The reactor and the request runner run joined: every inbound request forks to the runner, while that event, and every other, reaches the recipe's `on_event` with shared `&state`, zero-copy.
    pub async fn run(self) {
        let PrnsNode {
            handle,
            host,
            node,
            notify_rx,
            command_rx,
            iface_build_rx,
            mut accepted_announce_observer,
            crypto_pool,
        } = self;
        let AssembledNode {
            engine,
            state,
            mut on_event,
            routes: _,
        } = node;
        let egress = Egress::new(std::vec::Vec::new());
        let store = handle.store.clone();
        let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let inflate_commands = handle.commands.clone();
        let inflate_workers = inflate_parallelism();
        let inflate_admission = Arc::new(tokio::sync::Semaphore::new(
            inflate_workers.saturating_mul(INFLATE_QUEUE_PER_WORKER),
        ));
        let inflate_execution = Arc::new(tokio::sync::Semaphore::new(inflate_workers));
        let reactor = reactor_driver::run_with_store(
            engine,
            host,
            reactor_driver::ReactorWiring {
                interfaces: std::vec::Vec::new(),
                ifacs: std::vec::Vec::new(),
                notify: notify_rx,
                inbound_lanes: std::vec::Vec::new(),
                commands: command_rx,
                egress,
            },
            |journaled| {
                if let Journaled::ResourceNeedsDecompression {
                    link_id,
                    hash,
                    stream,
                    uncompressed_data_len,
                } = &journaled
                {
                    let (link_id, hash, uncompressed_data_len) =
                        (*link_id, *hash, *uncompressed_data_len);
                    let stream = stream.to_vec();
                    let commands = inflate_commands.clone();
                    let admission = inflate_admission.clone().try_acquire_owned();
                    let execution = inflate_execution.clone();
                    let Ok(admission) = admission else {
                        let _ = commands.send(HostCommand::ProvideDecompressed(
                            ProvideDecompressedHostCommand {
                                link_id,
                                hash,
                                plaintext: std::vec::Vec::new().into(),
                            },
                        ));
                        return;
                    };
                    tokio::spawn(async move {
                        let Ok(execution) = execution.acquire_owned().await else {
                            return;
                        };
                        let plaintext = tokio::task::spawn_blocking(move || {
                            compression::decompress_bounded(
                                &stream,
                                resource_segment_decompression_bound(uncompressed_data_len),
                            )
                            .unwrap_or_default()
                        })
                        .await
                        .unwrap_or_default();
                        drop(execution);
                        drop(admission);
                        let _ = commands.send(HostCommand::ProvideDecompressed(
                            ProvideDecompressedHostCommand {
                                link_id,
                                hash,
                                plaintext: plaintext.into(),
                            },
                        ));
                    });
                    return;
                }
                notify_accepted_announce(&mut accepted_announce_observer, &journaled);
                let event = PrnsEvent::from(journaled);
                #[cfg(feature = "tracing")]
                super::tracing_events::emit(&event);
                if let PrnsEvent::Message(Message::Request {
                    link_id,
                    request_id,
                    path_hash,
                    requested_at,
                    rtt,
                    data,
                }) = &event
                {
                    let _ = req_tx.try_send(RunnerRequest {
                        link_id: *link_id,
                        request_id: *request_id,
                        path_hash: *path_hash,
                        requested_at: *requested_at,
                        rtt: *rtt,
                        data: data.to_vec(),
                    });
                }
                on_event(event, &state);
            },
            store,
            crypto_pool,
        );
        let driver_commands = handle.commands.clone();
        let driver_interfaces = handle.interfaces.clone();
        tokio::join!(
            reactor,
            run_router::<St, R>(&state, req_rx, handle),
            drive_interfaces(
                std::vec::Vec::new(),
                iface_build_rx,
                driver_commands,
                driver_interfaces
            ),
        );
    }
}

/// The boot origin for a wall-clocked host: wall time floored by the stored high-water, so a rolled-back clock can never restart the timeline under persisted rows. Absent or unreadable snapshots fall back gracefully — boot never blocks on storage health.
pub fn boot_timeline_origin(store: &impl PersistedStore) -> InstantMillis {
    let wall_now = wall_clock_timeline_origin().0;
    let mut buf = [0u8; TIMEBASE_SNAPSHOT_LEN];
    let high_water = match store.load(SnapshotRegion::Timebase, &mut buf) {
        Ok(Some(bytes)) => read_timebase_snapshot(bytes)
            .map(|high_water| high_water.0)
            .unwrap_or(0),
        _ => 0,
    };
    InstantMillis(wall_now.max(high_water))
}

fn wall_clock_timeline_origin() -> InstantMillis {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    InstantMillis(millis)
}

/// What a boot-restore pass did with the stored rows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RouteSeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RouteSeedProgress {
    pub processed_count: u32,
    pub total_count: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BlackholeSeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DestinationIdentitySeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

/// What a boot-restore pass did with the stored tunnels.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TunnelSeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

/// What a boot-restore pass did with the stored self-ratchet records.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RatchetSeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

/// The vault label addressing one destination's self-ratchet record.
#[allow(clippy::expect_used)]
fn ratchet_label(destination: &DestinationHash) -> IdentityLabel {
    let mut label = String::with_capacity("ratchets.".len() + destination.as_bytes().len() * 2);
    label.push_str("ratchets.");
    for byte in destination.as_bytes() {
        let _ = core::fmt::Write::write_fmt(&mut label, format_args!("{byte:02x}"));
    }
    IdentityLabel::new(&label).expect("a hex destination under a fixed prefix is label-lawful")
}

impl SelfRatchetsSnapshot {
    pub fn store_into<V: IdentityVault>(self, vault: &mut V) -> Result<u32, V::Error> {
        let mut flushed_count = 0u32;
        for (destination, sealed) in self.blobs {
            vault.store_blob(&ratchet_label(&destination), &sealed)?;
            flushed_count = flushed_count.saturating_add(1);
        }
        Ok(flushed_count)
    }
}

impl SelfRatchetSnapshot {
    pub fn store_into<V: IdentityVault>(self, vault: &mut V) -> Result<(), V::Error> {
        vault.store_blob(&ratchet_label(&self.destination), &self.sealed)
    }
}

pub struct PreparedFlush {
    snapshot: PersistedStateSnapshot,
}

impl PreparedFlush {
    #[allow(clippy::expect_used)]
    pub fn commit_to_store<P: PersistedStore>(
        self,
        store: &mut P,
        mark: &mut FlushMark,
    ) -> Result<FlushReport, FlushError<P::Error>> {
        let mut timebase = [0u8; TIMEBASE_SNAPSHOT_LEN];
        let timebase_len = write_timebase_snapshot(self.snapshot.taken_at, &mut timebase)
            .expect("TIMEBASE_SNAPSHOT_LEN sizes its own snapshot");
        store
            .store(SnapshotRegion::Timebase, &timebase[..timebase_len])
            .map_err(FlushError::Store)?;
        let routing_table = store_changed_region(
            store,
            SnapshotRegion::RoutingTable,
            &self.snapshot.routing_table,
            &mut mark.routing_table,
        )?;
        let tunnels = store_changed_region(
            store,
            SnapshotRegion::Tunnels,
            &self.snapshot.tunnels,
            &mut mark.tunnels,
        )?;
        let destination_identities = store_changed_region(
            store,
            SnapshotRegion::DestinationIdentities,
            &self.snapshot.destination_identities,
            &mut mark.destination_identities,
        )?;
        Ok(FlushReport {
            high_water: self.snapshot.taken_at,
            routing_table,
            tunnels,
            destination_identities,
        })
    }
}

fn store_changed_region<P: PersistedStore>(
    store: &mut P,
    region: SnapshotRegion,
    sealed: &[u8],
    last_landed: &mut Option<SnapshotFingerprint>,
) -> Result<RegionFlush, FlushError<P::Error>> {
    let fingerprint = snapshot_fingerprint(sealed);
    if fingerprint.is_some() && fingerprint == *last_landed {
        return Ok(RegionFlush::UnchangedSkipped);
    }
    store.store(region, sealed).map_err(FlushError::Store)?;
    *last_landed = fingerprint;
    Ok(RegionFlush::Wrote)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareFlushError {
    NodeStopped,
}

/// The fingerprints of the last flush this mark's owner landed, one per skippable region. A fresh mark knows nothing, so its first flush writes everything once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FlushMark {
    pub routing_table: Option<SnapshotFingerprint>,
    pub tunnels: Option<SnapshotFingerprint>,
    pub destination_identities: Option<SnapshotFingerprint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionFlush {
    Wrote,
    UnchangedSkipped,
}

/// What one changed-flush pass did: the high-water it stamped and each region's write-or-skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushReport {
    pub high_water: InstantMillis,
    pub routing_table: RegionFlush,
    pub tunnels: RegionFlush,
    pub destination_identities: RegionFlush,
}

#[derive(Debug)]
pub enum FlushError<E> {
    NodeStopped,
    Store(E),
}

#[cfg(test)]
mod tests;
