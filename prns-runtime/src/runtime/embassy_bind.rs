//! The embassy command surface — the embedded twin of `TokioPrnsHandle`, kept warm while the neutral
//! `Prns`/[`PrnsRecipe`](super::PrnsRecipe) entry point is dialed in on the tokio side. The
//! embassy *runner* (the borrow-bundle that drove the reactor over `static` channels) is parked; what
//! survives here are the two pieces an embedded node reattaches to once the neutral runner is ported:
//! the handle and its completion store.
//!
//! The handle is [`EmbassyPrnsHandle`] — built over the command channel's `Sender` and a
//! [`CompletionPool`] the app provides as a `static` (the embedded stand-in for tokio's per-command
//! oneshot, since no_std has no ownable completion to ride the command). The app keeps the `Sender`
//! wrapped in the handle and holds the matching `Receiver` and the same pool borrow for the runner —
//! the channel and pool living in static storage instead of the heap.

use core::cell::RefCell;
use core::future::Future;
use core::marker::PhantomData;

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::signal::Signal;
use heapless::Vec as HeaplessVec;
use portable_atomic::{AtomicU64, Ordering};

use crate::engine::{
    CloseLink, CommandId, Delivered, EngineCommand, EngineState, FanTarget, IssuedCommand,
    Journaled, Respond, RespondData, SendSingle, SendSingleFailure, SendSinglePayload, Settlement,
};
use crate::identity::IdentityHash;
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::reactor::grant::{GrantConsumer, GrantProducer};
use crate::reactor::impls::embassy_reactor::{
    run_pooled, EmbassyGrantConsumer, EmbassyGrantProducer, InterfaceLifecycle, PooledEgress,
    PooledWiring,
};
use crate::reactor::Host;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::recipe::PreConfiguredDestination;
use super::request_router::{RespondToken, RouteSet};
use super::{InterfaceCountSink, PrnsEvent, PrnsRecipe, SendError};

/// The free-slot sentinel — no real [`CommandId`] reaches `u64::MAX` (the handle mints from zero).
const NO_AWAITER: u64 = u64::MAX;

/// A fixed pool of completion slots an embassy app provides as a `static`, alongside its command
/// channel — the embedded twin of tokio's per-command oneshot. An awaited send claims a slot, parks
/// on its [`Signal`], and the binding fires that slot by command id when the engine settles; the
/// send future releases its slot on drop, so a cancelled send can never wake a later claimant. `N`
/// bounds the awaited sends in flight at once. All bookkeeping is serialized under one blocking
/// mutex, so claim, release, and settle never race even across cores — and `settle` signals while
/// holding it, closing the window where a freed slot could be reused mid-fire.
pub struct CompletionPool<M: RawMutex, const N: usize> {
    next_id: AtomicU64,
    awaited: BlockingMutex<M, RefCell<[u64; N]>>,
    slots: [Signal<M, Settlement>; N],
}

impl<M: RawMutex, const N: usize> Default for CompletionPool<M, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex, const N: usize> CompletionPool<M, N> {
    /// A pool with every slot free — `const`, so it lives in a `static`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            awaited: BlockingMutex::new(RefCell::new([NO_AWAITER; N])),
            slots: [const { Signal::new() }; N],
        }
    }

    fn mint(&self) -> CommandId {
        CommandId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Reserve a free slot for `id`, clearing any stale signal first. `None` when the pool is full —
    /// the caller already has more awaited sends in flight than `N`.
    fn claim(&self, id: CommandId) -> Option<usize> {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            let slot = awaited.iter().position(|&a| a == NO_AWAITER)?;
            self.slots[slot].reset();
            awaited[slot] = id.0;
            Some(slot)
        })
    }

    /// Free `slot` only if it still belongs to `id` — the send future's drop path. After a settle
    /// has cleared the slot (and another send may have claimed it), this is a no-op, so a late drop
    /// can't clobber a newer claimant.
    fn release(&self, slot: usize, id: CommandId) {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            if awaited[slot] == id.0 {
                awaited[slot] = NO_AWAITER;
                self.slots[slot].reset();
            }
        });
    }

    /// Hand `settlement` to the slot awaiting `id`, if any, and report whether it fired — the
    /// runner drops a fired settlement from the event stream so an awaited command resolves once,
    /// through its `.await`, not also through `on_event`. Signals under the lock so a concurrent
    /// release/claim can't slip the slot out from under the wakeup.
    fn settle(&self, id: CommandId, settlement: Settlement) -> bool {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            match awaited.iter().position(|&a| a == id.0) {
                Some(slot) => {
                    awaited[slot] = NO_AWAITER;
                    self.slots[slot].signal(settlement);
                    true
                }
                None => false,
            }
        })
    }

    async fn parked(&self, slot: usize) -> Settlement {
        self.slots[slot].wait().await
    }
}

/// The embassy command handle — the embedded twin of `TokioPrnsHandle`. It
/// holds the command channel's [`Sender`] and a borrow of the app's [`CompletionPool`], and is
/// `Copy`, so any task can drive the node through it. Every [`CommandId`] is minted from the pool's
/// one counter, so the app never picks ids and a fire-and-forget [`issue`](Self::issue) can't
/// collide with an awaited [`send_single`](Self::send_single).
pub struct EmbassyPrnsHandle<'a, M: RawMutex, const COMMANDS: usize, const N: usize> {
    commands: Sender<'a, M, IssuedCommand, COMMANDS>,
    pool: &'a CompletionPool<M, N>,
}

impl<M: RawMutex, const COMMANDS: usize, const N: usize> Clone
    for EmbassyPrnsHandle<'_, M, COMMANDS, N>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const COMMANDS: usize, const N: usize> Copy
    for EmbassyPrnsHandle<'_, M, COMMANDS, N>
{
}

impl<'a, M: RawMutex, const COMMANDS: usize, const N: usize> EmbassyPrnsHandle<'a, M, COMMANDS, N> {
    /// Pair the command channel's sender with the completion pool — the app holds both as `static`s
    /// and passes the matching [`CompletionPool`] reference to the runner too.
    #[must_use]
    pub fn new(
        commands: Sender<'a, M, IssuedCommand, COMMANDS>,
        pool: &'a CompletionPool<M, N>,
    ) -> Self {
        Self { commands, pool }
    }

    /// Queue an engine command and return the [`CommandId`] it was minted under — watch the event
    /// stream for the settlement tagged with it. `None` if the bounded command lane is full. The
    /// fire-and-forget escape hatch; to await the outcome, prefer [`send_single`](Self::send_single).
    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        let id = self.pool.mint();
        self.commands.try_send(IssuedCommand { id, command }).ok()?;
        Some(id)
    }

    /// Send one Single data packet to `destination` and await its delivery proof — the embedded peer
    /// of `TokioPrnsHandle::send_single`. Claims a pool slot,
    /// parks on it until the engine settles, and frees the slot on every exit, cancellation
    /// included. `Err(SendError::Busy)` when more awaited sends are in flight than the pool's `N`.
    pub async fn send_single(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<Delivered, SendError<SendSingleFailure>> {
        let payload =
            SendSinglePayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        let id = self.pool.mint();
        let slot = self.pool.claim(id).ok_or(SendError::Busy)?;
        let _guard = SlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: EngineCommand::SendSingle(SendSingle {
                    destination,
                    payload,
                }),
            })
            .map_err(|_| SendError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::SendSingle(result) => result.map_err(SendError::Failed),
            _ => Err(SendError::NodeStopped),
        }
    }

    /// Answer a request with `body` as a single RESPONSE packet — the request runner's path. Embedded
    /// responds inline, so a `body` past the link MDU is refused here (returns `false`); the host
    /// auto-upgrades to a resource instead.
    pub fn respond(&self, responder: RespondToken, body: &[u8]) -> bool {
        match RespondData::from_slice(body) {
            Ok(data) => self.respond_owned(responder, data),
            Err(_) => false,
        }
    }

    /// Answer a request by moving a prebuilt [`RespondData`] in — the request runner's path, one copy
    /// fewer than [`respond`](Self::respond) since the handler already filled a `RespondData` grant.
    /// Returns `false` once the command lane is full. The embedded twin of `TokioPrnsHandle::respond_owned`.
    pub fn respond_owned(&self, responder: RespondToken, data: RespondData) -> bool {
        self.issue(EngineCommand::Respond(Respond {
            link_id: responder.link_id,
            request_id: responder.request_id,
            data,
        }))
        .is_some()
    }

    /// Sever an active link. Returns `false` if the command lane is full.
    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.issue(EngineCommand::CloseLink(CloseLink { link_id }))
            .is_some()
    }
}

/// Frees a claimed completion slot when its awaited send finishes or is cancelled. Release is
/// guarded by the awaited id, so a late drop after the settle already reused the slot is a no-op.
struct SlotGuard<'a, M: RawMutex, const N: usize> {
    pool: &'a CompletionPool<M, N>,
    slot: usize,
    id: CommandId,
}

impl<M: RawMutex, const N: usize> Drop for SlotGuard<'_, M, N> {
    fn drop(&mut self) {
        self.pool.release(self.slot, self.id);
    }
}

impl<M: RawMutex, const COMMANDS: usize, const N: usize> super::PrnsApi
    for EmbassyPrnsHandle<'_, M, COMMANDS, N>
{
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

/// The reactor-side wiring an embassy node runs on: the pool's inbound consumers and the egress,
/// the three channel receivers the reactor parks on, and the command handle the app drives it
/// through. The board declares the matching `static` channels and hands this bundle to
/// [`Prns::new`]; the interface-side seam halves (and the fleet senders) come off the same pool
/// separately, so the node owns only the reactor's half.
pub struct ReactorPlumbing<
    M,
    const SLOT: usize,
    const IFACES: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
    const COMPLETIONS: usize,
> where
    M: RawMutex + 'static,
{
    inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, SLOT>), IFACES>,
    egress: PooledEgress<M, SLOT, IFACES>,
    notify: Receiver<'static, M, InterfaceId, NOTIFY>,
    commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
    lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
    handle: EmbassyPrnsHandle<'static, M, COMMANDS, COMPLETIONS>,
}

impl<
        M: RawMutex + 'static,
        const SLOT: usize,
        const IFACES: usize,
        const NOTIFY: usize,
        const COMMANDS: usize,
        const LIFECYCLE: usize,
        const COMPLETIONS: usize,
    > ReactorPlumbing<M, SLOT, IFACES, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>
{
    /// Bundle the reactor's half of the pool. `inbound` and `egress` carry every slot's
    /// reactor-side endpoint (free slots are tagged by the pool); the receivers are the matching
    /// halves of the node's three `static` channels; `handle` pairs the command sender with the
    /// completion pool.
    #[must_use]
    pub fn new(
        inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, SLOT>), IFACES>,
        egress: PooledEgress<M, SLOT, IFACES>,
        notify: Receiver<'static, M, InterfaceId, NOTIFY>,
        commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
        lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
        handle: EmbassyPrnsHandle<'static, M, COMMANDS, COMPLETIONS>,
    ) -> Self {
        Self {
            inbound,
            egress,
            notify,
            commands,
            lifecycle,
            handle,
        }
    }
}

/// A node on an embassy host: the no_std twin of the tokio `Prns`, built from a
/// [`PrnsRecipe`] over a board-declared static interface pool ([`ReactorPlumbing`]). The recipe
/// still names the node (its transport role, destinations, and routes); the wires are attached
/// explicitly because the board owns their `static` storage. [`handle`](Self::handle) hands out the
/// command surface, [`activate`](Self::activate) stands up a top-level interface on a pool slot, and
/// [`run`](Self::run) joins the reactor with the caller's interface/supervisor drive — a plain
/// embassy `join`, the shape an embedded app reaches for.
pub struct Prns<
    St,
    R,
    F,
    S,
    H,
    M,
    const SLOT: usize,
    const IFACES: usize,
    const MAX_IFACES: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
    const COMPLETIONS: usize,
> where
    S: StorageLayout,
    M: RawMutex + 'static,
{
    engine: EngineState<S>,
    inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, SLOT>), IFACES>,
    egress: PooledEgress<M, SLOT, IFACES>,
    notify: Receiver<'static, M, InterfaceId, NOTIFY>,
    commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
    lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
    handle: EmbassyPrnsHandle<'static, M, COMMANDS, COMPLETIONS>,
    host: H,
    initial: HeaplessVec<InterfaceConfig, MAX_IFACES>,
    state: St,
    on_event: F,
    _routes: PhantomData<R>,
    store: Option<&'static dyn InterfaceCountSink>,
}

impl<
        St,
        R,
        F,
        S,
        H,
        M,
        const SLOT: usize,
        const IFACES: usize,
        const MAX_IFACES: usize,
        const NOTIFY: usize,
        const COMMANDS: usize,
        const LIFECYCLE: usize,
        const COMPLETIONS: usize,
    > Prns<St, R, F, S, H, M, SLOT, IFACES, MAX_IFACES, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>
where
    R: RouteSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
    H: Host,
    M: RawMutex + 'static,
{
    /// Stand a node up from `recipe` over the board's `plumbing` and `host` (its clock + entropy):
    /// assemble the engine (transport role, destinations, the routes' request handlers) exactly as
    /// the tokio `Prns::new` does, then hold the reactor's half ready. No
    /// interface is wired yet — [`activate`](Self::activate) names the top-level wires and the
    /// supervisor drive names the rest, both at [`run`](Self::run).
    #[allow(clippy::expect_used)]
    pub fn new<'d, D, I>(
        recipe: PrnsRecipe<D, St, R, F, I, S>,
        plumbing: ReactorPlumbing<M, SLOT, IFACES, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>,
        host: H,
        initial: HeaplessVec<InterfaceConfig, MAX_IFACES>,
    ) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
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

        Prns {
            engine,
            inbound: plumbing.inbound,
            egress: plumbing.egress,
            notify: plumbing.notify,
            commands: plumbing.commands,
            lifecycle: plumbing.lifecycle,
            handle: plumbing.handle,
            host,
            initial,
            state: recipe.app_state,
            on_event: recipe.on_event,
            _routes: PhantomData,
            store: None,
        }
    }

    pub fn set_interface_store(&mut self, store: &'static dyn InterfaceCountSink) {
        self.store = Some(store);
    }

    /// The command surface for this node — the embedded twin of `TokioPrnsHandle`. `Copy`, so any task
    /// can drive the node through it while [`run`](Self::run) owns the loop.
    #[must_use]
    pub fn handle(&self) -> EmbassyPrnsHandle<'static, M, COMMANDS, COMPLETIONS> {
        self.handle
    }

    /// Stand a top-level interface up on pool `slot` and hand back the interface-side seam to drive
    /// it on. The board pairs this with the same `slot`'s interface-side halves off the pool; the
    /// returned descriptor's id routes inbound and egress to this slot from the moment
    /// [`run`](Self::run) starts. The home of the always-present wires (the board's USB-auto);
    /// the supervisor's peers come up later through its [`Fleet`].
    pub fn activate(&mut self, slot: usize, config: InterfaceConfig) {
        if let Some(entry) = self.inbound.get_mut(slot) {
            entry.0 = config.id;
            self.egress.activate(slot, config.id);
            let _ = self.initial.push(config);
        }
    }

    /// Register a supervisor's shared lane on pool `slot`, keyed by the supervisor's id. Unlike
    /// [`activate`](Self::activate) this adds no engine interface — the supervisor itself never
    /// carries routes; its members do, each added later through the [`Fleet`]. Inbound and egress for
    /// every child of the supervisor's kind route to this one lane (see `lane_serves`).
    pub fn activate_fleet(&mut self, slot: usize, supervisor: InterfaceId) {
        if let Some(entry) = self.inbound.get_mut(slot) {
            entry.0 = supervisor;
            self.egress.activate(slot, supervisor);
        }
    }

    /// Drive the node until the executor drops it: the reactor (over its slot pool) joined with the
    /// caller's `drive` — the interface and supervisor run-futures, joined however the board likes.
    /// Every engine event reaches the recipe's `on_event` with shared `&state`, zero-copy.
    pub async fn run(self, drive: impl Future<Output = ()>) {
        let Prns {
            mut engine,
            mut inbound,
            mut egress,
            notify,
            commands,
            lifecycle,
            handle,
            mut host,
            initial,
            state,
            mut on_event,
            _routes,
            store,
        } = self;
        let reactor = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                initial: &initial,
                inbound: &mut inbound,
                egress: &mut egress,
                notify,
                commands,
                lifecycle,
            },
            |journaled| {
                if let Journaled::CommandSettled { id, settlement } = &journaled {
                    if handle.pool.settle(*id, settlement.clone()) {
                        return;
                    }
                }
                on_event(PrnsEvent::from(journaled), &state);
            },
            |_| false,
            store,
        );
        join(reactor, drive).await;
    }

    /// Drive only the reactor — no interface drive joined. The board runs its interfaces and
    /// supervisors wherever it likes (a separate task, or a separate *core*: the reactor↔interface
    /// seam is all `CriticalSectionRawMutex` channels, so the engine can own one core while the I/O
    /// owns another, genuine parallelism with no shared state but the lanes). The single-core
    /// convenience is [`run`](Self::run), which joins the reactor with the drive on one task.
    pub async fn run_reactor(&mut self) {
        let Prns {
            engine,
            inbound,
            egress,
            notify,
            commands,
            lifecycle,
            handle,
            host,
            initial,
            state,
            on_event,
            _routes,
            store,
        } = self;
        run_pooled(
            engine,
            host,
            PooledWiring {
                initial: &*initial,
                inbound,
                egress,
                notify: *notify,
                commands: *commands,
                lifecycle: *lifecycle,
            },
            |journaled| {
                if let Journaled::CommandSettled { id, settlement } = &journaled {
                    if handle.pool.settle(*id, settlement.clone()) {
                        return;
                    }
                }
                on_event(PrnsEvent::from(journaled), state);
            },
            |_| false,
            *store,
        )
        .await;
    }
}

/// One member slot's reactor wire, lent to a supervisor: the inbound producer it funnels frames it
/// receives off the medium into, the outbound consumer it drains the reactor's directives off, and
/// the notify funnel it announces each inbound commit on — tagged with the member's *current* id, so
/// the supervisor decides the notify tag per peer (the slot's id changes as peers come and go). The
/// endpoints are permanent, so the slot reuses for the next peer with no re-split.
pub struct MemberWire<M: RawMutex + 'static, const SLOT: usize, const NOTIFY: usize> {
    pub inbound: EmbassyGrantProducer<'static, M, SLOT>,
    pub outbound: EmbassyGrantConsumer<'static, M, SLOT>,
    pub notify: Sender<'static, M, InterfaceId, NOTIFY>,
    pub outbound_wake: &'static Signal<M, ()>,
}

/// A supervisor's lever onto the node's reactor — the embedded twin of the host `Fleet`, minus the
/// spawn. The whole fleet shares **one** [`MemberWire`]: the supervisor funnels every peer's inbound
/// frame into it tagged with that peer's id ([`deliver_inbound`](Self::deliver_inbound)) and drains
/// the reactor's outbound frames off it tagged with their target peer ([`next_outbound`](Self::next_outbound)),
/// so the reactor's kind-routing demuxes a whole fleet over one lane-pair instead of one per peer.
/// A confirmed peer becomes a distinct engine interface with [`register_member`](Self::register_member)
/// and goes away with [`deregister_member`](Self::deregister_member) — each costs only a descriptor,
/// never a lane. The supervisor's own loop drives this, so no future is ever spawned.
pub struct Fleet<
    M: RawMutex + 'static,
    const SLOT: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
> {
    wire: MemberWire<M, SLOT, NOTIFY>,
    lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
}

impl<M: RawMutex + 'static, const SLOT: usize, const NOTIFY: usize, const LIFECYCLE: usize>
    Fleet<M, SLOT, NOTIFY, LIFECYCLE>
{
    /// Build a fleet over its one shared `wire` (the interface-side halves of the supervisor's lane,
    /// whose reactor side the node's [`ReactorPlumbing`] holds) and the `lifecycle` sender whose
    /// receiver the reactor parks on.
    #[must_use]
    pub fn new(
        wire: MemberWire<M, SLOT, NOTIFY>,
        lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
    ) -> Self {
        Self { wire, lifecycle }
    }

    /// Register a confirmed peer as a distinct engine interface under `config` (the peer's
    /// medium-derived id and descriptor): the engine forwards to it at once, its frames routing to
    /// this fleet's one lane by kind. `false` if the lifecycle lane is full.
    pub fn register_member(&self, config: InterfaceConfig) -> bool {
        self.lifecycle
            .try_send(InterfaceLifecycle::Add { config })
            .is_ok()
    }

    /// Drop the member with this id: the reactor culls its routes and forgets its descriptor. The
    /// shared lane stays for the rest of the fleet. `false` if the lifecycle lane is full.
    pub fn deregister_member(&self, id: InterfaceId) -> bool {
        self.lifecycle
            .try_send(InterfaceLifecycle::Remove { id })
            .is_ok()
    }

    /// Funnel one inbound frame from peer `child` into the shared lane, tagged so the reactor ingests
    /// it as `child`'s — then announce the commit on the notify funnel. `false` if the lane is
    /// momentarily full (the frame is dropped, as a full lane does), so a slow reactor never stalls
    /// the medium read.
    pub fn deliver_inbound(&mut self, child: InterfaceId, bytes: &[u8]) -> bool {
        let Some(grant) = self.wire.inbound.try_grant() else {
            return false;
        };
        grant.fill_for(child, bytes);
        self.wire.inbound.commit();
        let _ = self.wire.notify.try_send(child);
        true
    }

    /// Park until the reactor grants an outbound frame, returning a copy of it plus how to deliver
    /// it: the peer id it targets (the slot's tag) for a direct send, or `Some(fan)` when it is a
    /// fleet broadcast the supervisor fans across the members the [`FanTarget`] selects. The frame
    /// is copied out (sized `OUT`, the medium's frame ceiling) rather than borrowed, so the returned
    /// value owns nothing of the fleet — that lets it ride a `select` arm beside the supervisor's
    /// other fleet uses without a borrow clash. The prior peek is released first, and the copied
    /// slot is released before returning, so the depth-1 lane is free for the reactor's next frame
    /// the instant this one is in hand, and each frame is carried exactly once.
    pub async fn next_outbound<const OUT: usize>(
        &mut self,
    ) -> (InterfaceId, Option<FanTarget>, HeaplessVec<u8, OUT>) {
        self.wire.outbound.release();
        let slot = self.wire.outbound.peek().await;
        let id = slot.interface_id;
        let fan = slot.fan;
        let mut bytes: HeaplessVec<u8, OUT> = HeaplessVec::new();
        let _ = bytes.extend_from_slice(slot.frame());
        self.wire.outbound.release();
        (id, fan, bytes)
    }

    /// Park until the reactor commits an outbound frame onto this fleet's shared lane. The reactor
    /// signals this on every commit, so a supervisor waiting here is roused across the task boundary
    /// without depending on the lane's own consumer waker — the inbound funnel's mirror image. On
    /// wake, drain with [`try_next_outbound`](Self::try_next_outbound) until it yields `None`.
    pub async fn outbound_ready(&self) {
        self.wire.outbound_wake.wait().await;
    }

    /// Take the next outbound frame without parking — `None` when the lane is momentarily empty. The
    /// copy/release contract matches [`next_outbound`](Self::next_outbound): the slot is freed before
    /// returning, so the depth-1 lane refills at once and each frame is carried exactly once. The
    /// signal-then-drain pair ([`outbound_ready`](Self::outbound_ready) then this in a loop) replaces
    /// awaiting the lane directly, so several frames committed before the supervisor runs all flush.
    pub fn try_next_outbound<const OUT: usize>(
        &mut self,
    ) -> Option<(InterfaceId, Option<FanTarget>, HeaplessVec<u8, OUT>)> {
        let slot = self.wire.outbound.try_peek()?;
        let id = slot.interface_id;
        let fan = slot.fan;
        let mut bytes: HeaplessVec<u8, OUT> = HeaplessVec::new();
        let _ = bytes.extend_from_slice(slot.frame());
        self.wire.outbound.release();
        Some((id, fan, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{hx, RAW_ANNOUNCE, TEST_TRANSPORT_ID};
    use crate::interfaces::{
        AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
        InterfaceKind, InterfaceMode, TransportCapability,
    };
    use crate::reactor::impls::embassy_reactor::{leaked_grant_lane, EmbassyHost};
    use crate::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
    use crate::runtime::Diagnostic;
    use crate::storage::GrowableHeap;
    use crate::units::Rtt;
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use embassy_time::{with_timeout, Duration, Timer};
    use std::rc::Rc;

    type Pool<const N: usize> = CompletionPool<CriticalSectionRawMutex, N>;

    fn delivered(ms: u64) -> Settlement {
        Settlement::SendSingle(Ok(Delivered {
            rtt: Rtt::from_millis(ms),
        }))
    }

    #[test]
    fn the_pool_mints_a_distinct_id_each_time() {
        let pool: Pool<2> = CompletionPool::new();
        assert_eq!(pool.mint(), CommandId(0));
        assert_eq!(pool.mint(), CommandId(1));
        assert_eq!(pool.mint(), CommandId(2));
    }

    #[test]
    fn the_pool_bounds_concurrent_awaited_sends() {
        let pool: Pool<2> = CompletionPool::new();
        let first = pool.claim(CommandId(0));
        let second = pool.claim(CommandId(1));
        assert!(first.is_some() && second.is_some());
        assert_ne!(first, second);
        assert_eq!(
            pool.claim(CommandId(2)),
            None,
            "a full pool refuses a claim"
        );
    }

    #[test]
    fn settle_wakes_only_the_slot_awaiting_that_id() {
        let pool: Pool<3> = CompletionPool::new();
        pool.claim(CommandId(10));
        pool.claim(CommandId(11));
        pool.claim(CommandId(12));
        assert!(
            !pool.settle(CommandId(99), delivered(1)),
            "no slot awaits 99"
        );
        assert!(pool.settle(CommandId(11), delivered(1)));
        assert!(pool.settle(CommandId(10), delivered(1)));
        assert!(pool.settle(CommandId(12), delivered(1)));
    }

    #[test]
    fn a_settled_slot_frees_for_reuse() {
        let pool: Pool<1> = CompletionPool::new();
        let id = CommandId(0);
        assert!(pool.claim(id).is_some());
        assert_eq!(pool.claim(CommandId(1)), None, "full while id awaits");
        assert!(pool.settle(id, delivered(1)));
        assert!(
            pool.claim(CommandId(1)).is_some(),
            "the slot frees once settled"
        );
    }

    #[test]
    fn a_cancelled_await_releases_its_slot_and_ignores_a_late_settlement() {
        let pool: Pool<1> = CompletionPool::new();
        let id = CommandId(0);
        let slot = pool.claim(id).expect("a slot");
        pool.release(slot, id);
        assert!(
            !pool.settle(id, delivered(1)),
            "a settlement for a released await fires nothing"
        );
        assert!(
            pool.claim(CommandId(1)).is_some(),
            "the released slot is reusable"
        );
    }

    #[test]
    fn a_late_release_never_clobbers_a_newer_claimant() {
        let pool: Pool<1> = CompletionPool::new();
        let first = CommandId(0);
        let slot = pool.claim(first).expect("a slot");
        assert!(pool.settle(first, delivered(1)));

        let second = CommandId(1);
        assert_eq!(pool.claim(second), Some(slot), "the same slot is reused");
        pool.release(slot, first);
        assert!(
            pool.settle(second, delivered(2)),
            "the stale release left the new claimant intact"
        );
    }

    type Mtx = CriticalSectionRawMutex;
    const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;

    fn descriptor(id: InterfaceId) -> InterfaceConfig {
        InterfaceConfig {
            id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            bitrate_bps: None,
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
        }
    }

    fn leak<T>(value: T) -> &'static T {
        std::boxed::Box::leak(std::boxed::Box::new(value))
    }

    #[test]
    fn next_outbound_releases_the_copied_grant_so_the_depth_one_lane_refills() {
        use crate::reactor::grant::AnyGrantProducer;

        let (inbound, _inbound_rx) = leaked_grant_lane::<SLOT>(1);
        let (mut outbound_tx, outbound) = leaked_grant_lane::<SLOT>(1);
        let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
        let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
        let mut fleet: Fleet<Mtx, SLOT, 1, 1> = Fleet::new(
            MemberWire {
                inbound,
                outbound,
                notify: notify.sender(),
                outbound_wake: leak(Signal::new()),
            },
            lifecycle.sender(),
        );

        assert!(outbound_tx.try_fill_frame_fan(FanTarget::All, b"one"));
        let (_, fan, frame) = block_on(fleet.next_outbound::<SLOT>());
        assert_eq!(fan, Some(FanTarget::All));
        assert_eq!(frame.as_slice(), b"one");

        assert!(
            outbound_tx.try_fill_frame_fan(FanTarget::All, b"two"),
            "the depth-1 lane must accept the next frame the instant next_outbound copied the last"
        );
        let (_, fan, frame) = block_on(fleet.next_outbound::<SLOT>());
        assert_eq!(fan, Some(FanTarget::All));
        assert_eq!(frame.as_slice(), b"two");
    }

    /// A supervisor parking on [`outbound_ready`](Fleet::outbound_ready) is roused when the reactor
    /// commits a frame, then drains it with [`try_next_outbound`](Fleet::try_next_outbound) — the
    /// outbound mirror of the inbound `notify` funnel. Without the commit's signal a supervisor with
    /// no other traffic to wake it would park forever beside a full lane, which is the bug this
    /// dedicated wake fixes: the lane's own consumer waker did not rouse the cross-task drain.
    #[test]
    fn an_outbound_commit_wakes_the_supervisor_and_try_next_outbound_drains() {
        use crate::reactor::grant::AnyGrantProducer;

        let (inbound, _inbound_rx) = leaked_grant_lane::<SLOT>(1);
        let (mut outbound_tx, outbound) = leaked_grant_lane::<SLOT>(1);
        let wake: &'static Signal<Mtx, ()> = leak(Signal::new());
        outbound_tx.set_outbound_wake(wake);
        let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
        let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
        let mut fleet: Fleet<Mtx, SLOT, 1, 1> = Fleet::new(
            MemberWire {
                inbound,
                outbound,
                notify: notify.sender(),
                outbound_wake: wake,
            },
            lifecycle.sender(),
        );

        assert!(
            fleet.try_next_outbound::<SLOT>().is_none(),
            "an empty lane drains to nothing"
        );

        assert!(outbound_tx.try_fill_frame_fan(FanTarget::All, b"hi"));
        block_on(with_timeout(
            Duration::from_millis(50),
            fleet.outbound_ready(),
        ))
        .expect("the commit must signal the outbound wake");

        let (_, fan, frame) = fleet
            .try_next_outbound::<SLOT>()
            .expect("the committed frame drains after the wake");
        assert_eq!(fan, Some(FanTarget::All));
        assert_eq!(frame.as_slice(), b"hi");
        assert!(
            fleet.try_next_outbound::<SLOT>().is_none(),
            "the depth-1 lane is empty once drained"
        );
    }

    /// A supervisor stands one peer up through its [`Fleet`] on a node built from a recipe, feeds an
    /// announce in over the member's wire, and the node hears it — then tears the peer back down.
    /// The whole high-level embassy path end to end: `Prns::new` over a recipe, `run` joining the
    /// reactor with the supervisor drive, and the Fleet's `stand_up`/`tear_down` reaching the pool.
    #[test]
    fn a_recipe_node_hears_an_announce_a_supervisor_stands_a_peer_up_for() {
        let notify: &'static Channel<Mtx, InterfaceId, 4> = leak(Channel::new());
        let commands: &'static Channel<Mtx, IssuedCommand, 4> = leak(Channel::new());
        let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 4> = leak(Channel::new());
        let completion: &'static CompletionPool<Mtx, 4> = leak(CompletionPool::new());

        // One wire pair for slot 0: the reactor side joins the node's plumbing, the interface side
        // becomes the fleet's one member.
        let (in_producer, in_consumer) = leaked_grant_lane::<SLOT>(4);
        let (out_producer, out_consumer) = leaked_grant_lane::<SLOT>(4);

        let free = InterfaceId::new([0xff; 8]);
        let mut inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, Mtx, SLOT>), 1> =
            HeaplessVec::new();
        let _ = inbound.push((free, in_consumer));
        let mut egress_lanes: HeaplessVec<
            (InterfaceId, EmbassyGrantProducer<'static, Mtx, SLOT>),
            1,
        > = HeaplessVec::new();
        let _ = egress_lanes.push((free, out_producer));

        let handle = EmbassyPrnsHandle::new(commands.sender(), completion);
        let plumbing = ReactorPlumbing::new(
            inbound,
            PooledEgress::new(egress_lanes),
            notify.receiver(),
            commands.receiver(),
            lifecycle.receiver(),
            handle,
        );

        let fleet: Fleet<Mtx, SLOT, 4, 4> = Fleet::new(
            MemberWire {
                inbound: in_producer,
                outbound: out_consumer,
                notify: notify.sender(),
                outbound_wake: leak(Signal::new()),
            },
            lifecycle.sender(),
        );

        let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let heard_sink = heard.clone();
        let recipe = PrnsRecipe {
            transport: Some(TEST_TRANSPORT_ID),
            pre_configured_destinations: [PreConfiguredDestination::Plain {
                app_name: "lxmf",
                aspects: &["delivery"],
            }],
            app_state: (),
            storage: GrowableHeap,
            routes: crate::routes![],
            interfaces: crate::interfaces![],
            on_event: move |event: PrnsEvent<'_>, _state: &()| {
                if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { .. }) = event {
                    *heard_sink.borrow_mut() += 1;
                }
            },
        };

        let mut node = Prns::new(
            recipe,
            plumbing,
            EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
            HeaplessVec::<InterfaceConfig, 1>::new(),
        );
        // The fleet's one lane is keyed by the supervisor's id; the WiFi peer routes to it by kind.
        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"test-supervisor");
        node.activate_fleet(0, supervisor);

        let raw = hx(RAW_ANNOUNCE);
        let peer = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"test-peer-medium");

        let drive = async move {
            let mut fleet = fleet;
            assert!(
                fleet.register_member(descriptor(peer)),
                "the lifecycle lane accepts the add"
            );
            Timer::after(Duration::from_millis(40)).await;

            assert!(
                fleet.deliver_inbound(peer, &raw),
                "the shared lane carries the peer's frame"
            );
            Timer::after(Duration::from_millis(80)).await;

            assert!(
                fleet.deregister_member(peer),
                "the lifecycle lane accepts the remove"
            );
            Timer::after(Duration::from_millis(20)).await;
        };

        let _ = block_on(with_timeout(Duration::from_millis(600), node.run(drive)));
        assert_eq!(
            *heard.borrow(),
            1,
            "the node heard the announce the supervisor's peer carried in"
        );
    }
}
