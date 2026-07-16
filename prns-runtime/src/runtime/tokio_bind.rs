use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::crypto::ratchets::SeedSelfRatchetsOutcome;
use crate::engine::{
    CloseLink, CommandId, Departure, EngineCommand, EngineState, EstablishLink,
    EstablishLinkFailure, IssuedCommand, KnownDestinationSeedOutcome, PacketReceiptDelivered,
    RouteSeedOutcome, SendRequestFailure, SendResourceFailure, SendSinglePacket,
    SendSinglePacketFailure, SendSinglePacketPayload, Settlement,
};
use crate::engine::{InspectionQuery, InspectionResult};
use crate::engine::{InstantMillis, Journaled};
use crate::identity::vault::{IdentityLabel, IdentityVault};
use crate::identity::Zeroizing;
use crate::inspection::{
    AnnounceRateSnapshot, InspectionSource, InterfaceIfacSnapshot, InterfaceInventoryEntry,
    RouteSnapshot,
};
use crate::interfaces::ifac::IfacContext;
use crate::interfaces::{
    ConnectionView, InterfaceId, InterfaceKind, InterfaceSnapshot, Membership, PacketPhyStats,
    ReportsStatus, StatusView,
};
use crate::persistence::{
    read_known_destinations_snapshot, read_routing_table_snapshot, read_self_ratchets_snapshot,
    read_timebase_snapshot, read_tunnels_snapshot, snapshot_fingerprint, write_timebase_snapshot,
    PersistedStore, SnapshotFingerprint, SnapshotRegion, TIMEBASE_SNAPSHOT_LEN,
};
use crate::reactor::impls::compression;
use crate::reactor::impls::tokio_reactor::{
    self, tokio_grant_lane, AddInterfaceCommand, CryptoPoolConfig, Egress, HostCommand,
    HostResourceMetadata, HostResourcePayload, PersistedStateSnapshot,
    ProvideDecompressedHostCommand, RequestAnyHostCommand, ResourceInbound, RespondAnyHostCommand,
    SelfRatchetsSnapshot, SendResourceSegmentHostCommand, TokioHost, TokioInterfaceSeam,
};
use crate::reactor::interface_seam::{frame_cap_for, Interface};
use crate::reactor::Host;
use crate::routing::blackhole::BlackholeTable;
use crate::routing::dedup::PacketHash;
use crate::routing::links::data::LINK_MDU;
use crate::routing::links::request::{RequestId, RESPONSE_WIRE_OVERHEAD};
use crate::routing::links::resources::{
    ResourceHash, ResourceStrategy, MAX_EFFICIENT_SIZE, METADATA_PREFIX_LEN,
};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::routing::tunnel::SeedTunnelOutcome;
use crate::routing::{BlackholeIdentityOutcome, BlackholeInsertFailure, BlackholedIdentity};
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, TransportId};

use super::byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};
use super::identity_blackhole::{settle_control, settle_source};
use super::recipe::{Manual, PreConfiguredDestination};
use super::request_router::{RespondToken, RouteSet};
use super::settle_known_destination_retention;
use super::tokio_runner::{run_router, RunnerRequest, REQUEST_QUEUE_DEPTH};
#[cfg(feature = "runtime-metrics")]
use super::RuntimeMetricsSnapshot;
use super::{
    ClearAnnounceQueuesOutcome, DropRouteOutcome, DropRoutesViaOutcome, IdentityBlackholeControl,
    IdentityBlackholeControlError, IdentityBlackholeHostCommand, IdentityBlackholeSource,
    IdentityBlackholeSourceError, InterfaceStore, KnownDestinationRetentionControl,
    KnownDestinationRetentionControlError, KnownDestinationRetentionHostCommand, Message,
    PrnsEvent, PrnsRecipe, RoutingControl, RoutingControlError, SendError,
};

/// How many frames a host lane holds in flight. RNS resource transfer bursts a whole window of
/// parts at once (`Resource.WINDOW_MAX_FAST` is 75, plus its flexibility), so a lane carrying a
/// transfer must be deeper than that window or it sheds parts and the transfer stalls; the old
/// byte-budget collapsed a fat-MTU lane to a handful of slots, exactly that failure. Growable
/// slots (`HeapFrameSlot`) cost only the frames actually in flight, so the depth is generous.
const HOST_LANE_DEPTH: usize = 256;

fn lane_depth_for(_slot_cap: usize) -> usize {
    HOST_LANE_DEPTH
}

/// The largest response that still fits a single link packet on the base-MDU link RNS 1.3.5
/// negotiates from. At or below it a response is always a packet, never a resource, so building a
/// bz2 candidate would only waste the codec on an answer that never carries it. Above it the answer
/// may ride a resource (the reactor decides against the live MDU), so it is worth compressing; the
/// floor sits under every live packet ceiling, so no resource-bound response goes uncompressed.
const RESPONSE_PACKET_CEILING: usize = LINK_MDU - RESPONSE_WIRE_OVERHEAD;

/// A cloneable, `Send` handle to a running node: the proactive surface. Every [`CommandId`] is
/// minted from one counter, so a fire-and-forget [`issue`](Self::issue) can never collide with
/// an awaited [`send_single_packet`](Self::send_single_packet) or a runner's respond.
#[derive(Clone)]
pub struct TokioPrnsHandle {
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

impl RuntimeIfac {
    fn snapshot(&self) -> InterfaceIfacSnapshot {
        InterfaceIfacSnapshot {
            signature: self.context.ifac_signature(),
            size: self.context.ifac_size(),
            network_name: self.network_name.clone(),
        }
    }
}

/// Why a [`send_resource`](TokioPrnsHandle::send_resource) stream did not complete.
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

/// What a completed [`receive_resource`](TokioPrnsHandle::receive_resource) yields: the assembled
/// resource's identity, total size, and any metadata that traveled. The bytes themselves were
/// streamed to the caller's sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceReceipt {
    pub original_hash: ResourceHash,
    pub total_size: u64,
    /// The transfer's packed metadata (msgpack the app unpacks), when one traveled.
    pub metadata: Option<std::vec::Vec<u8>>,
}

/// Why a [`receive_resource`](TokioPrnsHandle::receive_resource) stream did not complete.
#[derive(Debug)]
pub enum ResourceReceiveError {
    /// Writing to `sink` failed before the whole resource arrived.
    Sink(std::io::Error),
    /// The transfer failed at the receiver — a bad segment, a vanished link, or a refused offer.
    Failed,
    /// The node's reactor has stopped.
    NodeStopped,
}

impl TokioPrnsHandle {
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

    /// Make a request of `path_hash` with `data` of any length and await the response. The
    /// runtime picks the rung (a single REQUEST packet within the link MDU, or a resource that
    /// rides past it), so a consumer never meets a size limit; the answer carries the measured round trip.
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

    /// Stream a resource of `total_len` bytes to a peer over an active link, draining `source`
    /// one segment at a time: the engine holds the transferring segment plus the next one staged (sealed early, advertised at the proof), and the host prepares one more behind those, so at most three segments are ever held, never the whole payload.
    /// The length is explicit because every segment advertises the total up front; a payload at or under one segment crosses unsplit.
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

    /// [`send_resource`](Self::send_resource) with the compression posture explicit, the RNS
    /// 1.3.5 `auto_compress` parameter: [`SegmentCompression::Never`] ships every segment
    /// uncompressed where the default attempts bz2 per segment.
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

    /// [`send_resource`](Self::send_resource) with RNS 1.3.5 resource metadata: `packed_metadata`
    /// is msgpack the peer's app unpacks, opaque all the way down. The block rides ahead of the
    /// data in segment one's stream and inside the advertised total, so segment one carries that
    /// much less data and a payload near the segment boundary may split one segment sooner.
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

    /// Receive the next inbound resource on `link_id`, streaming it into `sink`: the mirror of
    /// [`send_resource`](Self::send_resource). Registers the sink before yielding, so a segment
    /// arriving the instant after cannot reach the app event stream instead; resolves with the assembled identity and size.
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

    /// Set the default resource strategy for one of this node's destinations, returning whether
    /// the node holds it. The recipe's `resource_strategy` sets this at construction; this is
    /// the runtime counterpart, for a destination re-tuned while the node runs.
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

    /// A consistent image of every persisted region, serialized on the reactor — the one place a
    /// consistent view exists — with the engine instant it was taken at. `None` once the node has stopped.
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

    /// One full flush, unconditionally rewriting every region — right for a shutdown handler; an
    /// interval loop wants [`flush_changed_to_store`](Self::flush_changed_to_store).
    pub async fn flush_to_store<P: PersistedStore>(
        &self,
        store: &mut P,
    ) -> Result<InstantMillis, FlushError<P::Error>> {
        self.flush_changed_to_store(store, &mut FlushMark::default())
            .await
            .map(|report| report.high_water)
    }

    /// The interval-cadence flush: a region whose sealed image fingerprints the same as `mark`'s
    /// last landed flush is skipped, so a quiet node's tick writes 12 bytes of timebase and
    /// nothing else. The timebase always writes — it is the restored timeline's rollback floor
    /// and it advances with uptime even while the tables sit still — and it lands before the
    /// region images it stamps: a crash between them leaves a newer high-water over older rows,
    /// which only over-ages the restored timeline, where the reverse order could strand rows in
    /// a wall-less boot's future, never expiring.
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
        let snapshot = self
            .snapshot_persisted_state()
            .await
            .ok_or(FlushError::NodeStopped)?;
        let mut timebase = [0u8; TIMEBASE_SNAPSHOT_LEN];
        let timebase_len = write_timebase_snapshot(snapshot.taken_at, &mut timebase)
            .expect("TIMEBASE_SNAPSHOT_LEN sizes its own snapshot");
        store
            .store(SnapshotRegion::Timebase, &timebase[..timebase_len])
            .map_err(FlushError::Store)?;
        let routing_table = Self::store_changed_region(
            store,
            SnapshotRegion::RoutingTable,
            &snapshot.routing_table,
            &mut mark.routing_table,
        )?;
        let tunnels = Self::store_changed_region(
            store,
            SnapshotRegion::Tunnels,
            &snapshot.tunnels,
            &mut mark.tunnels,
        )?;
        let known_destinations = Self::store_changed_region(
            store,
            SnapshotRegion::KnownDestinations,
            &snapshot.known_destinations,
            &mut mark.known_destinations,
        )?;
        Ok(FlushReport {
            high_water: snapshot.taken_at,
            routing_table,
            tunnels,
            known_destinations,
        })
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

    /// Every tracked destination's sealed self-ratchet record, serialized on the reactor.
    /// `None` once the node has stopped.
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

    /// Flush every destination's self-ratchet record to `vault`, returning how many landed.
    /// Ratchet secrets never touch a [`PersistedStore`]: the vault is where the identity
    /// secret itself lives, so the record inherits its protections.
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
        let mut flushed_count = 0u32;
        for (destination, sealed) in &snapshot.blobs {
            vault
                .store_blob(&ratchet_label(destination), sealed)
                .map_err(FlushError::Store)?;
            flushed_count += 1;
        }
        Ok(flushed_count)
    }

    /// Bring a link up to `destination` and await it: `Ok(LinkId)` once the peer's proof
    /// validates, or the typed reason it never established. The resolved id is the handle every
    /// link-scoped verb takes.
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

    /// Answer a request via its token, returning the link's round trip (the request arrived over
    /// it) — or `None` if the node has stopped before the answer could be queued.
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

    /// Open a byte-stream reader on this link and stream id. Awaits the run loop's
    /// acknowledgement that the sink is live before yielding the reader, so a chunk arriving
    /// the instant the link opens is buffered for the reader, never forwarded past it to the app.
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

    /// Open a byte-stream writer on this link and stream id: an `AsyncWrite` framing each write as a
    /// stream-data channel send.
    pub fn byte_stream_writer(&self, link_id: LinkId, stream_id: StreamId) -> ByteStreamWriter {
        ByteStreamWriter::new(self.clone(), link_id, stream_id)
    }

    /// Open a bidirectional byte stream: a reader on `rx` and a writer on `tx` over one link's
    /// channel, RNS's `create_bidirectional_buffer`. Awaits the reader's registration (see
    /// [`byte_stream_reader`](Self::byte_stream_reader)) so the read half is live before either is handed back.
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

    /// Attach an interface to the running node and get a handle to tear it back down. Grab any
    /// per-interface control handle (`.status()`, a radio's own controls) before calling this,
    /// since it takes the interface by value.
    ///
    /// `I: Send` is the host's bargain: the interface rides to the `run` task inside a `Send`
    /// builder closure which mints its run future there, so the future itself never has to be
    /// `Send` (what keeps `!Send` interface bodies legal) and the reactor stays `Send` and spawnable.
    pub fn add_interface<I>(&self, interface: I) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_access(interface, None)
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
        self.add_interface_access(
            interface,
            Some(RuntimeIfac {
                context: ifac,
                network_name,
            }),
        )
    }

    fn add_interface_access<I>(&self, interface: I, ifac: Option<RuntimeIfac>) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        let view = interface.status_view();
        let connection = interface.connection_view();
        let attached = attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            None,
            connection,
            ifac.as_ref().map(|access| access.context.clone()),
        );
        register_status(
            &self.interfaces,
            attached.id(),
            view,
            Membership::Independent,
            ifac.as_ref().map(RuntimeIfac::snapshot),
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
                let membership = registered.membership;
                let ifac = registered.ifac.clone();
                let configured_name = registered.configured_name.clone();
                (registered.view)().into_iter().map(move |vitals| {
                    let counts = self.store.counts(vitals.id);
                    InterfaceInventoryEntry {
                        configured_name: configured_name.clone(),
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
                            membership,
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
        interface.configured_name = Some(name.into());
        true
    }

    /// Every interface attached through this handle, as a complete [`InterfaceSnapshot`]: live
    /// vitals read at call time joined with the engine counts and fleet position. The raw fleet
    /// an inspection face can project for its own presentation, with no app-side bookkeeping.
    #[must_use]
    pub fn interfaces(&self) -> std::vec::Vec<InterfaceSnapshot> {
        self.interface_inventory()
            .into_iter()
            .map(|entry| entry.snapshot)
            .collect()
    }

    /// Attach an interface supervisor: a node that owns no wire of its own but stands up a
    /// fleet member per validated connection through the [`Fleet`] handle it is given. The
    /// supervisor is no engine interface (no descriptor, no lanes); each member is an ordinary
    /// flat interface recorded under it, so teardown cascades to the whole fleet.
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
        register_status(
            &self.interfaces,
            id,
            view,
            Membership::Independent,
            ifac_status,
        );
        AttachedSupervisor {
            id,
            iface_build: self.iface_build.clone(),
        }
    }

    /// Detach the interface with this id (the inverse of [`add_interface`](Self::add_interface)):
    /// deregister its lanes on the reactor and stop its run future on the driver. For a supervisor,
    /// the driver cascades the stop to every member of its fleet.
    /// The routes learned through it stay warm for the departure grace, so a same-identity
    /// re-attach (a radio toggled off and on, a retune switched back) restores them;
    /// [`forget_interface`](Self::forget_interface) is the detach that drops them at once.
    pub fn remove_interface(&self, id: InterfaceId) {
        let _ = self.commands.send(HostCommand::RemoveInterface {
            id,
            departure: Departure::MayReturn,
        });
        let _ = self.iface_build.send(DriverMsg::Stop { id });
    }

    /// Detach like [`remove_interface`](Self::remove_interface) and drop the routes learned
    /// through the interface at once, instead of holding them warm for a return.
    pub fn forget_interface(&self, id: InterfaceId) {
        let _ = self.commands.send(HostCommand::RemoveInterface {
            id,
            departure: Departure::Forgotten,
        });
        let _ = self.iface_build.send(DriverMsg::Stop { id });
    }

    /// Attach anything from the interface menu and get back its kind's attachment handle —
    /// the one verb over [`add_interface`](Self::add_interface) and [`supervise`](Self::supervise).
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

/// One registration story per menu type: the type itself encodes whether it joins as a single
/// wire (`add_interface`) or a discovery fleet (`supervise`), so no callsite has to know.
pub trait Attachable {
    type Attached;
    fn attach_to(self, handle: &TokioPrnsHandle) -> Self::Attached;
    fn attach_to_with_ifac(
        self,
        handle: &TokioPrnsHandle,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> Self::Attached;
}

/// The recipe's `interfaces` answer: [`Manual`] says the app attaches through the handle
/// itself, a closure over the handle is the inline shopping list, prefabs compose the common cases.
pub trait AttachIntent {
    fn attach(self, handle: &TokioPrnsHandle);
}

impl AttachIntent for Manual {
    fn attach(self, _handle: &TokioPrnsHandle) {}
}

impl<F: FnOnce(&TokioPrnsHandle)> AttachIntent for F {
    fn attach(self, handle: &TokioPrnsHandle) {
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

impl RoutingControl for TokioPrnsHandle {
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

impl KnownDestinationRetentionControl for TokioPrnsHandle {
    fn mark_destination_used(
        &self,
        destination: DestinationHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::MarkDestinationUsedOutcome,
            KnownDestinationRetentionControlError,
        >,
    > + Send {
        settle_known_destination_retention(self.commands.clone(), move |reply| {
            KnownDestinationRetentionHostCommand::MarkUsed { destination, reply }
        })
    }

    fn retain_destination(
        &self,
        destination: DestinationHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::RetainDestinationOutcome,
            KnownDestinationRetentionControlError,
        >,
    > + Send {
        settle_known_destination_retention(self.commands.clone(), move |reply| {
            KnownDestinationRetentionHostCommand::RetainDestination { destination, reply }
        })
    }

    fn release_destination(
        &self,
        destination: DestinationHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::ReleaseDestinationOutcome,
            KnownDestinationRetentionControlError,
        >,
    > + Send {
        settle_known_destination_retention(self.commands.clone(), move |reply| {
            KnownDestinationRetentionHostCommand::ReleaseDestination { destination, reply }
        })
    }

    fn retain_identity(
        &self,
        identity: crate::identity::IdentityHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::RetainIdentityOutcome,
            KnownDestinationRetentionControlError,
        >,
    > + Send {
        settle_known_destination_retention(self.commands.clone(), move |reply| {
            KnownDestinationRetentionHostCommand::RetainIdentity { identity, reply }
        })
    }
}

impl IdentityBlackholeSource for TokioPrnsHandle {
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

impl IdentityBlackholeControl for TokioPrnsHandle {
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

impl InspectionSource for TokioPrnsHandle {
    fn interface_inventory(&self) -> std::vec::Vec<InterfaceInventoryEntry> {
        TokioPrnsHandle::interface_inventory(self)
    }

    async fn link_count(&self) -> u32 {
        match self
            .settle(EngineCommand::Inspect(InspectionQuery::LinkCount))
            .await
        {
            Some(Settlement::Inspection(InspectionResult::LinkCount(count))) => count,
            Some(_) | None => 0,
        }
    }

    fn packet_phy(&self, packet_hash: PacketHash) -> Option<PacketPhyStats> {
        self.store.packet_phy(packet_hash)
    }

    async fn announce_rates(&self) -> std::vec::Vec<AnnounceRateSnapshot> {
        match self
            .settle(EngineCommand::Inspect(InspectionQuery::AnnounceRates))
            .await
        {
            Some(Settlement::Inspection(InspectionResult::AnnounceRates(rows))) => rows,
            Some(_) | None => std::vec::Vec::new(),
        }
    }

    async fn routes(&self) -> std::vec::Vec<RouteSnapshot> {
        match self
            .settle(EngineCommand::Inspect(InspectionQuery::Routes))
            .await
        {
            Some(Settlement::Inspection(InspectionResult::Routes(rows))) => rows,
            Some(_) | None => std::vec::Vec::new(),
        }
    }

    async fn route(&self, destination: DestinationHash) -> Option<RouteSnapshot> {
        match self
            .settle(EngineCommand::Inspect(InspectionQuery::Route(destination)))
            .await
        {
            Some(Settlement::Inspection(InspectionResult::Route(entry))) => entry,
            Some(_) | None => None,
        }
    }
}

impl super::PrnsApi for TokioPrnsHandle {
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

/// A handle to one interface attached at runtime: its minted id and the lever to detach it.
/// Dropping the handle leaves the interface running; only [`teardown`](Self::teardown) (or [`TokioPrnsHandle::remove_interface`]) takes it down.
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

    /// Detach the interface: deregister its lanes on the reactor and stop its run future.
    /// Its routes stay warm for the departure grace, so a same-identity re-attach restores them.
    pub fn teardown(self) {
        let _ = self.commands.send(HostCommand::RemoveInterface {
            id: self.id,
            departure: Departure::MayReturn,
        });
        let _ = self.iface_build.send(DriverMsg::Stop { id: self.id });
    }
}

/// A handle to a supervisor attached through [`TokioPrnsHandle::supervise`]. Teardown is a single
/// stop on the driver, ending its discovery loop and cascading to its whole fleet; dropping the handle leaves it running.
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

/// Wire one interface onto the running node: build its grant lanes + seam, hand the reactor the
/// `Send` lane halves, and hand the driver the `Send` builder that mints its run future.
/// `supervisor` records it as a fleet member so the driver cascades teardown.
fn attach_interface<I>(
    commands: &UnboundedSender<HostCommand>,
    iface_build: &UnboundedSender<DriverMsg>,
    notify_tx: &UnboundedSender<InterfaceId>,
    interface: I,
    supervisor: Option<InterfaceId>,
    connection: Option<ConnectionView>,
    ifac: Option<IfacContext>,
) -> AttachedInterface
where
    I: Interface + Send + 'static,
{
    let descriptor = interface.descriptor();
    let id = descriptor.id;
    let logical_interface = supervisor.unwrap_or(id);
    let slot_cap = frame_cap_for(&descriptor);
    let depth = lane_depth_for(slot_cap);
    let (in_producer, in_consumer) = tokio_grant_lane(slot_cap, depth);
    let (out_producer, out_consumer) = tokio_grant_lane(slot_cap, depth);
    let seam = TokioInterfaceSeam::new(id, in_producer, notify_tx.clone(), out_consumer)
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

/// A supervisor's lever to stand up fleet members. Each [`add`](Self::add) registers a flat
/// engine interface recorded as this supervisor's member; the supervisor typically holds the returned [`AttachedInterface`] to detach that member when its link drops.
pub struct Fleet {
    supervisor_id: InterfaceId,
    commands: UnboundedSender<HostCommand>,
    iface_build: UnboundedSender<DriverMsg>,
    notify_tx: UnboundedSender<InterfaceId>,
    interfaces: Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    ifac: Option<RuntimeIfac>,
}

impl Fleet {
    /// Stand up a fleet member under this supervisor — identical to [`TokioPrnsHandle::add_interface`]
    /// except the member is recorded as this supervisor's, so a supervisor teardown takes it with it.
    pub fn add<I>(&self, interface: I) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        let view = interface.status_view();
        let connection = interface.connection_view();
        let attached = attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            Some(self.supervisor_id),
            connection,
            self.ifac.as_ref().map(|access| access.context.clone()),
        );
        register_status(
            &self.interfaces,
            attached.id(),
            view,
            Membership::FleetMember {
                supervisor_id: self.supervisor_id,
            },
            self.ifac.as_ref().map(RuntimeIfac::snapshot),
        );
        attached
    }

    /// A [`Fleet`] wired to no reactor: member builds and host commands flow into the returned
    /// [`DetachedFleet`] tail and go nowhere. For driving a supervisor by hand (unit tests, a bench harness).
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

/// The unplugged end of [`Fleet::detached`]: holds the channel tails so the fleet's sends stay
/// deliverable while a hand-driven harness runs. Drop it and sends start failing, like a runtime whose reactor exited.
pub struct DetachedFleet {
    _commands: UnboundedReceiver<HostCommand>,
    _iface_build: UnboundedReceiver<DriverMsg>,
    _notify: UnboundedReceiver<InterfaceId>,
}

/// An interface supervisor: a node that owns no wire of its own but runs a discovery loop and
/// stands up a fleet member per validated connection. Attached with [`TokioPrnsHandle::supervise`].
#[allow(async_fn_in_trait)]
pub trait InterfaceSupervisor {
    /// The medium this supervisor stands for — the namespace root of its id.
    const KIND: InterfaceKind;

    /// The bytes that uniquely tag this supervisor, typically config-derived (the group it
    /// serves); the same rules as [`channel_tag`](crate::reactor::interface_seam::Interface::channel_tag) apply.
    fn channel_tag(&self) -> &[u8];

    async fn run(self, fleet: Fleet);
}

/// A message to the interface driver: a new interface to start driving, or a request to stop one.
/// The driver lives on the `!Send` `run` task, so an interface's `!Send` run future never has to
/// cross a thread — only the `Send` builder closure does.
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

/// Drive every interface run future — the recipe's initial set, plus any added through the handle
/// at runtime — on the `run` task. Each runtime-added interface is wrapped with a stop signal so
/// [`TokioPrnsHandle::remove_interface`] can drop it mid-flight; the initial set runs for the node's life.
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
            // An interface whose run future ended on its own (a dropped connection, no
            // reconnect) deregisters itself: its descriptor must not outlive its wire. A future
            // ended by a `Stop` already had its id pulled from `stops`, so the `stops.remove`
            // here is what distinguishes a natural completion from a deliberate one.
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

/// A status view the runtime tracks centrally, tagged with where its interface sits in the fleet.
/// `interfaces()` joins each with the engine's count store to mint an `InterfaceSnapshot`.
struct RegisteredInterface {
    view: StatusView,
    membership: Membership,
    ifac: Option<InterfaceIfacSnapshot>,
    configured_name: Option<String>,
}

fn register_status(
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    id: InterfaceId,
    view: Option<StatusView>,
    membership: Membership,
    ifac: Option<InterfaceIfacSnapshot>,
) {
    if let (Some(view), Ok(mut map)) = (view, interfaces.lock()) {
        map.insert(
            id,
            RegisteredInterface {
                view,
                membership,
                ifac,
                configured_name: None,
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
/// A node on the tokio host. Built from a [`PrnsRecipe`] with [`new`](Self::new) (synchronous:
/// it wires the engine and spawns each interface), then driven by [`run`](Self::run). Hold
/// [`handle`](Self::handle) clones to drive it from other tasks/threads while `run` owns the loop.
pub struct Prns<St, R, F, S: StorageLayout> {
    handle: TokioPrnsHandle,
    host: TokioHost,
    engine: EngineState<S>,
    notify_rx: UnboundedReceiver<InterfaceId>,
    command_rx: UnboundedReceiver<HostCommand>,
    iface_build_rx: UnboundedReceiver<DriverMsg>,
    state: St,
    on_event: F,
    crypto_pool: CryptoPoolConfig,
    _routes: PhantomData<R>,
}

impl<St, R, F, S: StorageLayout> Prns<St, R, F, S>
where
    R: RouteSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
{
    /// Stand a node up from `recipe` on the storage layout it names: assemble the engine
    /// (transport role, destinations, the routes' request handlers), then let the recipe's
    /// `interfaces` intent attach the node's edges through its own handle. Only [`run`](Self::run) awaits.
    #[allow(clippy::expect_used)]
    pub fn new<'a, D, I>(recipe: PrnsRecipe<D, St, R, F, I, S>) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'a>>,
        I: AttachIntent,
    {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (iface_build_tx, iface_build_rx) = mpsc::unbounded_channel();

        let mut engine = EngineState::<S>::default();
        for destination in recipe.pre_configured_destinations {
            match destination {
                PreConfiguredDestination::Plain { app_name, aspects } => {
                    engine
                        .register_plain_destination(app_name, aspects)
                        .expect("recipe plain destination is valid");
                }
                PreConfiguredDestination::Single {
                    app_name,
                    aspects,
                    identity,
                    announce_app_data: app_data,
                    proof,
                    link_requests,
                    ratchet,
                    resource_strategy,
                } => {
                    let held = engine
                        .hold_identity(identity)
                        .expect("recipe identity fits the store");
                    let dest = engine
                        .register_single_destination(
                            &held,
                            app_name,
                            aspects,
                            app_data,
                            proof,
                            link_requests,
                            ratchet,
                        )
                        .expect("recipe single destination is valid");
                    engine.set_default_resource_strategy(&dest, resource_strategy);
                    for (path, policy) in R::REGISTRATIONS {
                        engine
                            .register_request_handler(&dest, path, policy.engine_policy())
                            .expect("recipe request handler fits the store");
                        for seed in policy.seed_list() {
                            engine
                                .allow_requester(&dest, path, *seed)
                                .expect("recipe seed identity admits to its own fresh handler");
                        }
                    }
                }
            }
        }

        if let Some(secret) = recipe.transport_identity {
            let identity = engine
                .hold_identity(secret)
                .expect("the transport identity fits the held-identity store");
            engine
                .set_transport_identity(&identity)
                .expect("the transport identity was just held");
        }

        let handle = TokioPrnsHandle {
            commands: command_tx,
            ids: Arc::new(AtomicU64::new(0)),
            notify_tx,
            iface_build: iface_build_tx,
            interfaces: Arc::new(Mutex::new(HashMap::new())),
            store: InterfaceStore::new(),
        };
        recipe.interfaces.attach(&handle);

        Prns {
            handle,
            host: TokioHost::start_at(wall_clock_timeline_origin()),
            engine,
            notify_rx,
            command_rx,
            iface_build_rx,
            state: recipe.app_state,
            on_event: recipe.on_event,
            crypto_pool: CryptoPoolConfig::host_default(),
            _routes: PhantomData,
        }
    }

    /// Must precede [`seed_routes_from_store`](Self::seed_routes_from_store) so restored rows sit in this boot's past.
    #[must_use]
    pub fn with_timeline_origin(mut self, origin: InstantMillis) -> Self {
        self.host = TokioHost::start_at(origin);
        self
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
            match self.engine.blackhole_identity(
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

    /// Boot-restore before [`run`](Self::run): every stored row re-verifies its signature and
    /// address binding before landing, and lands with the departed grace on its interface.
    /// Refusals and drops are counted, never fatal — a damaged snapshot costs rows, not the boot.
    pub fn seed_routes_from_store(&mut self, store: &impl PersistedStore) -> RouteSeedReport {
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
        for row in rows {
            let Ok(row) = row else {
                report.refused_count += 1;
                break;
            };
            match self.engine.seed_route(&row, now) {
                RouteSeedOutcome::Seeded => report.seeded_count += 1,
                RouteSeedOutcome::RefusedDestinationMismatch
                | RouteSeedOutcome::RefusedBlackholedIdentity
                | RouteSeedOutcome::RefusedInvalidSignature => report.refused_count += 1,
                RouteSeedOutcome::AlreadyPresent
                | RouteSeedOutcome::TableFull
                | RouteSeedOutcome::AppDataArenaFull => report.dropped_count += 1,
            }
        }
        report
    }

    pub fn seed_known_destinations_from_store(
        &mut self,
        store: &impl PersistedStore,
    ) -> KnownDestinationSeedReport {
        let mut report = KnownDestinationSeedReport::default();
        let Ok(Some(stored_len)) = store.stored_len(SnapshotRegion::KnownDestinations) else {
            return report;
        };
        let Some(mut buf) = try_zeroed_buffer(stored_len) else {
            report.refused_count = report.refused_count.saturating_add(1);
            return report;
        };
        let Ok(Some(bytes)) = store.load(SnapshotRegion::KnownDestinations, &mut buf) else {
            return report;
        };
        let Ok(rows) = read_known_destinations_snapshot(bytes) else {
            report.refused_count += 1;
            return report;
        };
        let now = self.host.now();
        for row in rows {
            let Ok(row) = row else {
                report.refused_count += 1;
                break;
            };
            match self.engine.seed_known_destination(row, now) {
                KnownDestinationSeedOutcome::Seeded => report.seeded_count += 1,
                KnownDestinationSeedOutcome::RefusedPublicKeyChanged => {
                    report.refused_count += 1;
                }
                KnownDestinationSeedOutcome::Replaced
                | KnownDestinationSeedOutcome::Expired
                | KnownDestinationSeedOutcome::CapacityExhausted => report.dropped_count += 1,
            }
        }
        report
    }

    /// Boot-restore before [`run`](Self::run): each seeded tunnel warms its stored interface's
    /// routes until the peer's next synthesize, which repoints them onto the live connection —
    /// this is what re-claims routes whose interface id never comes back, an ephemeral client's
    /// reconnect being the canonical case. Refusals are counted, never fatal.
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
            match self.engine.seed_tunnel(row) {
                SeedTunnelOutcome::Seeded => report.seeded_count += 1,
                SeedTunnelOutcome::AlreadyPresent | SeedTunnelOutcome::TableFull => {
                    report.dropped_count += 1;
                }
            }
        }
        report
    }

    /// Boot-restore before [`run`](Self::run): each ratcheted destination the recipe registered
    /// reloads its rotation clock and retained secrets from the vault, so singles peers
    /// encrypted toward pre-reboot ratchets decrypt again. Refusals are counted, never fatal.
    pub fn seed_self_ratchets_from_vault<V: IdentityVault>(
        &mut self,
        vault: &V,
    ) -> RatchetSeedReport {
        let mut report = RatchetSeedReport::default();
        let destinations: Vec<DestinationHash> = self
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
            match self.engine.seed_self_ratchets(
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

    /// Override how this node runs its asymmetric crypto. Defaults to
    /// `CryptoPoolConfig::host_default` (pooled on capable hosts, inline on mobile).
    #[must_use]
    pub fn with_crypto_pool(mut self, crypto_pool: CryptoPoolConfig) -> Self {
        self.crypto_pool = crypto_pool;
        self
    }

    /// A `Send + Clone` handle for other tasks/threads to drive the node while [`run`](Self::run) owns the loop.
    #[must_use]
    pub fn handle(&self) -> TokioPrnsHandle {
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

    /// Drive the node until it stops (in practice forever). The reactor and the request runner
    /// run joined: every inbound request forks to the runner, while that event, and every
    /// other, reaches the recipe's `on_event` with shared `&state`, zero-copy.
    pub async fn run(self) {
        let Prns {
            handle,
            host,
            engine,
            notify_rx,
            command_rx,
            iface_build_rx,
            state,
            mut on_event,
            crypto_pool,
            _routes,
        } = self;
        let egress = Egress::new(std::vec::Vec::new());
        let store = handle.store.clone();
        let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let inflate_commands = handle.commands.clone();
        let inflate_workers = inflate_parallelism();
        let inflate_admission = Arc::new(tokio::sync::Semaphore::new(
            inflate_workers.saturating_mul(INFLATE_QUEUE_PER_WORKER),
        ));
        let inflate_execution = Arc::new(tokio::sync::Semaphore::new(inflate_workers));
        let reactor = tokio_reactor::run_with_store(
            engine,
            host,
            tokio_reactor::ReactorWiring {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN;
    use crate::interfaces::ifac::IfacSize;
    use crate::interfaces::{InterfaceStatus, InterfaceVitals};
    use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;

    const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

    fn handle() -> (TokioPrnsHandle, UnboundedReceiver<HostCommand>) {
        let (commands, command_rx) = mpsc::unbounded_channel();
        (TokioPrnsHandle::over(commands), command_rx)
    }

    #[test]
    fn an_oversized_persisted_length_is_rejected_before_allocation() {
        assert!(try_zeroed_buffer(MAX_BOOT_RECORD_LEN + 1).is_none());
        assert!(try_zeroed_buffer(usize::MAX).is_none());
    }

    #[test]
    fn inspection_reads_the_runtime_packet_phy_store() {
        let (handle, _command_rx) = handle();
        let packet_hash = PacketHash::new([0x42; 32]);
        let packet_phy = PacketPhyStats {
            rssi: Some(crate::interfaces::RssiDbm::new(-82)),
            snr: None,
            quality: None,
        };
        handle.store.remember_packet_phy(packet_hash, packet_phy);

        assert_eq!(
            InspectionSource::packet_phy(&handle, packet_hash),
            Some(packet_phy)
        );
    }

    #[test]
    fn the_standard_timeline_origin_is_unix_epoch_aligned() {
        let wall_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let origin = wall_clock_timeline_origin();

        assert!(wall_now.abs_diff(u128::from(origin.0)) < 1_000);
    }

    #[test]
    fn boot_blackholes_seed_against_the_resumed_timeline() {
        let mut node = Prns::new(PrnsRecipe {
            transport_identity: None,
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: (),
            storage: crate::storage::GrowableHeap,
            routes: crate::routes![],
            interfaces: Manual,
            on_event: |_event, _state: &()| {},
        })
        .with_timeline_origin(InstantMillis(1_000));
        let identity = crate::identity::IdentityHash::new([0x31; 16]);
        let source = crate::identity::IdentityHash::new([0x41; 16]);

        let report = node.seed_blackholed_identities([
            BlackholedIdentity {
                identity,
                source,
                expiry: crate::routing::BlackholeExpiry::At(InstantMillis(2_000)),
                reason: Some("active"),
            },
            BlackholedIdentity {
                identity,
                source,
                expiry: crate::routing::BlackholeExpiry::Indefinite,
                reason: Some("duplicate"),
            },
            BlackholedIdentity {
                identity: crate::identity::IdentityHash::new([0x32; 16]),
                source,
                expiry: crate::routing::BlackholeExpiry::At(InstantMillis(999)),
                reason: Some("expired"),
            },
        ]);

        assert_eq!(
            report,
            BlackholeSeedReport {
                seeded_count: 1,
                refused_count: 1,
                dropped_count: 1,
            }
        );
        assert!(node.engine.is_identity_blackholed(&identity));
        assert_eq!(node.engine.blackholed_identity_count(), 1);
    }

    #[cfg(feature = "runtime-metrics")]
    #[tokio::test]
    async fn metrics_snapshots_are_requested_from_the_reactor() {
        let (handle, mut command_rx) = handle();
        let expected = RuntimeMetricsSnapshot {
            taken_at: InstantMillis(42),
            engine: Default::default(),
            egress: Default::default(),
            crypto: None,
            reliability: Default::default(),
        };
        let snapshotting = tokio::spawn(async move { handle.metrics_snapshot().await });

        let HostCommand::SnapshotMetrics { reply } = command_rx.recv().await.unwrap() else {
            panic!("expected a metrics snapshot command");
        };
        reply.send(expected.clone()).unwrap();

        assert_eq!(snapshotting.await.unwrap(), Some(expected));
    }

    #[tokio::test]
    async fn announce_rate_inspection_settles_with_the_reactor_snapshot() {
        let (handle, mut command_rx) = handle();
        let expected = std::vec![AnnounceRateSnapshot {
            destination: DestinationHash::new([0x42; 16]),
            last_allowed_announce_at: InstantMillis(20),
            blocked_until: InstantMillis(0),
            rate_violations: 1,
            observed_at: std::vec![InstantMillis(10), InstantMillis(20)],
        }];
        let reading = tokio::spawn(async move { handle.announce_rates().await });

        let HostCommand::AwaitedEngine { issued, completion } = command_rx.recv().await.unwrap()
        else {
            panic!("expected an awaited engine command");
        };
        assert_eq!(
            issued.command,
            EngineCommand::Inspect(InspectionQuery::AnnounceRates)
        );
        completion
            .send(Settlement::Inspection(InspectionResult::AnnounceRates(
                expected.clone(),
            )))
            .unwrap();

        assert_eq!(reading.await.unwrap(), expected);
    }

    #[tokio::test]
    async fn routing_controls_resolve_their_typed_reactor_replies() {
        let (handle, mut command_rx) = handle();

        let dropping = tokio::spawn({
            let handle = handle.clone();
            async move { handle.drop_route(PEER).await }
        });
        let HostCommand::DropRoute { destination, reply } = command_rx.recv().await.unwrap() else {
            panic!("expected a route drop command");
        };
        assert_eq!(destination, PEER);
        reply.send(DropRouteOutcome::Dropped).unwrap();
        assert_eq!(dropping.await.unwrap(), Ok(DropRouteOutcome::Dropped));

        let transport = TransportId::new([0x42; 16]);
        let dropping_via = tokio::spawn({
            let handle = handle.clone();
            async move { handle.drop_routes_via(transport).await }
        });
        let HostCommand::DropRoutesVia {
            transport: requested,
            reply,
        } = command_rx.recv().await.unwrap()
        else {
            panic!("expected a transport route drop command");
        };
        assert_eq!(requested, transport);
        reply
            .send(DropRoutesViaOutcome { dropped_routes: 3 })
            .unwrap();
        assert_eq!(
            dropping_via.await.unwrap(),
            Ok(DropRoutesViaOutcome { dropped_routes: 3 })
        );

        let clearing = tokio::spawn(async move { handle.clear_announce_queues().await });
        let HostCommand::ClearAnnounceQueues { reply } = command_rx.recv().await.unwrap() else {
            panic!("expected an announce queue clear command");
        };
        reply
            .send(ClearAnnounceQueuesOutcome {
                dropped_announces: 5,
            })
            .unwrap();
        assert_eq!(
            clearing.await.unwrap(),
            Ok(ClearAnnounceQueuesOutcome {
                dropped_announces: 5,
            })
        );
    }

    #[tokio::test]
    async fn routing_controls_report_a_stopped_reactor() {
        let (handle, command_rx) = handle();
        drop(command_rx);

        assert_eq!(
            handle.drop_route(PEER).await,
            Err(RoutingControlError::NodeStopped)
        );
        assert_eq!(
            handle.drop_routes_via(TransportId::new([0x42; 16])).await,
            Err(RoutingControlError::NodeStopped)
        );
        assert_eq!(
            handle.clear_announce_queues().await,
            Err(RoutingControlError::NodeStopped)
        );
    }

    #[tokio::test]
    async fn identity_blackhole_capabilities_resolve_typed_reactor_replies() {
        let (handle, mut command_rx) = handle();
        let identity = crate::identity::IdentityHash::new([0x31; 16]);
        let source = crate::identity::IdentityHash::new([0x41; 16]);
        let expected = BlackholedIdentity {
            identity,
            source,
            expiry: crate::routing::BlackholeExpiry::Indefinite,
            reason: Some(String::from("operator")),
        };

        let reading = tokio::spawn({
            let handle = handle.clone();
            async move { handle.blackholed_identities().await }
        });
        let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::ReadAll { reply }) =
            command_rx.recv().await.unwrap()
        else {
            panic!("expected a blackhole table read command");
        };
        reply.send(vec![expected.clone()]).unwrap();
        assert_eq!(reading.await.unwrap(), Ok(vec![expected.clone()]));

        let checking = tokio::spawn({
            let handle = handle.clone();
            async move { handle.is_blackholed(identity).await }
        });
        let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::IsBlackholed {
            identity: requested,
            reply,
        }) = command_rx.recv().await.unwrap()
        else {
            panic!("expected an identity blackhole query command");
        };
        assert_eq!(requested, identity);
        reply.send(true).unwrap();
        assert_eq!(checking.await.unwrap(), Ok(true));

        let blackholing = tokio::spawn({
            let handle = handle.clone();
            async move {
                handle
                    .blackhole_identity(BlackholedIdentity {
                        identity,
                        source,
                        expiry: crate::routing::BlackholeExpiry::Indefinite,
                        reason: Some("operator"),
                    })
                    .await
            }
        });
        let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::Blackhole {
            entry,
            reply,
        }) = command_rx.recv().await.unwrap()
        else {
            panic!("expected an identity blackhole command");
        };
        assert_eq!(entry, expected);
        reply.send(Ok(BlackholeIdentityOutcome::Added)).unwrap();
        assert_eq!(
            blackholing.await.unwrap(),
            Ok(BlackholeIdentityOutcome::Added)
        );

        let unblackholing =
            tokio::spawn(async move { handle.unblackhole_identity(identity).await });
        let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::Unblackhole {
            identity: requested,
            reply,
        }) = command_rx.recv().await.unwrap()
        else {
            panic!("expected an identity unblackhole command");
        };
        assert_eq!(requested, identity);
        reply
            .send(Ok(crate::routing::UnblackholeIdentityOutcome::Removed))
            .unwrap();
        assert_eq!(
            unblackholing.await.unwrap(),
            Ok(crate::routing::UnblackholeIdentityOutcome::Removed)
        );
    }

    #[tokio::test]
    async fn identity_blackhole_capabilities_report_a_stopped_reactor() {
        let (handle, command_rx) = handle();
        drop(command_rx);
        let identity = crate::identity::IdentityHash::new([0x31; 16]);
        let source = crate::identity::IdentityHash::new([0x41; 16]);

        assert_eq!(
            handle.blackholed_identities().await,
            Err(IdentityBlackholeSourceError::NodeStopped)
        );
        assert_eq!(
            handle.is_blackholed(identity).await,
            Err(IdentityBlackholeSourceError::NodeStopped)
        );
        assert_eq!(
            handle
                .blackhole_identity(BlackholedIdentity {
                    identity,
                    source,
                    expiry: crate::routing::BlackholeExpiry::Indefinite,
                    reason: None,
                })
                .await,
            Err(IdentityBlackholeControlError::NodeStopped)
        );
        assert_eq!(
            handle.unblackhole_identity(identity).await,
            Err(IdentityBlackholeControlError::NodeStopped)
        );
    }

    struct StatusInterface {
        tag: std::vec::Vec<u8>,
        status: TokioInterfaceStatus,
    }

    impl StatusInterface {
        fn new(tag: &[u8]) -> Self {
            let id = InterfaceId::from_channel_tag(InterfaceKind::Pipe, tag);
            Self {
                tag: tag.to_vec(),
                status: TokioInterfaceStatus::new(
                    id,
                    crate::interfaces::ConnectionState::Connected,
                ),
            }
        }

        fn id(&self) -> InterfaceId {
            self.status.id()
        }
    }

    impl Interface for StatusInterface {
        const HW_MTU: usize = crate::wire::BROADCAST_MTU;
        const KIND: InterfaceKind = InterfaceKind::Pipe;

        fn channel_tag(&self) -> &[u8] {
            &self.tag
        }

        fn descriptor(&self) -> crate::interfaces::InterfaceDescriptor {
            crate::interfaces::InterfaceDescriptor {
                id: self.id(),
                capabilities: crate::interfaces::InterfaceCapabilities {
                    ingress: crate::interfaces::IngressCapability::Enabled,
                    egress: crate::interfaces::EgressCapability::Enabled(
                        crate::interfaces::TransportCapability::CrossInterfaceOnly,
                    ),
                },
                mode: crate::interfaces::InterfaceMode::Full,
                bitrate: crate::interfaces::BitrateBps::guess(1_000_000),
                hardware_mtu: None,
                announce_rate_limit: None,
                announce_bandwidth_cap: crate::interfaces::AnnounceBandwidthCap::Unlimited,
                airtime_duty_cycle: None,
            }
        }

        async fn run<S: crate::reactor::interface_seam::InterfaceSeam>(self, _seam: S) {}
    }

    impl ReportsStatus for StatusInterface {
        fn status_view(&self) -> Option<StatusView> {
            let status = self.status.clone();
            Some(Arc::new(move || std::vec![InterfaceVitals::of(&status)]))
        }

        fn connection_view(&self) -> Option<ConnectionView> {
            Some(ConnectionView::of(self.status.clone()))
        }
    }

    #[tokio::test]
    async fn runtime_attachment_carries_ifac_wire_and_status_metadata() {
        let (handle, mut command_rx) = handle();
        let interface = StatusInterface::new(b"protected-wire");
        let id = interface.id();
        let ifac =
            IfacContext::derive(Some("private-net"), Some("secret"), IfacSize::WIDE).unwrap();
        let signature = ifac.ifac_signature();
        let _attached =
            handle.add_interface_with_ifac_name(interface, ifac, Some("private-net".into()));

        let HostCommand::AddInterface(add) = command_rx.recv().await.unwrap() else {
            panic!("expected an interface add");
        };
        assert_eq!(
            add.connection.as_ref().map(ConnectionView::connection),
            Some(crate::interfaces::ConnectionState::Connected)
        );
        let wire_ifac = add.ifac.unwrap();
        assert_eq!(wire_ifac.ifac_signature(), signature);
        assert_eq!(wire_ifac.ifac_size(), IfacSize::WIDE);
        assert!(handle.set_interface_name(id, "Protected wire"));

        assert_eq!(
            handle.interface_inventory(),
            std::vec![InterfaceInventoryEntry {
                configured_name: Some("Protected wire".into()),
                snapshot: InterfaceSnapshot {
                    id,
                    connection: crate::interfaces::ConnectionState::Connected,
                    failure_reason: None,
                    rx_bytes: 0,
                    tx_bytes: 0,
                    transfer_rates: None,
                    destinations: 0,
                    links: 0,
                    transported_links: 0,
                    membership: Membership::Independent,
                },
                ifac: Some(InterfaceIfacSnapshot {
                    signature,
                    size: IfacSize::WIDE,
                    network_name: Some("private-net".into()),
                }),
            }]
        );
    }

    #[tokio::test]
    async fn a_fleet_member_inherits_its_supervisors_ifac() {
        let supervisor = InterfaceId::new([0x71; 8]);
        let (mut fleet, mut tail) = Fleet::detached(supervisor);
        let ifac = IfacContext::derive(Some("fleet-net"), None, IfacSize::NARROW).unwrap();
        let signature = ifac.ifac_signature();
        fleet.ifac = Some(RuntimeIfac {
            context: ifac,
            network_name: Some("fleet-net".into()),
        });
        let interface = StatusInterface::new(b"fleet-member");
        let id = interface.id();
        let _attached = fleet.add(interface);

        let HostCommand::AddInterface(add) = tail._commands.recv().await.unwrap() else {
            panic!("expected a fleet member add");
        };
        assert_eq!(add.ifac.unwrap().ifac_signature(), signature);
        let map = fleet.interfaces.lock().unwrap();
        assert_eq!(
            map.get(&id)
                .unwrap()
                .ifac
                .as_ref()
                .unwrap()
                .network_name
                .as_deref(),
            Some("fleet-net")
        );
    }

    #[tokio::test]
    async fn request_emits_a_request_any_and_returns_the_response_with_its_rtt() {
        let (handle, mut command_rx) = handle();
        let link = LinkId::new([5; 16]);
        let path_hash = RequestPathHash::new([0x44; 16]);

        let requesting =
            tokio::spawn(async move { handle.request(link, path_hash, b"ping").await });

        let HostCommand::RequestAny(request) = command_rx.recv().await.unwrap() else {
            panic!("request issues a RequestAny host command");
        };
        assert_eq!(request.link_id, link);
        assert_eq!(request.path_hash, path_hash);
        assert_eq!(request.data.as_slice(), &b"ping"[..]);
        request
            .completion
            .send(Ok((b"pong".to_vec(), RttMillis::new(42))))
            .unwrap();

        let (data, rtt) = requesting.await.unwrap().unwrap();
        assert_eq!(data, b"pong");
        assert_eq!(rtt, RttMillis::new(42));
    }

    #[tokio::test]
    async fn respond_returns_the_links_round_trip() {
        use crate::routing::links::request::RequestId;
        use crate::runtime::request_router::RespondToken;

        let (handle, _command_rx) = handle();
        let token = RespondToken {
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            rtt: RttMillis::new(99),
        };
        assert_eq!(
            handle.respond(token, b"answer"),
            Some(RttMillis::new(99)),
            "respond surfaces the rtt the request arrived on",
        );
    }

    #[tokio::test]
    async fn a_large_response_carries_a_bz2_candidate() {
        use crate::routing::links::request::RequestId;
        use crate::runtime::request_router::RespondToken;

        let (handle, mut command_rx) = handle();
        let token = RespondToken {
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            rtt: RttMillis::new(50),
        };
        let body = std::vec![42u8; RESPONSE_PACKET_CEILING + 4096];
        assert_eq!(handle.respond(token, &body), Some(RttMillis::new(50)));
        let Some(HostCommand::RespondAny(respond)) = command_rx.recv().await else {
            panic!("expected a RespondAny command");
        };
        assert_eq!(
            respond
                .compressed_candidate
                .as_ref()
                .map(|c| c.as_slice().to_vec()),
            compression::compress_if_smaller(&body),
            "a response past the packet ceiling rides a bz2 candidate matching the codec",
        );
        assert!(respond.compressed_candidate.is_some(), "a run compresses");
    }

    #[tokio::test]
    async fn a_packet_sized_response_skips_compression() {
        use crate::routing::links::request::RequestId;
        use crate::runtime::request_router::RespondToken;

        let (handle, mut command_rx) = handle();
        let token = RespondToken {
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            rtt: RttMillis::new(50),
        };
        let body = std::vec![42u8; RESPONSE_PACKET_CEILING];
        handle.respond(token, &body);
        let Some(HostCommand::RespondAny(respond)) = command_rx.recv().await else {
            panic!("expected a RespondAny command");
        };
        assert!(
            respond.compressed_candidate.is_none(),
            "a response that fits a packet never builds a candidate the rung would discard",
        );
    }

    #[tokio::test]
    async fn a_self_completing_interface_run_deregisters_it() {
        // The driver is deliberately `!Send` (it drives `!Send` interface futures on the run task),
        // so it is run concurrently with the assertion via `join!`, never spawned.
        let (msg_tx, msg_rx) = mpsc::unbounded_channel::<DriverMsg>();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<HostCommand>();

        let id = InterfaceId::from_channel_tag(
            crate::interfaces::InterfaceKind::LocalClient,
            b"ephemeral-peer",
        );
        msg_tx
            .send(DriverMsg::Add {
                id,
                supervisor: None,
                build: Box::new(|| {
                    let run: Pin<Box<dyn Future<Output = ()>>> = Box::pin(async {});
                    run
                }),
            })
            .expect("the driver is listening");
        // Closing the channel lets the driver drain and return once the self-completed member's
        // cull is done, so the `join!` below terminates.
        drop(msg_tx);

        let interfaces = Arc::new(Mutex::new(HashMap::new()));
        tokio::join!(
            drive_interfaces(std::vec![], msg_rx, cmd_tx, interfaces),
            async {
                let command =
                    tokio::time::timeout(std::time::Duration::from_secs(1), cmd_rx.recv())
                        .await
                        .expect("the driver culls the completed interface within 1s")
                        .expect("the command channel stays open");
                assert!(
                    matches!(
                        command,
                        HostCommand::RemoveInterface {
                            id: removed,
                            departure: Departure::MayReturn,
                        } if removed == id
                    ),
                    "an interface whose run ended on its own deregisters itself as a may-return departure"
                );
            }
        );
    }

    #[tokio::test]
    async fn payload_beyond_the_mdu_is_rejected_before_the_wire() {
        let (prns, _command_rx) = handle();
        let oversize = [0u8; MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN + 1];
        assert_eq!(
            prns.send_single_packet(PEER, &oversize).await,
            Err(SendError::PayloadTooLarge),
        );
    }

    #[tokio::test]
    async fn a_send_on_a_stopped_node_settles_as_node_stopped() {
        let (prns, command_rx) = handle();
        drop(command_rx);
        assert_eq!(
            prns.send_single_packet(PEER, b"ping").await,
            Err(SendError::NodeStopped),
        );
    }

    #[tokio::test]
    async fn an_awaited_send_issues_the_completion_carrying_command() {
        let (prns, mut command_rx) = handle();
        let issuer = prns.clone();
        let send = tokio::spawn(async move { issuer.send_single_packet(PEER, b"ping").await });

        match command_rx.recv().await.expect("the command was issued") {
            HostCommand::AwaitedEngine { issued, completion } => {
                assert!(matches!(issued.command, EngineCommand::SendSinglePacket(_)));
                completion
                    .send(Settlement::SendSinglePacket(Ok(PacketReceiptDelivered {
                        rtt: crate::units::RttMillis::new(7),
                    })))
                    .expect("the awaiter is still parked");
            }
            _ => panic!("send_single must issue an AwaitedEngine command"),
        }

        assert_eq!(
            send.await.expect("the send task joins"),
            Ok(PacketReceiptDelivered {
                rtt: crate::units::RttMillis::new(7),
            }),
        );
    }

    #[tokio::test]
    async fn establish_link_resolves_the_link_id_from_the_settlement() {
        use crate::engine::LinkEstablished;

        let (prns, mut command_rx) = handle();
        let issuer = prns.clone();
        let establish = tokio::spawn(async move { issuer.establish_link(PEER).await });

        match command_rx.recv().await.expect("the command was issued") {
            HostCommand::AwaitedEngine { issued, completion } => {
                assert_eq!(
                    issued.command,
                    EngineCommand::EstablishLink(EstablishLink { destination: PEER }),
                );
                completion
                    .send(Settlement::EstablishLink(Ok(LinkEstablished {
                        link_id: LinkId::new([0x42; 16]),
                        rtt_ms: 11,
                    })))
                    .expect("the awaiter is still parked");
            }
            _ => panic!("establish_link must issue an AwaitedEngine command"),
        }

        assert_eq!(
            establish.await.expect("the establish task joins"),
            Ok(LinkId::new([0x42; 16])),
        );
    }

    #[tokio::test]
    async fn establish_link_surfaces_a_typed_failure() {
        let (prns, mut command_rx) = handle();
        let issuer = prns.clone();
        let establish = tokio::spawn(async move { issuer.establish_link(PEER).await });

        let HostCommand::AwaitedEngine { completion, .. } =
            command_rx.recv().await.expect("the command was issued")
        else {
            panic!("establish_link must issue an AwaitedEngine command");
        };
        completion
            .send(Settlement::EstablishLink(Err(
                EstablishLinkFailure::Timeout,
            )))
            .expect("the awaiter is still parked");

        assert_eq!(
            establish.await.expect("the establish task joins"),
            Err(SendError::Failed(EstablishLinkFailure::Timeout)),
        );
    }

    #[tokio::test]
    async fn byte_stream_reader_is_withheld_until_the_run_loop_acks_registration() {
        let (prns, mut command_rx) = handle();
        let link = LinkId::new([5; 16]);
        let stream = StreamId::new(2).unwrap();
        let opener = prns.clone();
        let open = tokio::spawn(async move { opener.byte_stream_reader(link, stream).await });

        let HostCommand::RegisterStreamReader {
            link_id,
            stream_id,
            ready,
            ..
        } = command_rx
            .recv()
            .await
            .expect("the registration was issued")
        else {
            panic!("byte_stream_reader must register its sink");
        };
        assert_eq!(link_id, link);
        assert_eq!(stream_id, stream);
        assert!(
            !open.is_finished(),
            "the reader is held back until the run loop acknowledges the registration",
        );

        ready.send(()).expect("the opener is parked on the ack");
        open.await.expect("the reader future resolves once acked");
    }

    #[test]
    fn the_prns_api_trait_dispatches_to_the_handle() {
        use crate::routing::links::LinkId;
        use crate::runtime::PrnsApi;

        let (prns, mut command_rx) = handle();
        let queued = PrnsApi::close_link(&prns, LinkId::new([3; 16]));
        assert!(
            queued,
            "the trait method reaches the handle and queues the close"
        );
        assert!(
            matches!(command_rx.try_recv(), Ok(HostCommand::Engine(_))),
            "dispatched through PrnsApi, the close rode the channel"
        );
    }

    const LINK: LinkId = LinkId::new([5; 16]);

    #[tokio::test]
    async fn send_resource_drains_a_source_into_proven_segments() {
        let (prns, mut command_rx) = handle();
        let total_len = MAX_EFFICIENT_SIZE as u64 + 100;
        let payload: std::vec::Vec<u8> = (0..total_len).map(|i| i as u8).collect();

        let drainer = tokio::spawn(async move {
            let mut got = std::vec::Vec::new();
            loop {
                let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
                    panic!("expected a SendResourceSegment command");
                };
                let last = seg.segment_index == seg.total_segments;
                if seg.segment_index == 1 {
                    assert!(
                        seg.compressed_candidate.is_some(),
                        "a compressible split segment carries its bz2 candidate",
                    );
                }
                got.push((
                    seg.segment_index,
                    seg.total_segments,
                    seg.data.as_slice().to_vec(),
                ));
                seg.completion
                    .send(Settlement::SendResource(Ok(())))
                    .expect("the awaiter is still parked");
                if last {
                    break;
                }
            }
            got
        });

        prns.send_resource(LINK, total_len, &payload[..])
            .await
            .expect("the stream completes");
        let got = drainer.await.unwrap();

        assert_eq!(got.len(), 2, "a payload one segment over splits in two");
        assert_eq!((got[0].0, got[0].1), (1, 2));
        assert_eq!((got[1].0, got[1].1), (2, 2));
        assert_eq!(got[0].2.len(), MAX_EFFICIENT_SIZE);
        assert_eq!(got[1].2.len(), 100);
        let mut reassembled = got[0].2.clone();
        reassembled.extend_from_slice(&got[1].2);
        assert_eq!(
            reassembled, payload,
            "the segments reassemble to the source"
        );
    }

    #[tokio::test]
    async fn a_small_send_resource_is_one_unsplit_segment() {
        let (prns, mut command_rx) = handle();
        let payload = std::vec![3u8; 500];
        let drainer = tokio::spawn(async move {
            let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
                panic!("expected a SendResourceSegment command");
            };
            let placement = (
                seg.segment_index,
                seg.total_segments,
                seg.data.as_slice().len(),
            );
            seg.completion
                .send(Settlement::SendResource(Ok(())))
                .expect("the awaiter is still parked");
            placement
        });
        prns.send_resource(LINK, 500, &payload[..])
            .await
            .expect("the single segment completes");
        assert_eq!(
            drainer.await.unwrap(),
            (1, 1, 500),
            "a sub-segment payload crosses as one unsplit resource",
        );
    }

    #[tokio::test]
    async fn a_resource_length_that_overflows_with_metadata_is_rejected() {
        let (prns, mut command_rx) = handle();
        let error = prns
            .send_resource_with_metadata(LINK, u64::MAX, &[][..], &[0x81])
            .await
            .unwrap_err();
        assert!(matches!(error, ResourceSendError::UnrepresentableLength));
        assert!(command_rx.try_recv().is_err());
    }

    #[test]
    fn a_split_resource_claim_cannot_raise_the_per_segment_inflate_bound() {
        assert_eq!(
            resource_segment_decompression_bound(u64::MAX),
            MAX_EFFICIENT_SIZE as u64,
        );
        assert_eq!(resource_segment_decompression_bound(4096), 4096);
    }

    #[tokio::test]
    async fn send_resource_compresses_a_compressible_segment() {
        let (prns, mut command_rx) = handle();
        let payload = std::vec![7u8; 8192];
        let drainer = tokio::spawn(async move {
            let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
                panic!("expected a SendResourceSegment command");
            };
            let candidate = seg
                .compressed_candidate
                .as_ref()
                .map(|c| c.as_slice().to_vec());
            seg.completion
                .send(Settlement::SendResource(Ok(())))
                .expect("the awaiter is still parked");
            candidate
        });
        prns.send_resource(LINK, payload.len() as u64, &payload[..])
            .await
            .expect("the single segment completes");
        let candidate = drainer.await.unwrap();
        assert_eq!(
            candidate,
            compression::compress_if_smaller(&payload),
            "the segment rides a bz2 candidate matching the codec",
        );
        assert!(
            candidate.is_some_and(|c| c.len() < payload.len()),
            "a run of one byte compresses far below its length",
        );
    }

    #[tokio::test]
    async fn send_resource_declines_to_compress_incompressible_data() {
        let (prns, mut command_rx) = handle();
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let payload: std::vec::Vec<u8> = (0..8192)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        let drainer = tokio::spawn(async move {
            let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
                panic!("expected a SendResourceSegment command");
            };
            let compressed = seg.compressed_candidate.is_some();
            seg.completion
                .send(Settlement::SendResource(Ok(())))
                .expect("the awaiter is still parked");
            compressed
        });
        prns.send_resource(LINK, payload.len() as u64, &payload[..])
            .await
            .expect("the single segment completes");
        assert!(
            !drainer.await.unwrap(),
            "high-entropy bytes carry no candidate, so the transfer stays uncompressed",
        );
    }

    #[tokio::test]
    async fn never_compression_ships_a_compressible_segment_uncompressed() {
        let (prns, mut command_rx) = handle();
        let payload = std::vec![7u8; 8192];
        let drainer = tokio::spawn(async move {
            let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
                panic!("expected a SendResourceSegment command");
            };
            let compressed = seg.compressed_candidate.is_some();
            seg.completion
                .send(Settlement::SendResource(Ok(())))
                .expect("the awaiter is still parked");
            compressed
        });
        prns.send_resource_with_compression(
            LINK,
            payload.len() as u64,
            &payload[..],
            SegmentCompression::Never,
        )
        .await
        .expect("the single segment completes");
        assert!(
            !drainer.await.unwrap(),
            "RNS auto_compress=False: no attempt, even on a run that would compress",
        );
    }

    #[tokio::test]
    async fn a_segment_past_the_attempt_ceiling_ships_uncompressed() {
        let (prns, mut command_rx) = handle();
        let payload = std::vec![7u8; 8192];
        let drainer = tokio::spawn(async move {
            let Some(HostCommand::SendResourceSegment(seg)) = command_rx.recv().await else {
                panic!("expected a SendResourceSegment command");
            };
            let compressed = seg.compressed_candidate.is_some();
            seg.completion
                .send(Settlement::SendResource(Ok(())))
                .expect("the awaiter is still parked");
            compressed
        });
        prns.send_resource_with_compression(
            LINK,
            payload.len() as u64,
            &payload[..],
            SegmentCompression::Attempt {
                up_to_byte_len: payload.len() as u64 - 1,
            },
        )
        .await
        .expect("the single segment completes");
        assert!(
            !drainer.await.unwrap(),
            "RNS auto_compress=<int>: a segment over the ceiling is never attempted",
        );
    }

    #[tokio::test]
    async fn send_resource_surfaces_a_segment_rejection_and_stops() {
        let (prns, mut command_rx) = handle();
        let total_len = 2 * MAX_EFFICIENT_SIZE as u64 + 100;
        let payload = std::vec![7u8; total_len as usize];
        let drainer = tokio::spawn(async move {
            let mut issued = 0u32;
            while let Some(command) = command_rx.recv().await {
                let HostCommand::SendResourceSegment(seg) = command else {
                    panic!("expected a SendResourceSegment command");
                };
                issued += 1;
                let _ = seg.completion.send(Settlement::SendResource(Err(
                    SendResourceFailure::RejectedByPeer,
                )));
            }
            issued
        });

        let result = prns.send_resource(LINK, total_len, &payload[..]).await;
        assert!(matches!(
            result,
            Err(ResourceSendError::Rejected(
                SendResourceFailure::RejectedByPeer
            )),
        ));
        drop(prns);
        assert_eq!(
            drainer.await.unwrap(),
            ENGINE_SEGMENT_LANES as u32,
            "a rejected first segment stops the stream — only its already-staged follower ever issued, the third never does",
        );
    }

    #[tokio::test]
    async fn send_resource_on_a_stopped_node_is_node_stopped() {
        let (prns, command_rx) = handle();
        drop(command_rx);
        let payload = std::vec![0u8; 10];
        assert!(matches!(
            prns.send_resource(LINK, 10, &payload[..]).await,
            Err(ResourceSendError::NodeStopped),
        ));
    }

    #[tokio::test]
    async fn receive_resource_streams_an_inbound_resource_into_the_sink() {
        let (prns, mut command_rx) = handle();
        let original = ResourceHash::new([9; 32]);

        let actor = tokio::spawn(async move {
            let Some(HostCommand::RegisterResourceSink {
                link_id,
                sink,
                ready,
            }) = command_rx.recv().await
            else {
                panic!("expected a RegisterResourceSink command");
            };
            ready.send(()).expect("the receiver awaits registration");
            sink.send(ResourceInbound::Chunk(b"hello ".to_vec()))
                .unwrap();
            sink.send(ResourceInbound::Chunk(b"world".to_vec()))
                .unwrap();
            sink.send(ResourceInbound::Complete {
                original_hash: original,
                total_size: 11,
            })
            .unwrap();
            link_id
        });

        let mut buf = std::vec::Vec::new();
        let receipt = prns
            .receive_resource(LINK, &mut buf)
            .await
            .expect("the resource arrives");
        assert_eq!(
            actor.await.unwrap(),
            LINK,
            "the sink registered on the link"
        );
        assert_eq!(
            buf, b"hello world",
            "the chunks stream into the sink in order"
        );
        assert_eq!(
            receipt,
            ResourceReceipt {
                original_hash: original,
                total_size: 11,
                metadata: None,
            },
        );
    }

    #[tokio::test]
    async fn receive_resource_carries_metadata_on_the_receipt() {
        let (prns, mut command_rx) = handle();
        let original = ResourceHash::new([9; 32]);

        let actor = tokio::spawn(async move {
            let Some(HostCommand::RegisterResourceSink { sink, ready, .. }) =
                command_rx.recv().await
            else {
                panic!("expected a RegisterResourceSink command");
            };
            ready.send(()).expect("the receiver awaits registration");
            sink.send(ResourceInbound::Metadata(b"packed".to_vec()))
                .unwrap();
            sink.send(ResourceInbound::Chunk(b"payload".to_vec()))
                .unwrap();
            sink.send(ResourceInbound::Complete {
                original_hash: original,
                total_size: 7,
            })
            .unwrap();
        });

        let mut buf = std::vec::Vec::new();
        let receipt = prns
            .receive_resource(LINK, &mut buf)
            .await
            .expect("the resource arrives");
        actor.await.unwrap();
        assert_eq!(buf, b"payload", "the metadata never enters the byte stream");
        assert_eq!(
            receipt,
            ResourceReceipt {
                original_hash: original,
                total_size: 7,
                metadata: Some(b"packed".to_vec()),
            },
        );
    }

    #[tokio::test]
    async fn receive_resource_surfaces_a_failed_transfer() {
        let (prns, mut command_rx) = handle();
        let actor = tokio::spawn(async move {
            let Some(HostCommand::RegisterResourceSink { sink, ready, .. }) =
                command_rx.recv().await
            else {
                panic!("expected a RegisterResourceSink command");
            };
            ready.send(()).unwrap();
            sink.send(ResourceInbound::Failed).unwrap();
        });
        let mut buf = std::vec::Vec::new();
        let result = prns.receive_resource(LINK, &mut buf).await;
        actor.await.unwrap();
        assert!(matches!(result, Err(ResourceReceiveError::Failed)));
        assert!(buf.is_empty(), "a failed transfer wrote nothing");
    }

    #[tokio::test]
    async fn receive_resource_on_a_stopped_node_is_node_stopped() {
        let (prns, command_rx) = handle();
        drop(command_rx);
        let mut buf = std::vec::Vec::new();
        assert!(matches!(
            prns.receive_resource(LINK, &mut buf).await,
            Err(ResourceReceiveError::NodeStopped),
        ));
    }
}

/// The boot origin for a wall-clocked host: wall time floored by the stored high-water, so a
/// rolled-back clock can never restart the timeline under persisted rows.
/// Absent or unreadable snapshots fall back gracefully — boot never blocks on storage health.
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
pub struct BlackholeSeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KnownDestinationSeedReport {
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

/// The fingerprints of the last flush this mark's owner landed, one per skippable region.
/// A fresh mark knows nothing, so its first flush writes everything once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FlushMark {
    pub routing_table: Option<SnapshotFingerprint>,
    pub tunnels: Option<SnapshotFingerprint>,
    pub known_destinations: Option<SnapshotFingerprint>,
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
    pub known_destinations: RegionFlush,
}

#[derive(Debug)]
pub enum FlushError<E> {
    NodeStopped,
    Store(E),
}
