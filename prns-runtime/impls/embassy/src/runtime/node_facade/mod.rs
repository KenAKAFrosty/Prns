use core::future::Future;

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Receiver;
use heapless::Vec as HeaplessVec;

use crate::engine::{IssuedCommand, Journaled};
use crate::interfaces::ifac::{IfacContext, InterfaceIfac};
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::reactor::driver::{
    run_pooled, EmbassyGrantConsumer, InterfaceLifecycle, PooledEgress, PooledWiring,
};
use crate::reactor::Host;
use crate::storage::StorageLayout;

use super::request_router::RouteSet;
use super::{
    EmbassyInterfaceStore, InterfaceInspectionStore, Manual, NoInterfaceInspectionStore,
    PreConfiguredDestination, PrnsEvent, PrnsNodeRecipe,
};
use prns_runtime::runtime::{assemble_node, AssembledNode};

mod command_handle;
mod interface_lifecycle;

pub use command_handle::{CompletionPool, PrnsNodeHandle};
pub use interface_lifecycle::{Fleet, FleetWire};

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

#[cfg(test)]
mod tests;
