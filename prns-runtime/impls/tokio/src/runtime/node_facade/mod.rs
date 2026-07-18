mod interface_lifecycle;
mod node_lifecycle;
mod persistence;

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::oneshot;

use crate::engine::{
    AnnounceNow, AnnounceNowFailure, CloseLink, CommandId, EngineCommand, EstablishLink,
    EstablishLinkFailure, IssuedCommand, PacketReceiptDelivered, PathFound, PathRequestId,
    RequestPath, RequestPathFailure, SendRequestFailure, SendResourceFailure, SendSinglePacket,
    SendSinglePacketFailure, SendSinglePacketPayload, Settlement, PATH_REQUEST_ID_LEN,
};
use crate::interfaces::{InterfaceId, PacketPhyStats};
use crate::node_introspection::{
    AnnounceRateSnapshot, InterfaceInventoryEntry, NodeIntrospection, NodeIntrospectionRequest,
    RouteSnapshot,
};
use crate::reactor::compression;
use crate::reactor::driver::{
    HostCommand, HostResourceMetadata, HostResourcePayload, RequestAnyHostCommand, ResourceInbound,
    RespondAnyHostCommand, SendResourceSegmentHostCommand,
};
use crate::routing::dedup::PacketHash;
use crate::routing::links::data::LINK_MDU;
use crate::routing::links::request::{RequestId, RESPONSE_WIRE_OVERHEAD};
use crate::routing::links::resources::{
    ResourceHash, ResourceStrategy, MAX_EFFICIENT_SIZE, METADATA_PREFIX_LEN,
};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestHandlerError, RequestPathHash};
use crate::routing::BlackholedIdentity;
use crate::storage::TablePushError;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, TransportId};

use super::byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};
use super::identity_blackhole::{settle_control, settle_source};
use super::request_router::RespondToken;
use super::settle_destination_identity_retention;
#[cfg(feature = "runtime-metrics")]
use super::RuntimeMetricsSnapshot;
use super::{
    ClearAnnounceQueuesOutcome, DestinationIdentityRetentionControl,
    DestinationIdentityRetentionControlError, DestinationIdentityRetentionHostCommand,
    DropRouteOutcome, DropRoutesViaOutcome, IdentityBlackholeControl,
    IdentityBlackholeControlError, IdentityBlackholeHostCommand, IdentityBlackholeSource,
    IdentityBlackholeSourceError, InterfaceStore, RoutingControl, RoutingControlError, SendError,
};
pub use interface_lifecycle::{
    AttachIntent, Attachable, AttachedInterface, AttachedSupervisor, DetachedFleet, Fleet,
    InterfaceAttachmentMetadata, InterfaceSupervisor,
};
use interface_lifecycle::{DriverMsg, RegisteredInterface};
pub use node_lifecycle::{NonRoutingIdentityError, PrnsNode, SharedInstanceIdentityError};
pub use persistence::{
    boot_timeline_origin, BlackholeSeedReport, DestinationIdentitySeedReport, FlushError,
    FlushMark, FlushReport, PrepareFlushError, PreparedFlush, RatchetSeedReport, RegionFlush,
    RouteSeedProgress, RouteSeedReport, TunnelSeedReport,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPathError {
    EntropyUnavailable,
    Failed(RequestPathFailure),
    NodeStopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterRequestRouteError {
    Registration(TablePushError),
    Seed(RequestHandlerError),
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

    pub async fn request_path(
        &self,
        destination: DestinationHash,
    ) -> Result<PathFound, RequestPathError> {
        let mut request_id = [0; PATH_REQUEST_ID_LEN];
        getrandom::getrandom(&mut request_id).map_err(|_| RequestPathError::EntropyUnavailable)?;
        match self
            .settle(EngineCommand::RequestPath(RequestPath {
                destination,
                id: PathRequestId::new(request_id),
            }))
            .await
        {
            Some(Settlement::RequestPath(result)) => result.map_err(RequestPathError::Failed),
            Some(_) | None => Err(RequestPathError::NodeStopped),
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

#[cfg(test)]
mod tests;
