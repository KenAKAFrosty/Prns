use core::future::Future;

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, Receiver};
use heapless::Vec as HeaplessVec;

use crate::engine::{IssuedCommand, Journaled, MAX_SEND_REQUEST_DATA_LEN};
use crate::interfaces::{IfacContext, InterfaceIfac};
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::reactor::driver::{
    run_pooled, EmbassyGrantConsumer, InterfaceLifecycle, PooledEgress, PooledWiring,
};
use crate::reactor::Host;
use crate::storage::StorageLayout;

use super::super::request_router::RouteSet;
use super::super::request_runner::{run_router, RunnerRequest};
use super::super::{
    EmbassyInterfaceStore, InterfaceInspectionStore, Manual, NoInterfaceInspectionStore,
    PreConfiguredDestination, PrnsEvent, PrnsNodeRecipe,
};
use super::command_handle::PrnsNodeHandle;
use prns_runtime::runtime::{assemble_node, AssembledNode};

/// Reactor-side endpoints for a board-owned static interface pool.
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

/// An Embassy node over a board-owned static interface pool.
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
    const ROUTED_REQUESTS: usize = 4,
    const ROUTED_REQUEST_BYTES: usize = MAX_SEND_REQUEST_DATA_LEN,
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

pub struct RequestRoutingCapacity<const REQUESTS: usize, const REQUEST_BYTES: usize>;

#[derive(Debug, PartialEq, Eq)]
pub enum InterfaceActivationError {
    LaneUnavailable { slot: usize },
    InterfaceCapacity,
    IfacCapacity,
}

enum LaneActivation {
    Interface(InterfaceDescriptor),
    Supervisor(InterfaceId),
}

impl LaneActivation {
    fn id(&self) -> InterfaceId {
        match self {
            Self::Interface(descriptor) => descriptor.id,
            Self::Supervisor(id) => *id,
        }
    }
}

impl<const REQUESTS: usize, const REQUEST_BYTES: usize> Default
    for RequestRoutingCapacity<REQUESTS, REQUEST_BYTES>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const REQUESTS: usize, const REQUEST_BYTES: usize>
    RequestRoutingCapacity<REQUESTS, REQUEST_BYTES>
{
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
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
    PrnsNode<
        St,
        R,
        F,
        S,
        H,
        M,
        SLOT,
        IFACES,
        MAX_IFACES,
        NOTIFY,
        COMMANDS,
        LIFECYCLE,
        COMPLETIONS,
        4,
        MAX_SEND_REQUEST_DATA_LEN,
    >
where
    R: RouteSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
    H: Host,
    M: RawMutex + 'static,
{
    pub fn new<'d, D>(
        recipe: PrnsNodeRecipe<D, St, R, F, Manual, S>,
        plumbing: ReactorPlumbing<M, SLOT, IFACES, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>,
        host: H,
        initial: HeaplessVec<InterfaceDescriptor, MAX_IFACES>,
    ) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        Self::build(recipe, plumbing, host, initial)
    }
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
        const ROUTED_REQUESTS: usize,
        const ROUTED_REQUEST_BYTES: usize,
    >
    PrnsNode<
        St,
        R,
        F,
        S,
        H,
        M,
        SLOT,
        IFACES,
        MAX_IFACES,
        NOTIFY,
        COMMANDS,
        LIFECYCLE,
        COMPLETIONS,
        ROUTED_REQUESTS,
        ROUTED_REQUEST_BYTES,
    >
where
    R: RouteSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
    H: Host,
    M: RawMutex + 'static,
{
    pub fn new_with_request_capacity<'d, D>(
        recipe: PrnsNodeRecipe<D, St, R, F, Manual, S>,
        plumbing: ReactorPlumbing<M, SLOT, IFACES, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>,
        host: H,
        initial: HeaplessVec<InterfaceDescriptor, MAX_IFACES>,
        _capacity: RequestRoutingCapacity<ROUTED_REQUESTS, ROUTED_REQUEST_BYTES>,
    ) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        Self::build(recipe, plumbing, host, initial)
    }

    fn build<'d, D>(
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

    #[must_use]
    pub fn handle(&self) -> PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS> {
        self.handle
    }

    pub fn activate(
        &mut self,
        slot: usize,
        descriptor: InterfaceDescriptor,
    ) -> Result<(), InterfaceActivationError> {
        self.activate_access(slot, LaneActivation::Interface(descriptor), None)
    }

    pub fn activate_with_ifac(
        &mut self,
        slot: usize,
        descriptor: InterfaceDescriptor,
        context: IfacContext,
    ) -> Result<(), InterfaceActivationError> {
        self.activate_access(slot, LaneActivation::Interface(descriptor), Some(context))
    }

    fn activate_access(
        &mut self,
        slot: usize,
        activation: LaneActivation,
        mut context: Option<IfacContext>,
    ) -> Result<(), InterfaceActivationError> {
        let Some(old_id) = self.inbound.get(slot).map(|entry| entry.0) else {
            return Err(InterfaceActivationError::LaneUnavailable { slot });
        };
        if !self.egress.has_slot(slot) {
            return Err(InterfaceActivationError::LaneUnavailable { slot });
        }

        let descriptor_position = self
            .initial
            .iter()
            .position(|descriptor| descriptor.id == old_id);
        let ifac_position = self.ifacs.iter().position(|ifac| ifac.id == old_id);
        let needs_descriptor =
            matches!(&activation, LaneActivation::Interface(_)) && descriptor_position.is_none();
        let needs_ifac = context.is_some() && ifac_position.is_none();

        if needs_descriptor && self.initial.is_full() {
            return Err(InterfaceActivationError::InterfaceCapacity);
        }
        if needs_ifac && self.ifacs.is_full() {
            return Err(InterfaceActivationError::IfacCapacity);
        }

        let id = activation.id();
        let mut pushed_ifac = false;
        if ifac_position.is_none() {
            if let Some(context) = context.take() {
                if self.ifacs.push(InterfaceIfac { id, context }).is_err() {
                    return Err(InterfaceActivationError::IfacCapacity);
                }
                pushed_ifac = true;
            }
        }

        match activation {
            LaneActivation::Interface(descriptor) => match descriptor_position {
                Some(position) => self.initial[position] = descriptor,
                None => {
                    if self.initial.push(descriptor).is_err() {
                        if pushed_ifac {
                            let _ = self.ifacs.pop();
                        }
                        return Err(InterfaceActivationError::InterfaceCapacity);
                    }
                }
            },
            LaneActivation::Supervisor(_) => {
                if let Some(position) = descriptor_position {
                    let _ = self.initial.swap_remove(position);
                }
            }
        }

        if let Some(position) = ifac_position {
            match context {
                Some(context) => self.ifacs[position] = InterfaceIfac { id, context },
                None => {
                    let _ = self.ifacs.swap_remove(position);
                }
            }
        }

        self.inbound[slot].0 = id;
        self.egress.activate(slot, id);
        Ok(())
    }

    pub fn activate_supervisor(
        &mut self,
        slot: usize,
        supervisor: InterfaceId,
    ) -> Result<(), InterfaceActivationError> {
        self.activate_access(slot, LaneActivation::Supervisor(supervisor), None)
    }

    pub fn activate_supervisor_with_ifac(
        &mut self,
        slot: usize,
        supervisor: InterfaceId,
        context: IfacContext,
    ) -> Result<(), InterfaceActivationError> {
        self.activate_access(slot, LaneActivation::Supervisor(supervisor), Some(context))
    }

    /// Runs the reactor with the caller's interface and supervisor tasks.
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
        let request_channel =
            Channel::<M, RunnerRequest<ROUTED_REQUEST_BYTES>, ROUTED_REQUESTS>::new();
        let request_sender = request_channel.sender();
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
                if let Some(request) = RunnerRequest::copy_from(&journaled) {
                    let _ = request_sender.try_send(request);
                }
                on_event(PrnsEvent::from(journaled), &state);
            },
            crate::reactor::decline_all(),
            store,
        );
        let router =
            run_router::<St, R, M, COMMANDS, COMPLETIONS, ROUTED_REQUESTS, ROUTED_REQUEST_BYTES>(
                &state,
                request_channel.receiver(),
                handle,
            );
        join(join(reactor, router), drive).await;
    }

    /// Runs only the reactor for boards that schedule interfaces separately.
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
        let request_channel =
            Channel::<M, RunnerRequest<ROUTED_REQUEST_BYTES>, ROUTED_REQUESTS>::new();
        let request_sender = request_channel.sender();
        let reactor = run_pooled(
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
                if let Some(request) = RunnerRequest::copy_from(&journaled) {
                    let _ = request_sender.try_send(request);
                }
                on_event(PrnsEvent::from(journaled), state);
            },
            crate::reactor::decline_all(),
            store,
        );
        let router =
            run_router::<St, R, M, COMMANDS, COMPLETIONS, ROUTED_REQUESTS, ROUTED_REQUEST_BYTES>(
                state,
                request_channel.receiver(),
                *handle,
            );
        join(reactor, router).await;
    }
}

#[cfg(test)]
mod tests;
