use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::engine::{
    CloseLink, CommandId, Delivered, EngineCommand, EngineState, IssuedCommand, SendSingle,
    SendSingleFailure, SendSinglePayload, Settlement,
};
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::reactor::impls::tokio_reactor::{
    self, tokio_grant_lane, AddInterfaceCommand, Egress, HostCommand, HostResourcePayload,
    RespondAnyHostCommand, TokioGrantConsumer, TokioGrantProducer, TokioHost, TokioInterfaceSeam,
};
use crate::reactor::interface_seam::{Interface, MAX_WIRE_FRAME_LEN};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::interface_set::{InterfaceAttach, InterfaceSet};
use super::recipe::PreConfiguredDestination;
use super::request_router::{RespondToken, RouteSet};
use super::tokio_runner::{run_router, RunnerRequest, REQUEST_QUEUE_DEPTH};
use super::{Message, PrnsEvent, PrnsRecipe, SendError};

const LANE_DEPTH: usize = 64;

/// A cloneable, `Send` handle to a running node — the proactive surface. Hand clones to other tasks
/// or threads (each shares one node, so an awaited send on any resolves wherever it settles).
///
/// Every [`CommandId`] is minted from one counter, so a fire-and-forget [`issue`](Self::issue) can
/// never collide with an awaited [`send_single`](Self::send_single) or a runner's respond.
#[derive(Clone)]
pub struct PrnsHandle {
    commands: UnboundedSender<HostCommand>,
    ids: Arc<AtomicU64>,
    notify_tx: UnboundedSender<InterfaceId>,
    iface_build: UnboundedSender<DriverMsg>,
}

impl PrnsHandle {
    #[cfg(test)]
    pub(crate) fn over(commands: UnboundedSender<HostCommand>) -> Self {
        let (notify_tx, _notify_rx) = mpsc::unbounded_channel();
        let (iface_build, _iface_build_rx) = mpsc::unbounded_channel();
        Self {
            commands,
            ids: Arc::new(AtomicU64::new(0)),
            notify_tx,
            iface_build,
        }
    }

    fn mint(&self) -> CommandId {
        CommandId(self.ids.fetch_add(1, Ordering::Relaxed))
    }

    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        let id = self.mint();
        self.commands
            .send(HostCommand::Engine(IssuedCommand { id, command }))
            .ok()?;
        Some(id)
    }

    pub async fn send_single(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<Delivered, SendError<SendSingleFailure>> {
        let payload =
            SendSinglePayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        match self
            .settle(EngineCommand::SendSingle(SendSingle {
                destination,
                payload,
            }))
            .await
        {
            Some(Settlement::SendSingle(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    async fn settle(&self, command: EngineCommand) -> Option<Settlement> {
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

    fn send_response(&self, responder: RespondToken, data: HostResourcePayload) -> bool {
        let id = self.mint();
        self.commands
            .send(HostCommand::RespondAny(RespondAnyHostCommand {
                id,
                link_id: responder.link_id,
                request_id: responder.request_id,
                data,
                compressed_candidate: None,
            }))
            .is_ok()
    }

    pub fn respond(&self, responder: RespondToken, body: &[u8]) -> bool {
        self.send_response(responder, body.to_vec().into())
    }

    pub fn respond_owned(&self, responder: RespondToken, body: std::vec::Vec<u8>) -> bool {
        self.send_response(responder, body.into())
    }

    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.issue(EngineCommand::CloseLink(CloseLink { link_id }))
            .is_some()
    }

    /// Attach an interface to the running node and get a handle to tear it back down. The interface
    /// is wired exactly the way the recipe wires the initial set and begins carrying traffic as soon
    /// as the reactor processes the attach. Grab any per-interface control handle (`.status()`, a
    /// radio's own controls) before calling this, since it takes the interface by value.
    ///
    /// `I: Send` is the host's bargain: the interface rides to the `run` task inside a `Send` builder
    /// closure (handed to the interface driver), which mints its run future there — so the future
    /// itself never has to be `Send`, which is what keeps `!Send` interface bodies legal here. The
    /// reactor only learns the `Send` lane halves, so it stays `Send` and spawnable.
    pub fn add_interface<I>(&self, interface: I) -> AttachedInterface
    where
        I: Interface + Send + 'static,
    {
        attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            None,
        )
    }

    /// Attach an interface supervisor: a node that owns no wire of its own, but runs a discovery loop
    /// and stands up a fleet member per validated connection through the [`Fleet`] handle it is
    /// given. The supervisor itself is no engine interface (no descriptor, no lanes); each member it
    /// adds is an ordinary flat engine interface recorded under it, so tearing the supervisor down
    /// cascades to its whole fleet. The home of the auto-interfaces (auto-wifi, auto-usb). Returns an
    /// [`AttachedSupervisor`], not an [`AttachedInterface`], because a supervisor is not a wire.
    pub fn supervise<S>(&self, supervisor: S) -> AttachedSupervisor
    where
        S: InterfaceSupervisor + Send + 'static,
    {
        let id = InterfaceId::mint();
        let fleet = Fleet {
            supervisor_id: id,
            commands: self.commands.clone(),
            iface_build: self.iface_build.clone(),
            notify_tx: self.notify_tx.clone(),
        };
        let build: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send> =
            Box::new(move || Box::pin(supervisor.run(fleet)));
        let _ = self.iface_build.send(DriverMsg::Add {
            id,
            supervisor: None,
            build,
        });
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
}

impl super::Commands for PrnsHandle {
    fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        self.issue(command)
    }

    async fn send_single(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<Delivered, SendError<SendSingleFailure>> {
        self.send_single(destination, data).await
    }

    fn respond(&self, responder: RespondToken, body: &[u8]) -> bool {
        self.respond(responder, body)
    }

    fn close_link(&self, link_id: LinkId) -> bool {
        self.close_link(link_id)
    }
}

/// A handle to one interface attached at runtime through [`PrnsHandle::add_interface`]: its minted
/// id, and the lever to detach it. Dropping the handle leaves the interface running; only
/// [`teardown`](Self::teardown) (or [`PrnsHandle::remove_interface`]) takes it down.
pub struct AttachedInterface {
    id: InterfaceId,
    commands: UnboundedSender<HostCommand>,
    iface_build: UnboundedSender<DriverMsg>,
}

impl AttachedInterface {
    /// The interface's framework-minted id.
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

/// A handle to a supervisor attached through [`PrnsHandle::supervise`]: its minted id, and the lever
/// to detach it. A supervisor owns no reactor lanes of its own, so tearing it down is a single stop
/// on the driver, which both ends its discovery loop and cascades to deregister every member of its
/// fleet. Dropping the handle leaves the supervisor running.
pub struct AttachedSupervisor {
    id: InterfaceId,
    iface_build: UnboundedSender<DriverMsg>,
}

impl AttachedSupervisor {
    /// The supervisor's framework-minted id.
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
/// `Send` lane halves, and hand the driver the `Send` builder that mints its run future. `supervisor`
/// records it as a fleet member so the driver cascades teardown. Shared by
/// [`PrnsHandle::add_interface`] (no supervisor) and [`Fleet::add`] (this supervisor's id).
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
    let (in_producer, in_consumer) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
    let (out_producer, out_consumer) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
    let seam = TokioInterfaceSeam::new(id, in_producer, notify_tx.clone(), out_consumer);
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

/// A supervisor's lever to stand up fleet members, handed to it by [`PrnsHandle::supervise`]. Each
/// [`add`](Self::add) registers a flat engine interface (its own minted id) recorded as this
/// supervisor's member; the supervisor typically holds the returned [`AttachedInterface`] so it can
/// detach that one member when its link drops.
pub struct Fleet {
    supervisor_id: InterfaceId,
    commands: UnboundedSender<HostCommand>,
    iface_build: UnboundedSender<DriverMsg>,
    notify_tx: UnboundedSender<InterfaceId>,
}

impl Fleet {
    /// Stand up a fleet member under this supervisor — identical to [`PrnsHandle::add_interface`]
    /// except the member is recorded as this supervisor's, so a supervisor teardown takes it with it.
    pub fn add<I>(&self, interface: I) -> AttachedInterface
    where
        I: Interface + Send + 'static,
    {
        attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            Some(self.supervisor_id),
        )
    }
}

/// An interface supervisor: a node that owns no wire of its own, but runs a discovery loop and
/// stands up a fleet member (a real interface) per validated connection through the [`Fleet`] handle
/// it is given. Attached with [`PrnsHandle::supervise`] (e.g. the WiFi/LAN auto-interface: multicast
/// discovery plus a peering ack, then a unicast member per confirmed peer).
#[allow(async_fn_in_trait)]
pub trait InterfaceSupervisor {
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
/// [`PrnsHandle::remove_interface`] can drop it mid-flight; the initial set runs for the node's life.
async fn drive_interfaces(
    initial: std::vec::Vec<Pin<Box<dyn Future<Output = ()>>>>,
    mut messages: UnboundedReceiver<DriverMsg>,
    commands: UnboundedSender<HostCommand>,
) {
    let mut futures: FuturesUnordered<Pin<Box<dyn Future<Output = ()>>>> =
        initial.into_iter().collect();
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
                    }));
                    stops.insert(id, stop_tx);
                }
                Some(DriverMsg::Stop { id }) => {
                    stop_interface(&mut stops, id);
                    supervisor_of.remove(&id);
                    let cascaded: std::vec::Vec<InterfaceId> = supervisor_of
                        .iter()
                        .filter(|(_, supervisor_id)| **supervisor_id == id)
                        .map(|(member, _)| *member)
                        .collect();
                    for member in cascaded {
                        stop_interface(&mut stops, member);
                        supervisor_of.remove(&member);
                        let _ = commands.send(HostCommand::RemoveInterface { id: member });
                    }
                }
                None => open = false,
            },
            _ = futures.next(), if !futures.is_empty() => {}
        }
    }
}

fn stop_interface(stops: &mut HashMap<InterfaceId, oneshot::Sender<()>>, id: InterfaceId) {
    if let Some(stop) = stops.remove(&id) {
        let _ = stop.send(());
    }
}

struct TokioAttach {
    notify_tx: UnboundedSender<InterfaceId>,
    interfaces: std::vec::Vec<InterfaceConfig>,
    inbound: std::vec::Vec<(InterfaceId, TokioGrantConsumer<MAX_WIRE_FRAME_LEN>)>,
    egress_lanes: std::vec::Vec<(InterfaceId, TokioGrantProducer<MAX_WIRE_FRAME_LEN>)>,
    runs: std::vec::Vec<Pin<Box<dyn Future<Output = ()>>>>,
}

impl InterfaceAttach for TokioAttach {
    fn attach<I: Interface + 'static>(&mut self, interface: I) {
        let descriptor = interface.descriptor();
        let id = descriptor.id;
        let (in_producer, in_consumer) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
        let (out_producer, out_consumer) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
        self.interfaces.push(descriptor);
        self.inbound.push((id, in_consumer));
        self.egress_lanes.push((id, out_producer));
        let seam = TokioInterfaceSeam::new(id, in_producer, self.notify_tx.clone(), out_consumer);
        self.runs.push(Box::pin(interface.run(seam)));
    }
}

/// A node on the tokio host — the loop side of the runtime. Built from a [`PrnsRecipe`] with
/// [`new`](Self::new) (synchronous: it wires the engine and spawns each interface), then driven by
/// [`run`](Self::run) (the reactor + the request runner, joined). Hold [`handle`](Self::handle)
/// clones to drive it from other tasks/threads while `run` owns the loop.
pub struct Prns<St, R, F, S: StorageLayout> {
    handle: PrnsHandle,
    host: TokioHost,
    engine: EngineState<S>,
    interfaces: std::vec::Vec<InterfaceConfig>,
    inbound: std::vec::Vec<(InterfaceId, TokioGrantConsumer<MAX_WIRE_FRAME_LEN>)>,
    egress_lanes: std::vec::Vec<(InterfaceId, TokioGrantProducer<MAX_WIRE_FRAME_LEN>)>,
    notify_rx: UnboundedReceiver<InterfaceId>,
    command_rx: UnboundedReceiver<HostCommand>,
    iface_build_rx: UnboundedReceiver<DriverMsg>,
    iface_runs: std::vec::Vec<Pin<Box<dyn Future<Output = ()>>>>,
    state: St,
    on_event: F,
    _routes: PhantomData<R>,
}

impl<St, R, F, S: StorageLayout> Prns<St, R, F, S>
where
    R: RouteSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
{
    /// Stand a node up from `recipe` on the storage layout it names (`recipe.storage`): assemble
    /// the engine (transport role, destinations, the routes' request handlers), then attach and
    /// spawn every interface. Synchronous — only [`run`](Self::run) awaits.
    #[allow(clippy::expect_used)]
    pub fn new<'a, D, I>(recipe: PrnsRecipe<D, St, R, F, I, S>) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'a>>,
        I: InterfaceSet,
    {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (iface_build_tx, iface_build_rx) = mpsc::unbounded_channel();
        let handle = PrnsHandle {
            commands: command_tx,
            ids: Arc::new(AtomicU64::new(0)),
            notify_tx: notify_tx.clone(),
            iface_build: iface_build_tx,
        };

        let mut engine = EngineState::<S>::default();
        if let Some(id) = recipe.transport {
            engine.set_transport_id(id);
        }
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
                } => {
                    let held = engine
                        .hold_identity(identity)
                        .expect("recipe identity fits the store");
                    let dest = engine
                        .register_single_destination(
                            &held, app_name, aspects, app_data, proof, ratchet,
                        )
                        .expect("recipe single destination is valid");
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

        let mut attach = TokioAttach {
            notify_tx,
            interfaces: std::vec::Vec::new(),
            inbound: std::vec::Vec::new(),
            egress_lanes: std::vec::Vec::new(),
            runs: std::vec::Vec::new(),
        };
        recipe.interfaces.attach_all(&mut attach);

        Prns {
            handle,
            host: TokioHost::new(),
            engine,
            interfaces: attach.interfaces,
            inbound: attach.inbound,
            egress_lanes: attach.egress_lanes,
            notify_rx,
            command_rx,
            iface_build_rx,
            iface_runs: attach.runs,
            state: recipe.app_state,
            on_event: recipe.on_event,
            _routes: PhantomData,
        }
    }

    /// A `Send + Clone` handle for other tasks/threads to drive the node while [`run`](Self::run)
    /// owns the loop.
    #[must_use]
    pub fn handle(&self) -> PrnsHandle {
        self.handle.clone()
    }

    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        self.handle.issue(command)
    }

    pub async fn send_single(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<Delivered, SendError<SendSingleFailure>> {
        self.handle.send_single(destination, data).await
    }

    pub fn respond(&self, responder: RespondToken, body: &[u8]) -> bool {
        self.handle.respond(responder, body)
    }

    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.handle.close_link(link_id)
    }

    /// Drive the node until it stops (the reactor loops indefinitely, so in practice forever). The
    /// reactor and the request runner run joined: every inbound request is forked to the runner
    /// (which answers it through the routes), while that event — and every other — reaches the
    /// recipe's `on_event` with shared `&state`, zero-copy.
    pub async fn run(self) {
        let Prns {
            handle,
            host,
            engine,
            interfaces,
            inbound,
            egress_lanes,
            notify_rx,
            command_rx,
            iface_build_rx,
            iface_runs,
            state,
            mut on_event,
            _routes,
        } = self;
        let egress = Egress::new(egress_lanes);
        let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let reactor = tokio_reactor::run(
            engine,
            interfaces,
            std::vec::Vec::new(),
            host,
            notify_rx,
            inbound,
            command_rx,
            egress,
            |journaled| {
                let event = PrnsEvent::from(journaled);
                if let PrnsEvent::Message(Message::Request {
                    link_id,
                    request_id,
                    path_hash,
                    requested_at,
                    data,
                }) = &event
                {
                    let _ = req_tx.try_send(RunnerRequest {
                        link_id: *link_id,
                        request_id: *request_id,
                        path_hash: *path_hash,
                        requested_at: *requested_at,
                        data: data.to_vec(),
                    });
                }
                on_event(event, &state);
            },
        );
        let driver_commands = handle.commands.clone();
        tokio::join!(
            reactor,
            run_router::<St, R>(&state, req_rx, handle),
            drive_interfaces(iface_runs, iface_build_rx, driver_commands),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MAX_SEND_SINGLE_PLAINTEXT_LEN;

    const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

    fn handle() -> (PrnsHandle, UnboundedReceiver<HostCommand>) {
        let (commands, command_rx) = mpsc::unbounded_channel();
        (PrnsHandle::over(commands), command_rx)
    }

    #[tokio::test]
    async fn payload_beyond_the_mdu_is_rejected_before_the_wire() {
        let (prns, _command_rx) = handle();
        let oversize = [0u8; MAX_SEND_SINGLE_PLAINTEXT_LEN + 1];
        assert_eq!(
            prns.send_single(PEER, &oversize).await,
            Err(SendError::PayloadTooLarge),
        );
    }

    #[tokio::test]
    async fn a_send_on_a_stopped_node_settles_as_node_stopped() {
        let (prns, command_rx) = handle();
        drop(command_rx);
        assert_eq!(
            prns.send_single(PEER, b"ping").await,
            Err(SendError::NodeStopped),
        );
    }

    #[tokio::test]
    async fn an_awaited_send_issues_the_completion_carrying_command() {
        let (prns, mut command_rx) = handle();
        let issuer = prns.clone();
        let send = tokio::spawn(async move { issuer.send_single(PEER, b"ping").await });

        match command_rx.recv().await.expect("the command was issued") {
            HostCommand::AwaitedEngine { issued, completion } => {
                assert!(matches!(issued.command, EngineCommand::SendSingle(_)));
                completion
                    .send(Settlement::SendSingle(Ok(Delivered {
                        rtt: crate::units::Rtt::from_millis(7),
                    })))
                    .expect("the awaiter is still parked");
            }
            _ => panic!("send_single must issue an AwaitedEngine command"),
        }

        assert_eq!(
            send.await.expect("the send task joins"),
            Ok(Delivered {
                rtt: crate::units::Rtt::from_millis(7),
            }),
        );
    }

    #[test]
    fn the_commands_trait_dispatches_to_the_handle() {
        use crate::routing::links::LinkId;
        use crate::runtime::Commands;

        let (prns, mut command_rx) = handle();
        let queued = Commands::close_link(&prns, LinkId::new([3; 16]));
        assert!(
            queued,
            "the trait method reaches the handle and queues the close"
        );
        assert!(
            matches!(command_rx.try_recv(), Ok(HostCommand::Engine(_))),
            "dispatched through Commands, the close rode the channel"
        );
    }
}
