use core::future::Future;

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::signal::Signal;
use heapless::Vec as HeaplessVec;

use crate::engine::{IssuedCommand, Journaled};
use crate::interfaces::ifac::{IfacContext, InterfaceIfac};
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::reactor::driver::{
    run_pooled, EmbassyGrantConsumer, EmbassyGrantProducer, InterfaceLifecycle, PooledEgress,
    PooledWiring,
};
use crate::reactor::grant::{FrameTarget, GrantConsumer, GrantProducer};
use crate::reactor::Host;
use crate::storage::StorageLayout;

use super::request_router::RouteSet;
use super::{
    EmbassyInterfaceStore, InterfaceInspectionStore, Manual, NoInterfaceInspectionStore,
    PreConfiguredDestination, PrnsEvent, PrnsNodeRecipe,
};
use prns_runtime::runtime::{assemble_node, AssembledNode};

mod command_handle;

pub use command_handle::{CompletionPool, PrnsNodeHandle};

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
                    if handle.settle(*id, settlement.clone()) {
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
                    if handle.settle(*id, settlement.clone()) {
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
