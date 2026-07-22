use core::future::Future;

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, Receiver};
use heapless::Vec as HeaplessVec;

use crate::engine::{IssuedCommand, Journaled, MAX_SEND_REQUEST_DATA_LEN};
use crate::interfaces::{InterfaceDescriptor, InterfaceId, InterfaceIfac};
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
use super::reactor_pool::{LaneConfiguration, LaneTarget};
use prns_runtime::runtime::{assemble_node, AssembledNode};

/// Reactor-side endpoints for a board-owned static interface pool.
pub struct ReactorWiring<
    M,
    const FRAME: usize,
    const LANE_COUNT: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
    const COMPLETIONS: usize,
> where
    M: RawMutex + 'static,
{
    inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, FRAME>), LANE_COUNT>,
    egress: PooledEgress<M, FRAME, LANE_COUNT>,
    configurations: [Option<LaneConfiguration>; LANE_COUNT],
    notify: Receiver<'static, M, InterfaceId, NOTIFY>,
    commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
    lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
    handle: PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS>,
}

impl<
        M: RawMutex + 'static,
        const FRAME: usize,
        const LANE_COUNT: usize,
        const NOTIFY: usize,
        const COMMANDS: usize,
        const LIFECYCLE: usize,
        const COMPLETIONS: usize,
    > ReactorWiring<M, FRAME, LANE_COUNT, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>
{
    #[must_use]
    pub(super) fn new(
        inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, FRAME>), LANE_COUNT>,
        egress: PooledEgress<M, FRAME, LANE_COUNT>,
        configurations: [Option<LaneConfiguration>; LANE_COUNT],
        notify: Receiver<'static, M, InterfaceId, NOTIFY>,
        commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
        lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
        handle: PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS>,
    ) -> Self {
        Self {
            inbound,
            egress,
            configurations,
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
    const FRAME: usize,
    const LANE_COUNT: usize,
    const INTERFACE_CAPACITY: usize,
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
    inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, FRAME>), LANE_COUNT>,
    egress: PooledEgress<M, FRAME, LANE_COUNT>,
    notify: Receiver<'static, M, InterfaceId, NOTIFY>,
    commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
    lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
    handle: PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS>,
    host: H,
    initial: HeaplessVec<InterfaceDescriptor, INTERFACE_CAPACITY>,
    ifacs: HeaplessVec<InterfaceIfac, LANE_COUNT>,
}

pub struct RequestRoutingCapacity<const REQUESTS: usize, const REQUEST_BYTES: usize>;

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
        const FRAME: usize,
        const LANE_COUNT: usize,
        const INTERFACE_CAPACITY: usize,
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
        FRAME,
        LANE_COUNT,
        INTERFACE_CAPACITY,
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
        wiring: ReactorWiring<M, FRAME, LANE_COUNT, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>,
        host: H,
    ) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        Self::build(recipe, wiring, host)
    }
}

impl<
        St,
        R,
        F,
        S,
        H,
        M,
        const FRAME: usize,
        const LANE_COUNT: usize,
        const INTERFACE_CAPACITY: usize,
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
        FRAME,
        LANE_COUNT,
        INTERFACE_CAPACITY,
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
        wiring: ReactorWiring<M, FRAME, LANE_COUNT, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>,
        host: H,
        _capacity: RequestRoutingCapacity<ROUTED_REQUESTS, ROUTED_REQUEST_BYTES>,
    ) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        Self::build(recipe, wiring, host)
    }

    fn build<'d, D>(
        recipe: PrnsNodeRecipe<D, St, R, F, Manual, S>,
        wiring: ReactorWiring<M, FRAME, LANE_COUNT, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS>,
        host: H,
    ) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        const {
            assert!(
                INTERFACE_CAPACITY >= LANE_COUNT,
                "PrnsNode INTERFACE_CAPACITY must cover every reactor lane"
            );
        }
        let (node, Manual) = assemble_node(recipe);
        let mut initial = HeaplessVec::new();
        let mut ifacs = HeaplessVec::new();
        for configuration in wiring.configurations.into_iter().flatten() {
            let id = match configuration.target {
                LaneTarget::Interface(descriptor) => {
                    assert!(initial.push(descriptor).is_ok());
                    descriptor.id
                }
                LaneTarget::Supervisor(id) => id,
            };
            if let Some(context) = configuration.ifac {
                assert!(ifacs.push(InterfaceIfac { id, context }).is_ok());
            }
        }

        PrnsNode {
            node,
            inbound: wiring.inbound,
            egress: wiring.egress,
            notify: wiring.notify,
            commands: wiring.commands,
            lifecycle: wiring.lifecycle,
            handle: wiring.handle,
            host,
            initial,
            ifacs,
        }
    }

    #[must_use]
    pub fn handle(&self) -> PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS> {
        self.handle
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
                INTERFACES >= INTERFACE_CAPACITY,
                "EmbassyInterfaceStore INTERFACES must cover PrnsNode INTERFACE_CAPACITY"
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
                INTERFACES >= INTERFACE_CAPACITY,
                "EmbassyInterfaceStore INTERFACES must cover PrnsNode INTERFACE_CAPACITY"
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
