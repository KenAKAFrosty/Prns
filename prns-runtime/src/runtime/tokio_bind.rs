use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::engine::{
    CloseLink, CommandId, EngineCommand, EngineState, EstablishLink, EstablishLinkFailure,
    IssuedCommand, PacketReceiptDelivered, SendRequestFailure, SendResourceFailure,
    SendSinglePacket, SendSinglePacketFailure, SendSinglePacketPayload, Settlement,
};
use crate::engine::{RpcPathEntry, RpcQuery, RpcQueryResult};
use crate::identity::IdentityHash;
use crate::interfaces::{
    InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceVitals, Membership, ReportsStatus,
    StatusView,
};
use crate::reactor::impls::tokio_reactor::{
    self, tokio_grant_lane, AddInterfaceCommand, CryptoPoolConfig, Egress, HostCommand,
    HostResourcePayload, RequestAnyHostCommand, ResourceInbound, RespondAnyHostCommand,
    SendResourceSegmentHostCommand, TokioHost, TokioInterfaceSeam,
};
use crate::reactor::interface_seam::{frame_cap_for, Interface};
use crate::routing::links::resources::{ResourceHash, ResourceStrategy, MAX_EFFICIENT_SIZE};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::storage::StorageLayout;
use crate::units::Rtt;
use crate::wire::DestinationHash;

use super::byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};
use super::recipe::{Manual, PreConfiguredDestination};
use super::request_router::{RespondToken, RouteSet};
use super::tokio_runner::{run_router, RunnerRequest, REQUEST_QUEUE_DEPTH};
use super::{InterfaceStore, Message, PrnsEvent, PrnsRecipe, SendError};

/// How many frames a host lane holds in flight. RNS resource transfer bursts a whole window of
/// parts at once (`Resource.WINDOW_MAX_FAST` is 75, plus its flexibility), so a lane carrying a
/// transfer must be deeper than that window or it sheds parts and the transfer stalls; the old
/// byte-budget collapsed a fat-MTU lane to a handful of slots, exactly that failure. Growable
/// slots (`HeapFrameSlot`) cost only the frames actually in flight, so the depth is generous.
const HOST_LANE_DEPTH: usize = 256;

fn lane_depth_for(_slot_cap: usize) -> usize {
    HOST_LANE_DEPTH
}

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

/// Why a [`send_resource`](TokioPrnsHandle::send_resource) stream did not complete.
#[derive(Debug)]
pub enum ResourceSendError {
    /// Reading `source` failed before the whole resource was sent.
    Source(std::io::Error),
    /// A segment was refused, timed out, or rejected by the peer.
    Rejected(SendResourceFailure),
    /// The node's reactor has stopped.
    NodeStopped,
}

/// What a completed [`receive_resource`](TokioPrnsHandle::receive_resource) yields: the assembled
/// resource's identity and total size. The bytes themselves were streamed to the caller's sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceReceipt {
    pub original_hash: ResourceHash,
    pub total_size: u64,
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
    pub async fn request(
        &self,
        link_id: LinkId,
        path_hash: RequestPathHash,
        data: &[u8],
    ) -> Result<(std::vec::Vec<u8>, Rtt), SendError<SendRequestFailure>> {
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
    /// one segment at a time and awaiting each segment's proof before reading the next, so the
    /// engine and the host each hold a single segment, never the whole payload. The length is
    /// explicit because every segment advertises the total up front; a payload at or under one segment crosses unsplit.
    pub async fn send_resource(
        &self,
        link_id: LinkId,
        total_len: u64,
        mut source: impl AsyncRead + Unpin,
    ) -> Result<(), ResourceSendError> {
        let segment_size = MAX_EFFICIENT_SIZE as u64;
        let total_segments = total_len.div_ceil(segment_size).max(1);
        let mut remaining = total_len;
        for segment_index in 1..=total_segments {
            let this_segment = remaining.min(segment_size);
            remaining -= this_segment;
            let mut chunk = std::vec![0u8; this_segment as usize];
            source
                .read_exact(&mut chunk)
                .await
                .map_err(ResourceSendError::Source)?;
            let id = self.mint();
            let (completion, settled) = oneshot::channel();
            self.commands
                .send(HostCommand::SendResourceSegment(
                    SendResourceSegmentHostCommand {
                        id,
                        link_id,
                        data: chunk.into(),
                        request_id: None,
                        segment_index,
                        total_segments,
                        total_data_size: total_len,
                        completion,
                    },
                ))
                .map_err(|_| ResourceSendError::NodeStopped)?;
            match settled.await {
                Ok(Settlement::SendResource(Ok(()))) => {}
                Ok(Settlement::SendResource(Err(failure))) => {
                    return Err(ResourceSendError::Rejected(failure))
                }
                Ok(_) | Err(_) => return Err(ResourceSendError::NodeStopped),
            }
        }
        Ok(())
    }

    /// Receive the next inbound resource on `link_id`, streaming it into `sink`: the mirror of
    /// [`send_resource`](Self::send_resource). Registers the sink before yielding, so a segment
    /// arriving the instant after cannot reach the app event stream instead; resolves with the assembled identity and size.
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
        loop {
            match inbound.recv().await {
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

    /// Bring a link up to `destination` and await it: `Ok(LinkId)` once the peer's proof
    /// validates, or the typed reason it never established. The resolved id is the handle every
    /// link-scoped verb takes.
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

    fn send_response(&self, responder: RespondToken, data: HostResourcePayload) -> Option<Rtt> {
        let id = self.mint();
        self.commands
            .send(HostCommand::RespondAny(RespondAnyHostCommand {
                id,
                link_id: responder.link_id,
                request_id: responder.request_id,
                data,
                compressed_candidate: None,
            }))
            .ok()
            .map(|()| responder.rtt)
    }

    /// Answer a request via its token, returning the link's round trip (the request arrived over
    /// it) — or `None` if the node has stopped before the answer could be queued.
    pub fn respond(&self, responder: RespondToken, body: &[u8]) -> Option<Rtt> {
        self.send_response(responder, body.to_vec().into())
    }

    pub fn respond_owned(&self, responder: RespondToken, body: std::vec::Vec<u8>) -> Option<Rtt> {
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
        let view = interface.status_view();
        let attached = attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            None,
        );
        register_status(
            &self.interfaces,
            attached.id(),
            view,
            Membership::Independent,
        );
        attached
    }

    /// Every interface attached through this handle, as a complete [`InterfaceSnapshot`]: live
    /// vitals read at call time joined with the engine counts and fleet position. The whole
    /// fleet a face or the shared-instance control RPC renders, with no app-side bookkeeping.
    #[must_use]
    pub fn interfaces(&self) -> std::vec::Vec<InterfaceSnapshot> {
        let Ok(map) = self.interfaces.lock() else {
            return std::vec::Vec::new();
        };
        map.values()
            .flat_map(|registered| {
                let membership = registered.membership;
                (registered.view)().into_iter().map(move |vitals| {
                    let counts = self.store.counts(vitals.id);
                    InterfaceSnapshot {
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
                    }
                })
            })
            .collect()
    }

    /// Every interface's live [`InterfaceVitals`] without the engine counts
    /// [`interfaces`](Self::interfaces) joins on; the shared-instance RPC's status-only `interface_stats` source.
    #[must_use]
    pub fn interface_vitals(&self) -> std::vec::Vec<InterfaceVitals> {
        let Ok(map) = self.interfaces.lock() else {
            return std::vec::Vec::new();
        };
        map.values()
            .flat_map(|registered| (registered.view)())
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
        let id = InterfaceId::from_channel_tag(S::KIND, supervisor.channel_tag());
        let view = supervisor.status_view();
        let fleet = Fleet {
            supervisor_id: id,
            commands: self.commands.clone(),
            iface_build: self.iface_build.clone(),
            notify_tx: self.notify_tx.clone(),
            interfaces: self.interfaces.clone(),
        };
        let build: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send> =
            Box::new(move || Box::pin(supervisor.run(fleet)));
        let _ = self.iface_build.send(DriverMsg::Add {
            id,
            supervisor: None,
            build,
        });
        register_status(&self.interfaces, id, view, Membership::Independent);
        AttachedSupervisor {
            id,
            iface_build: self.iface_build.clone(),
        }
    }

    /// Detach the interface with this id (the inverse of [`add_interface`](Self::add_interface)):
    /// deregister its lanes on the reactor and stop its run future on the driver. For a supervisor,
    /// the driver cascades the stop to every member of its fleet.
    pub fn remove_interface(&self, id: InterfaceId) {
        let _ = self.commands.send(HostCommand::RemoveInterface { id });
        let _ = self.iface_build.send(DriverMsg::Stop { id });
    }

    /// Attach anything from the interface menu and get back its kind's attachment handle —
    /// the one verb over [`add_interface`](Self::add_interface) and [`supervise`](Self::supervise).
    pub fn attach<A: Attachable>(&self, attachable: A) -> A::Attached {
        attachable.attach_to(self)
    }
}

/// One registration story per menu type: the type itself encodes whether it joins as a single
/// wire (`add_interface`) or a discovery fleet (`supervise`), so no callsite has to know.
pub trait Attachable {
    type Attached;
    fn attach_to(self, handle: &TokioPrnsHandle) -> Self::Attached;
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

/// The node handle answers the shared-instance control RPC's read-only queries by demuxing each onto
/// the command lane and awaiting its settlement — the same `settle` path the diagnostic counts use.
impl crate::interfaces::shared_instance::rpc::RpcQuerySource for TokioPrnsHandle {
    async fn link_count(&self) -> u32 {
        match self
            .settle(EngineCommand::RpcQuery(RpcQuery::LinkCount))
            .await
        {
            Some(Settlement::RpcQuery(RpcQueryResult::LinkCount(count))) => count,
            Some(_) | None => 0,
        }
    }

    async fn path_table(&self) -> std::vec::Vec<RpcPathEntry> {
        match self
            .settle(EngineCommand::RpcQuery(RpcQuery::PathTable))
            .await
        {
            Some(Settlement::RpcQuery(RpcQueryResult::PathTable(rows))) => rows,
            Some(_) | None => std::vec::Vec::new(),
        }
    }

    async fn route(&self, destination: DestinationHash) -> Option<RpcPathEntry> {
        match self
            .settle(EngineCommand::RpcQuery(RpcQuery::Route(destination)))
            .await
        {
            Some(Settlement::RpcQuery(RpcQueryResult::Route(entry))) => entry,
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
    pub fn teardown(self) {
        let _ = self
            .commands
            .send(HostCommand::RemoveInterface { id: self.id });
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

/// Wire one interface onto the running node: build its grant lanes + seam, hand the reactor the
/// `Send` lane halves, and hand the driver the `Send` builder that mints its run future.
/// `supervisor` records it as a fleet member so the driver cascades teardown.
fn attach_interface<I>(
    commands: &UnboundedSender<HostCommand>,
    iface_build: &UnboundedSender<DriverMsg>,
    notify_tx: &UnboundedSender<InterfaceId>,
    interface: I,
    supervisor: Option<InterfaceId>,
) -> AttachedInterface
where
    I: Interface + Send + 'static,
{
    let descriptor = interface.descriptor();
    let id = descriptor.id;
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
        inbound: in_consumer,
        egress: out_producer,
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
}

impl Fleet {
    /// Stand up a fleet member under this supervisor — identical to [`TokioPrnsHandle::add_interface`]
    /// except the member is recorded as this supervisor's, so a supervisor teardown takes it with it.
    pub fn add<I>(&self, interface: I) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        let view = interface.status_view();
        let attached = attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            Some(self.supervisor_id),
        );
        register_status(
            &self.interfaces,
            attached.id(),
            view,
            Membership::FleetMember {
                supervisor_id: self.supervisor_id,
            },
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
                        let _ = commands.send(HostCommand::RemoveInterface { id: member });
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
                        let _ = commands.send(HostCommand::RemoveInterface { id });
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
}

fn register_status(
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    id: InterfaceId,
    view: Option<StatusView>,
    membership: Membership,
) {
    if let (Some(view), Ok(mut map)) = (view, interfaces.lock()) {
        map.insert(id, RegisteredInterface { view, membership });
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
                    ratchet,
                    resource_strategy,
                } => {
                    let held = engine
                        .hold_identity(identity)
                        .expect("recipe identity fits the store");
                    let dest = engine
                        .register_single_destination(
                            &held, app_name, aspects, app_data, proof, ratchet,
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

        if let Some(id) = recipe.transport {
            let identity = IdentityHash::new(*id.as_bytes());
            if engine.set_transport_identity(&identity).is_err() {
                engine.set_transport_id(id);
            }
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
            host: TokioHost::new(),
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

    pub fn respond(&self, responder: RespondToken, body: &[u8]) -> Option<Rtt> {
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
                let event = PrnsEvent::from(journaled);
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

    const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

    fn handle() -> (TokioPrnsHandle, UnboundedReceiver<HostCommand>) {
        let (commands, command_rx) = mpsc::unbounded_channel();
        (TokioPrnsHandle::over(commands), command_rx)
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
            .send(Ok((b"pong".to_vec(), Rtt(42))))
            .unwrap();

        let (data, rtt) = requesting.await.unwrap().unwrap();
        assert_eq!(data, b"pong");
        assert_eq!(rtt, Rtt(42));
    }

    #[tokio::test]
    async fn respond_returns_the_links_round_trip() {
        use crate::routing::links::request::RequestId;
        use crate::runtime::request_router::RespondToken;

        let (handle, _command_rx) = handle();
        let token = RespondToken {
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            rtt: Rtt(99),
        };
        assert_eq!(
            handle.respond(token, b"answer"),
            Some(Rtt(99)),
            "respond surfaces the rtt the request arrived on",
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
                    matches!(command, HostCommand::RemoveInterface { id: removed } if removed == id),
                    "an interface whose run ended on its own deregisters itself"
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
                        rtt: crate::units::Rtt::from_millis(7),
                    })))
                    .expect("the awaiter is still parked");
            }
            _ => panic!("send_single must issue an AwaitedEngine command"),
        }

        assert_eq!(
            send.await.expect("the send task joins"),
            Ok(PacketReceiptDelivered {
                rtt: crate::units::Rtt::from_millis(7),
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
    async fn send_resource_surfaces_a_segment_rejection_and_stops() {
        let (prns, mut command_rx) = handle();
        let total_len = MAX_EFFICIENT_SIZE as u64 + 100;
        let payload = std::vec![7u8; total_len as usize];
        let drainer = tokio::spawn(async move {
            let mut issued = 0u32;
            while let Some(command) = command_rx.recv().await {
                let HostCommand::SendResourceSegment(seg) = command else {
                    panic!("expected a SendResourceSegment command");
                };
                issued += 1;
                seg.completion
                    .send(Settlement::SendResource(Err(
                        SendResourceFailure::RejectedByPeer,
                    )))
                    .expect("the awaiter is still parked");
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
            1,
            "a rejected first segment stops the stream — the second never issues",
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
