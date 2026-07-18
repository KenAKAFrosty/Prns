//! The Embassy command surface. [`PrnsNodeHandle`] combines the command channel's [`Sender`] with an application-provided static [`CompletionPool`]; the node owns the matching receiver and shares the pool.

use core::cell::RefCell;
use core::future::Future;

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::signal::Signal;
use heapless::Vec as HeaplessVec;
use portable_atomic::{AtomicU64, Ordering};

use crate::engine::{
    CloseLink, CommandId, EngineCommand, IssuedCommand, Journaled, PacketReceiptDelivered, Respond,
    RespondData, SendSinglePacket, SendSinglePacketFailure, SendSinglePacketPayload, Settlement,
};
use crate::interfaces::ifac::{IfacContext, InterfaceIfac};
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::reactor::driver::{
    run_pooled, EmbassyGrantConsumer, EmbassyGrantProducer, InterfaceLifecycle, PooledEgress,
    PooledWiring,
};
use crate::reactor::grant::{FrameTarget, GrantConsumer, GrantProducer};
use crate::reactor::Host;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::request_router::{RespondToken, RouteSet};
use super::{
    EmbassyInterfaceStore, InterfaceInspectionStore, Manual, NoInterfaceInspectionStore,
    PreConfiguredDestination, PrnsEvent, PrnsNodeRecipe, SendError,
};
use prns_runtime::runtime::{assemble_node, AssembledNode};

/// The free-slot sentinel — no real [`CommandId`] reaches `u64::MAX` (the handle mints from zero).
const NO_AWAITER: u64 = u64::MAX;

/// A fixed pool of completion slots an embassy app provides as a `static`: the embedded twin of tokio's per-command oneshot. An awaited send claims a slot, parks on its [`Signal`], and the binding fires that slot by command id when the engine settles; the send future releases its slot on drop, so a cancelled send can never wake a later claimant. All bookkeeping is serialized under one blocking mutex, and `settle` signals while holding it, closing the window where a freed slot could be reused mid-fire. `N` bounds awaited sends in flight.
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

    /// Reserve a free slot for `id`, clearing any stale signal first. `None` when the pool is full — the caller already has more awaited sends in flight than `N`.
    fn claim(&self, id: CommandId) -> Option<usize> {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            let slot = awaited.iter().position(|&a| a == NO_AWAITER)?;
            self.slots[slot].reset();
            awaited[slot] = id.0;
            Some(slot)
        })
    }

    /// Free `slot` only if it still belongs to `id` — the send future's drop path. After a settle has cleared the slot (and another send may have claimed it), this is a no-op, so a late drop can't clobber a newer claimant.
    fn release(&self, slot: usize, id: CommandId) {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            if awaited[slot] == id.0 {
                awaited[slot] = NO_AWAITER;
                self.slots[slot].reset();
            }
        });
    }

    /// Hand `settlement` to the slot awaiting `id`, if any, and report whether it fired; the runner drops a fired settlement from the event stream so an awaited command resolves once. Signals under the lock so a concurrent release/claim can't slip the slot out from under the wakeup.
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

/// The Embassy command handle. It is `Copy`, so any task can drive the node, and mints every [`CommandId`] from the completion pool's shared counter.
pub struct PrnsNodeHandle<'a, M: RawMutex, const COMMANDS: usize, const N: usize> {
    commands: Sender<'a, M, IssuedCommand, COMMANDS>,
    pool: &'a CompletionPool<M, N>,
}

impl<M: RawMutex, const COMMANDS: usize, const N: usize> Clone
    for PrnsNodeHandle<'_, M, COMMANDS, N>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const COMMANDS: usize, const N: usize> Copy
    for PrnsNodeHandle<'_, M, COMMANDS, N>
{
}

impl<'a, M: RawMutex, const COMMANDS: usize, const N: usize> PrnsNodeHandle<'a, M, COMMANDS, N> {
    /// Pair the command channel's sender with the completion pool — the app holds both as `static`s and passes the matching [`CompletionPool`] reference to the runner too.
    #[must_use]
    pub fn new(
        commands: Sender<'a, M, IssuedCommand, COMMANDS>,
        pool: &'a CompletionPool<M, N>,
    ) -> Self {
        Self { commands, pool }
    }

    /// Queue an engine command and return the [`CommandId`] it was minted under — watch the event stream for the settlement tagged with it. `None` if the bounded command lane is full. The fire-and-forget escape hatch; to await the outcome, prefer [`send_single_packet`](Self::send_single_packet).
    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        let id = self.pool.mint();
        self.commands.try_send(IssuedCommand { id, command }).ok()?;
        Some(id)
    }

    /// Send one Single and await its delivery proof. Claims a pool slot and frees it on every exit, cancellation included; returns `SendError::Busy` when more awaited sends are in flight than the pool's `N`.
    pub async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        let payload =
            SendSinglePacketPayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
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
                command: EngineCommand::SendSinglePacket(SendSinglePacket {
                    destination,
                    payload,
                }),
            })
            .map_err(|_| SendError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::SendSinglePacket(result) => result.map_err(SendError::Failed),
            _ => Err(SendError::NodeStopped),
        }
    }

    /// Answer a request with `body` as a single RESPONSE packet — the request runner's path. Embedded responds inline, so a `body` past the link MDU is refused here (returns `false`); the host auto-upgrades to a resource instead.
    pub fn respond(&self, responder: RespondToken, body: &[u8]) -> bool {
        match RespondData::from_slice(body) {
            Ok(data) => self.respond_owned(responder, data),
            Err(_) => false,
        }
    }

    /// Answer a request by moving a prebuilt [`RespondData`] in: one copy fewer than [`respond`](Self::respond) since the handler already filled a grant. `false` once the command lane is full.
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

/// Frees a claimed completion slot when its awaited send finishes or is cancelled. Release is guarded by the awaited id, so a late drop after the settle already reused the slot is a no-op.
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

impl<M: RawMutex, const COMMANDS: usize, const N: usize> super::PrnsNodeApi
    for PrnsNodeHandle<'_, M, COMMANDS, N>
{
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
        self.respond(responder, body)
    }

    fn close_link(&self, link_id: LinkId) -> bool {
        self.close_link(link_id)
    }
}

/// The reactor-side wiring an embassy node runs on: the pool's inbound consumers and egress, the three channel receivers, and the command handle. The board declares the matching `static` channels and hands this bundle to [`PrnsNode::new`]; the interface-side seam halves come off the same pool separately.
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
    handle: PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS>,
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
    /// Bundle the reactor's half of the pool: every slot's reactor-side endpoint, the receivers of the node's three `static` channels, and the command sender paired with the completion pool.
    #[must_use]
    pub fn new(
        inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, SLOT>), IFACES>,
        egress: PooledEgress<M, SLOT, IFACES>,
        notify: Receiver<'static, M, InterfaceId, NOTIFY>,
        commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
        lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
        handle: PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS>,
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

/// A node on an Embassy host, built from a [`PrnsNodeRecipe`] over a board-declared static interface pool ([`ReactorPlumbing`]). The wires are attached explicitly because the board owns their static storage: [`activate`](Self::activate) stands up a top-level interface on a pool slot, and [`run`](Self::run) joins the reactor with the caller's drive.
pub struct PrnsNode<
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
    node: AssembledNode<St, R, F, S>,
    inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, SLOT>), IFACES>,
    egress: PooledEgress<M, SLOT, IFACES>,
    notify: Receiver<'static, M, InterfaceId, NOTIFY>,
    commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
    lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
    handle: PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS>,
    host: H,
    initial: HeaplessVec<InterfaceDescriptor, MAX_IFACES>,
    ifacs: HeaplessVec<InterfaceIfac, IFACES>,
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
    >
    PrnsNode<St, R, F, S, H, M, SLOT, IFACES, MAX_IFACES, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>
where
    R: RouteSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
    H: Host,
    M: RawMutex + 'static,
{
    /// Stand a node up from `recipe` over the board's `plumbing` and `host` (its clock + entropy). No interface is wired yet: [`activate`](Self::activate) names the top-level wires, and the supervisor drive names the rest.
    pub fn new<'d, D>(
        recipe: PrnsNodeRecipe<D, St, R, F, Manual, S>,
        plumbing: ReactorPlumbing<M, SLOT, IFACES, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>,
        host: H,
        initial: HeaplessVec<InterfaceDescriptor, MAX_IFACES>,
    ) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        let (node, Manual) = assemble_node(recipe);

        PrnsNode {
            node,
            inbound: plumbing.inbound,
            egress: plumbing.egress,
            notify: plumbing.notify,
            commands: plumbing.commands,
            lifecycle: plumbing.lifecycle,
            handle: plumbing.handle,
            host,
            initial,
            ifacs: HeaplessVec::new(),
        }
    }

    /// The command surface: `Copy`, so any task can drive the node while [`run`](Self::run) owns the loop.
    #[must_use]
    pub fn handle(&self) -> PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS> {
        self.handle
    }

    /// Stand a top-level interface up on pool `slot` and hand back the interface-side seam to drive it on; the returned descriptor's id routes inbound and egress to this slot from the moment [`run`](Self::run) starts. The supervisor's peers come up later through its [`Fleet`].
    pub fn activate(&mut self, slot: usize, descriptor: InterfaceDescriptor) {
        let _ = self.activate_access(slot, descriptor, None);
    }

    pub fn activate_with_ifac(
        &mut self,
        slot: usize,
        descriptor: InterfaceDescriptor,
        context: IfacContext,
    ) -> bool {
        self.activate_access(slot, descriptor, Some(context))
    }

    fn activate_access(
        &mut self,
        slot: usize,
        descriptor: InterfaceDescriptor,
        context: Option<IfacContext>,
    ) -> bool {
        if let Some(entry) = self.inbound.get_mut(slot) {
            let old_id = entry.0;
            if let Some(position) = self.ifacs.iter().position(|ifac| ifac.id == old_id) {
                let _ = self.ifacs.swap_remove(position);
            }
            if let Some(context) = context {
                if self
                    .ifacs
                    .push(InterfaceIfac {
                        id: descriptor.id,
                        context,
                    })
                    .is_err()
                {
                    return false;
                }
            }
            entry.0 = descriptor.id;
            self.egress.activate(slot, descriptor.id);
            let _ = self.initial.push(descriptor);
            true
        } else {
            false
        }
    }

    /// Register a supervisor's shared lane on pool `slot`, keyed by the supervisor's id. Unlike [`activate`](Self::activate) this adds no engine interface; inbound and egress for every child of the supervisor's kind route to this one lane (see `lane_serves`).
    pub fn activate_fleet(&mut self, slot: usize, supervisor: InterfaceId) {
        let _ = self.activate_fleet_access(slot, supervisor, None);
    }

    pub fn activate_fleet_with_ifac(
        &mut self,
        slot: usize,
        supervisor: InterfaceId,
        context: IfacContext,
    ) -> bool {
        self.activate_fleet_access(slot, supervisor, Some(context))
    }

    fn activate_fleet_access(
        &mut self,
        slot: usize,
        supervisor: InterfaceId,
        context: Option<IfacContext>,
    ) -> bool {
        if let Some(entry) = self.inbound.get_mut(slot) {
            let old_id = entry.0;
            if let Some(position) = self.ifacs.iter().position(|ifac| ifac.id == old_id) {
                let _ = self.ifacs.swap_remove(position);
            }
            if let Some(context) = context {
                if self
                    .ifacs
                    .push(InterfaceIfac {
                        id: supervisor,
                        context,
                    })
                    .is_err()
                {
                    return false;
                }
            }
            entry.0 = supervisor;
            self.egress.activate(slot, supervisor);
            true
        } else {
            false
        }
    }

    /// Drive the node until the executor drops it: the reactor joined with the caller's `drive`. Every engine event reaches the recipe's `on_event` with shared `&state`, zero-copy.
    pub async fn run(self, drive: impl Future<Output = ()>) {
        self.run_with_inspection_store(&NoInterfaceInspectionStore, drive)
            .await;
    }

    pub async fn run_with_interface_store<
        const INTERFACES: usize,
        const PACKET_PHY_CAPACITY: usize,
        const PACKET_PHY_INDEX_BUCKETS: usize,
    >(
        self,
        store: &EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>,
        drive: impl Future<Output = ()>,
    ) where
        M: Sync,
    {
        const {
            assert!(
                INTERFACES >= MAX_IFACES,
                "EmbassyInterfaceStore INTERFACES must cover PrnsNode MAX_IFACES"
            );
        }
        self.run_with_inspection_store(store, drive).await;
    }

    async fn run_with_inspection_store<Store>(self, store: &Store, drive: impl Future<Output = ()>)
    where
        Store: InterfaceInspectionStore,
    {
        let PrnsNode {
            node,
            mut inbound,
            mut egress,
            notify,
            commands,
            lifecycle,
            handle,
            mut host,
            initial,
            mut ifacs,
        } = self;
        let AssembledNode {
            mut engine,
            state,
            mut on_event,
            routes: _,
        } = node;
        let reactor = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                initial: &initial,
                ifacs: &mut ifacs,
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
            crate::reactor::decline_all(),
            store,
        );
        join(reactor, drive).await;
    }

    /// Drive only the reactor, no interface drive joined: the board runs its interfaces and supervisors wherever it likes, including a separate *core*. The reactor↔interface seam is all `CriticalSectionRawMutex` channels, so the engine can own one core while the I/O owns another, genuine parallelism with no shared state but the lanes.
    pub async fn run_reactor(&mut self) {
        self.run_reactor_with_inspection_store(&NoInterfaceInspectionStore)
            .await;
    }

    pub async fn run_reactor_with_interface_store<
        const INTERFACES: usize,
        const PACKET_PHY_CAPACITY: usize,
        const PACKET_PHY_INDEX_BUCKETS: usize,
    >(
        &mut self,
        store: &EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>,
    ) where
        M: Sync,
    {
        const {
            assert!(
                INTERFACES >= MAX_IFACES,
                "EmbassyInterfaceStore INTERFACES must cover PrnsNode MAX_IFACES"
            );
        }
        self.run_reactor_with_inspection_store(store).await;
    }

    async fn run_reactor_with_inspection_store<Store>(&mut self, store: &Store)
    where
        Store: InterfaceInspectionStore,
    {
        let PrnsNode {
            node,
            inbound,
            egress,
            notify,
            commands,
            lifecycle,
            handle,
            host,
            initial,
            ifacs,
        } = self;
        let AssembledNode {
            engine,
            state,
            on_event,
            routes: _,
        } = node;
        run_pooled(
            engine,
            host,
            PooledWiring {
                initial: &*initial,
                ifacs,
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
            crate::reactor::decline_all(),
            store,
        )
        .await;
    }
}

/// One member slot's reactor wire, lent to a supervisor: the inbound producer, the outbound consumer, and the notify funnel, tagged with the member's *current* id (the slot's id changes as peers come and go). The endpoints are permanent, so the slot reuses for the next peer with no re-split.
pub struct MemberWire<M: RawMutex + 'static, const SLOT: usize, const NOTIFY: usize> {
    pub inbound: EmbassyGrantProducer<'static, M, SLOT>,
    pub outbound: EmbassyGrantConsumer<'static, M, SLOT>,
    pub notify: Sender<'static, M, InterfaceId, NOTIFY>,
    pub outbound_wake: &'static Signal<M, ()>,
}

/// A supervisor's lever onto the node's reactor: the embedded twin of the host `Fleet`, minus the spawn. The whole fleet shares **one** [`MemberWire`]: every peer's inbound frame is funneled in tagged with that peer's id, and the reactor's outbound frames drain off tagged with their target, so the kind-routing demuxes a whole fleet over one lane-pair. A confirmed peer becomes a distinct engine interface with [`register_member`](Self::register_member); each costs only a descriptor, never a lane.
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
    /// Build a fleet over its one shared `wire` (the interface-side halves of the supervisor's lane) and the `lifecycle` sender whose receiver the reactor parks on.
    #[must_use]
    pub fn new(
        wire: MemberWire<M, SLOT, NOTIFY>,
        lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
    ) -> Self {
        Self { wire, lifecycle }
    }

    /// Register a confirmed peer as a distinct engine interface under `descriptor`: the engine forwards to it at once, its frames routing to this fleet's one lane by kind. `false` if the lifecycle lane is full.
    pub fn register_member(&self, descriptor: InterfaceDescriptor) -> bool {
        self.lifecycle
            .try_send(InterfaceLifecycle::Add { descriptor })
            .is_ok()
    }

    /// Drop the member with this id: the reactor culls its routes and forgets its descriptor. The shared lane stays for the rest of the fleet. `false` if the lifecycle lane is full.
    pub fn deregister_member(&self, id: InterfaceId) -> bool {
        self.lifecycle
            .try_send(InterfaceLifecycle::Remove { id })
            .is_ok()
    }

    /// Funnel one inbound frame from peer `child` into the shared lane, tagged so the reactor ingests it as `child`'s, then announce the commit on the notify funnel. `false` if the lane is momentarily full (the frame drops, as a full lane does), so a slow reactor never stalls the medium read.
    pub fn deliver_inbound(&mut self, child: InterfaceId, bytes: &[u8]) -> bool {
        let Some(grant) = self.wire.inbound.try_grant() else {
            return false;
        };
        grant.fill_for(child, bytes);
        self.wire.inbound.commit();
        let _ = self.wire.notify.try_send(child);
        true
    }

    /// Park until the reactor grants an outbound frame, returning a copy plus its [`FrameTarget`]: the one peer it addresses, or the fan a fleet broadcast selects members by. The frame is copied out rather than borrowed, so the returned value owns nothing of the fleet (it can ride a `select` arm without a borrow clash), and the slot is released before returning, so the depth-1 lane refills at once and each frame is carried exactly once.
    pub async fn next_outbound<const OUT: usize>(&mut self) -> (FrameTarget, HeaplessVec<u8, OUT>) {
        self.wire.outbound.release();
        let slot = self.wire.outbound.peek().await;
        let target = slot.target;
        let mut bytes: HeaplessVec<u8, OUT> = HeaplessVec::new();
        let _ = bytes.extend_from_slice(slot.frame());
        self.wire.outbound.release();
        (target, bytes)
    }

    /// Park until the reactor commits an outbound frame onto this fleet's shared lane: the reactor signals every commit, rousing a waiting supervisor across the task boundary without depending on the lane's own consumer waker. On wake, drain with [`try_next_outbound`](Self::try_next_outbound) until `None`.
    pub async fn outbound_ready(&self) {
        self.wire.outbound_wake.wait().await;
    }

    /// Take the next outbound frame without parking; `None` when the lane is momentarily empty. The copy/release contract matches [`next_outbound`](Self::next_outbound). The signal-then-drain pair replaces awaiting the lane directly, so several frames committed before the supervisor runs all flush.
    pub fn try_next_outbound<const OUT: usize>(
        &mut self,
    ) -> Option<(FrameTarget, HeaplessVec<u8, OUT>)> {
        let slot = self.wire.outbound.try_peek()?;
        let target = slot.target;
        let mut bytes: HeaplessVec<u8, OUT> = HeaplessVec::new();
        let _ = bytes.extend_from_slice(slot.frame());
        self.wire.outbound.release();
        Some((target, bytes))
    }
}

#[cfg(test)]
mod tests;
