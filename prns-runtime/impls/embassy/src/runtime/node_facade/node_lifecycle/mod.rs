use core::future::Future;
use core::mem::MaybeUninit;

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, Receiver};
use embedded_storage_async::nor_flash::NorFlash;
use heapless::Vec as HeaplessVec;
use static_cell::StaticCell;

use crate::engine::{IssuedCommand, ProofRequest, MAX_SEND_REQUEST_DATA_LEN};
use crate::identity::held::HoldIdentityError;
use crate::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{InterfaceDescriptor, InterfaceId, InterfaceIfac};
use crate::manifold::driver::{
    run_pooled, EmbassyInterfaceStatus, InterfaceLifecycle, PooledEgress, PooledWiring,
    ResumableHost,
};
use crate::manifold::grant::ManifoldLaneReader;
use crate::manifold::Host;
use crate::storage::StorageLayout;

use super::super::request_endpoints::RequestEndpointSet;
use super::super::request_runner::{run_router, RunnerRequest};
use super::super::{
    EmbassyInterfaceStore, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistenceRestoreReport, InterfaceInspectionStore, ManifoldPersistence,
    ManuallyAttached, NoInterfaceInspectionStore, NoManifoldPersistence, PreConfiguredDestination,
    PrnsEvent, PrnsNodeRecipe, RouteSnapshotKeys,
};
use super::command_handle::JournalRoute;
use super::command_handle::PrnsNodeHandle;
use prns_runtime::runtime::placement::assemble_node_in_place;
use prns_runtime::runtime::{assemble_node, AssembledNode, NoPersistence};

pub struct ManifoldWiring<
    M,
    const LANE_COUNT: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize = 0,
    const RESPONSE_BYTES: usize = 0,
> where
    M: RawMutex + 'static,
{
    pub(super) inbound: HeaplessVec<(InterfaceId, &'static mut dyn ManifoldLaneReader), LANE_COUNT>,
    pub(super) frame_accounting_statuses: HeaplessVec<&'static EmbassyInterfaceStatus, LANE_COUNT>,
    pub(super) egress: PooledEgress<LANE_COUNT>,
    pub(super) initial: HeaplessVec<InterfaceDescriptor, LANE_COUNT>,
    pub(super) ifacs: HeaplessVec<InterfaceIfac, LANE_COUNT>,
    pub(super) notify: Receiver<'static, M, InterfaceId, NOTIFY>,
    pub(super) commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
    pub(super) lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
    pub(super) handle:
        PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
}

pub struct PrnsNode<
    St,
    R,
    F,
    S,
    H,
    M,
    const LANE_COUNT: usize,
    const INTERFACE_CAPACITY: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
    const COMPLETIONS: usize,
    const ROUTED_REQUESTS: usize = 4,
    const ROUTED_REQUEST_BYTES: usize = MAX_SEND_REQUEST_DATA_LEN,
    const REQUEST_COMPLETIONS: usize = 0,
    const RESPONSE_BYTES: usize = 0,
> where
    S: StorageLayout,
    M: RawMutex + 'static,
{
    node: AssembledNode<St, R, F, S>,
    inbound: HeaplessVec<(InterfaceId, &'static mut dyn ManifoldLaneReader), LANE_COUNT>,
    frame_accounting_statuses: HeaplessVec<&'static EmbassyInterfaceStatus, LANE_COUNT>,
    egress: PooledEgress<LANE_COUNT>,
    notify: Receiver<'static, M, InterfaceId, NOTIFY>,
    commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
    lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
    handle: PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    host: H,
    descriptors: HeaplessVec<InterfaceDescriptor, INTERFACE_CAPACITY>,
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
        const LANE_COUNT: usize,
        const INTERFACE_CAPACITY: usize,
        const NOTIFY: usize,
        const COMMANDS: usize,
        const LIFECYCLE: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    >
    PrnsNode<
        St,
        R,
        F,
        S,
        H,
        M,
        LANE_COUNT,
        INTERFACE_CAPACITY,
        NOTIFY,
        COMMANDS,
        LIFECYCLE,
        COMPLETIONS,
        4,
        MAX_SEND_REQUEST_DATA_LEN,
        REQUEST_COMPLETIONS,
        RESPONSE_BYTES,
    >
where
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
    H: Host,
    M: RawMutex + 'static,
{
    pub fn new<'d, D>(
        recipe: PrnsNodeRecipe<D, St, R, F, ManuallyAttached, S>,
        wiring: ManifoldWiring<
            M,
            LANE_COUNT,
            NOTIFY,
            COMMANDS,
            LIFECYCLE,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
        >,
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
        const LANE_COUNT: usize,
        const INTERFACE_CAPACITY: usize,
        const NOTIFY: usize,
        const COMMANDS: usize,
        const LIFECYCLE: usize,
        const COMPLETIONS: usize,
        const ROUTED_REQUESTS: usize,
        const ROUTED_REQUEST_BYTES: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    >
    PrnsNode<
        St,
        R,
        F,
        S,
        H,
        M,
        LANE_COUNT,
        INTERFACE_CAPACITY,
        NOTIFY,
        COMMANDS,
        LIFECYCLE,
        COMPLETIONS,
        ROUTED_REQUESTS,
        ROUTED_REQUEST_BYTES,
        REQUEST_COMPLETIONS,
        RESPONSE_BYTES,
    >
where
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
    H: Host,
    M: RawMutex + 'static,
{
    pub fn init_static<'d, D>(
        cell: &'static StaticCell<Self>,
        recipe: PrnsNodeRecipe<D, St, R, F, ManuallyAttached, S>,
        wiring: ManifoldWiring<
            M,
            LANE_COUNT,
            NOTIFY,
            COMMANDS,
            LIFECYCLE,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
        >,
        host: H,
    ) -> &'static mut Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        let (node, NoPersistence) = Self::init_static_with_persistence(cell, recipe, wiring, host);
        node
    }

    #[expect(
        unsafe_code,
        clippy::undocumented_unsafe_blocks,
        clippy::mut_from_ref,
        reason = "every PrnsNode field is initialized before the slot is exposed"
    )]
    pub fn init_static_with_persistence<'d, D, P>(
        cell: &'static StaticCell<Self>,
        recipe: PrnsNodeRecipe<D, St, R, F, ManuallyAttached, S, P>,
        wiring: ManifoldWiring<
            M,
            LANE_COUNT,
            NOTIFY,
            COMMANDS,
            LIFECYCLE,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
        >,
        host: H,
    ) -> (&'static mut Self, P)
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        const {
            assert!(
                INTERFACE_CAPACITY >= LANE_COUNT,
                "PrnsNode INTERFACE_CAPACITY must cover every manifold lane"
            );
        }
        let slot = cell.uninit();
        let ManifoldWiring {
            inbound,
            frame_accounting_statuses,
            egress,
            initial,
            ifacs,
            notify,
            commands,
            lifecycle,
            handle,
        } = wiring;
        let node = slot.as_mut_ptr();
        let persistence = unsafe {
            let assembled = &mut *core::ptr::addr_of_mut!((*node).node)
                .cast::<MaybeUninit<AssembledNode<St, R, F, S>>>();
            let (_, ManuallyAttached, persistence) = assemble_node_in_place(assembled, recipe);
            core::ptr::addr_of_mut!((*node).inbound).write(inbound);
            core::ptr::addr_of_mut!((*node).frame_accounting_statuses)
                .write(frame_accounting_statuses);
            core::ptr::addr_of_mut!((*node).egress).write(egress);
            core::ptr::addr_of_mut!((*node).notify).write(notify);
            core::ptr::addr_of_mut!((*node).commands).write(commands);
            core::ptr::addr_of_mut!((*node).lifecycle).write(lifecycle);
            core::ptr::addr_of_mut!((*node).handle).write(handle);
            core::ptr::addr_of_mut!((*node).host).write(host);
            core::ptr::addr_of_mut!((*node).descriptors).write(HeaplessVec::new());
            core::ptr::addr_of_mut!((*node).ifacs).write(ifacs);
            persistence
        };
        let node = unsafe { slot.assume_init_mut() };
        for descriptor in initial {
            if node.descriptors.push(descriptor).is_err() {
                unreachable!()
            }
        }
        (node, persistence)
    }

    pub fn new_with_request_capacity<'d, D>(
        recipe: PrnsNodeRecipe<D, St, R, F, ManuallyAttached, S>,
        wiring: ManifoldWiring<
            M,
            LANE_COUNT,
            NOTIFY,
            COMMANDS,
            LIFECYCLE,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
        >,
        host: H,
        _capacity: RequestRoutingCapacity<ROUTED_REQUESTS, ROUTED_REQUEST_BYTES>,
    ) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        Self::build(recipe, wiring, host)
    }

    fn build<'d, D>(
        recipe: PrnsNodeRecipe<D, St, R, F, ManuallyAttached, S>,
        wiring: ManifoldWiring<
            M,
            LANE_COUNT,
            NOTIFY,
            COMMANDS,
            LIFECYCLE,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
        >,
        host: H,
    ) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'d>>,
    {
        const {
            assert!(
                INTERFACE_CAPACITY >= LANE_COUNT,
                "PrnsNode INTERFACE_CAPACITY must cover every manifold lane"
            );
        }
        let (node, ManuallyAttached, NoPersistence) = assemble_node(recipe);
        let mut descriptors = HeaplessVec::new();
        for descriptor in wiring.initial {
            if descriptors.push(descriptor).is_err() {
                unreachable!()
            }
        }

        PrnsNode {
            node,
            inbound: wiring.inbound,
            frame_accounting_statuses: wiring.frame_accounting_statuses,
            egress: wiring.egress,
            notify: wiring.notify,
            commands: wiring.commands,
            lifecycle: wiring.lifecycle,
            handle: wiring.handle,
            host,
            descriptors,
            ifacs: wiring.ifacs,
        }
    }

    pub fn set_protocol_policy(&mut self, policy: crate::engine::EngineProtocolPolicy) {
        self.node.engine.set_protocol_policy(policy);
    }

    pub fn hold_remote_control_controller_identity(
        &mut self,
        secret: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    ) -> Result<IdentityHash, HoldIdentityError> {
        self.node.engine.hold_identity(secret)
    }

    #[must_use]
    pub fn handle(
        &self,
    ) -> PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
    {
        self.handle
    }

    /// Runs the manifold with the caller's interface and supervisor tasks.
    pub async fn run(self, drive: impl Future<Output = ()>) {
        self.run_with_inspection_store(&NoInterfaceInspectionStore, drive)
            .await;
    }

    /// Runs the node with the synchronous application decision used by destinations
    /// configured with [`ProofStrategy::ProveIf`](crate::routing::ProofStrategy::ProveIf).
    ///
    /// The closure lives in this future and is consulted inline after delivery is
    /// journaled. Prns allocates no policy table or per-packet decision state; capture a
    /// shared handle to application state when proof policy must change at runtime.
    pub async fn run_with_proof_decider<P>(self, should_prove: P, drive: impl Future<Output = ()>)
    where
        P: FnMut(&ProofRequest) -> bool,
    {
        self.run_with_inspection_store_and_proof_decider(
            &NoInterfaceInspectionStore,
            should_prove,
            drive,
        )
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

    pub async fn run_with_interface_store_and_proof_decider<
        P,
        const INTERFACES: usize,
        const PACKET_PHY_CAPACITY: usize,
        const PACKET_PHY_INDEX_BUCKETS: usize,
    >(
        self,
        store: &EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>,
        should_prove: P,
        drive: impl Future<Output = ()>,
    ) where
        M: Sync,
        P: FnMut(&ProofRequest) -> bool,
    {
        const {
            assert!(
                INTERFACES >= INTERFACE_CAPACITY,
                "EmbassyInterfaceStore INTERFACES must cover PrnsNode INTERFACE_CAPACITY"
            );
        }
        self.run_with_inspection_store_and_proof_decider(store, should_prove, drive)
            .await;
    }

    async fn run_with_inspection_store<Store>(self, store: &Store, drive: impl Future<Output = ()>)
    where
        Store: InterfaceInspectionStore,
    {
        self.run_with_inspection_store_and_proof_decider(store, |_| false, drive)
            .await;
    }

    async fn run_with_inspection_store_and_proof_decider<Store, P>(
        self,
        store: &Store,
        should_prove: P,
        drive: impl Future<Output = ()>,
    ) where
        Store: InterfaceInspectionStore,
        P: FnMut(&ProofRequest) -> bool,
    {
        let PrnsNode {
            node,
            mut inbound,
            frame_accounting_statuses,
            mut egress,
            notify,
            commands,
            lifecycle,
            handle,
            mut host,
            mut descriptors,
            mut ifacs,
        } = self;
        let AssembledNode {
            mut engine,
            state,
            mut on_event,
            request_endpoints: _,
        } = node;
        let request_channel =
            Channel::<M, RunnerRequest<ROUTED_REQUEST_BYTES>, ROUTED_REQUESTS>::new();
        let request_sender = request_channel.sender();
        let mut persistence = NoManifoldPersistence;
        let manifold = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                descriptors: &mut descriptors,
                ifacs: &mut ifacs,
                inbound: &mut inbound,
                frame_accounting_statuses: &frame_accounting_statuses,
                egress: &mut egress,
                notify,
                commands,
                lifecycle,
            },
            |journaled| {
                if let JournalRoute::Awaiter = handle.route_journaled(&journaled) {
                    return;
                }
                if let Some(request) = RunnerRequest::copy_from(&journaled) {
                    let _ = request_sender.try_send(request);
                }
                on_event(PrnsEvent::from(journaled), &state);
            },
            crate::manifold::AppDeciders {
                should_prove,
                should_accept_resource: |_| false,
            },
            store,
            &mut persistence,
        );
        let router = run_router::<
            St,
            R,
            M,
            COMMANDS,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
            ROUTED_REQUESTS,
            ROUTED_REQUEST_BYTES,
        >(&state, request_channel.receiver(), handle);
        join(join(manifold, router), drive).await;
    }

    /// Runs only the manifold for boards that schedule interfaces separately.
    pub async fn run_manifold(&mut self) {
        self.run_manifold_with_inspection_store(&NoInterfaceInspectionStore)
            .await;
    }

    /// Runs only the manifold with a synchronous application proof decision.
    pub async fn run_manifold_with_proof_decider<P>(&mut self, should_prove: P)
    where
        P: FnMut(&ProofRequest) -> bool,
    {
        let mut persistence = NoManifoldPersistence;
        self.run_manifold_with_inspection_store_and_persistence_and_proof_decider(
            &NoInterfaceInspectionStore,
            &mut persistence,
            should_prove,
        )
        .await;
    }

    pub async fn run_manifold_with_interface_store<
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
        self.run_manifold_with_inspection_store(store).await;
    }

    pub async fn run_manifold_with_interface_store_and_proof_decider<
        P,
        const INTERFACES: usize,
        const PACKET_PHY_CAPACITY: usize,
        const PACKET_PHY_INDEX_BUCKETS: usize,
    >(
        &mut self,
        store: &EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>,
        should_prove: P,
    ) where
        M: Sync,
        P: FnMut(&ProofRequest) -> bool,
    {
        const {
            assert!(
                INTERFACES >= INTERFACE_CAPACITY,
                "EmbassyInterfaceStore INTERFACES must cover PrnsNode INTERFACE_CAPACITY"
            );
        }
        let mut persistence = NoManifoldPersistence;
        self.run_manifold_with_inspection_store_and_persistence_and_proof_decider(
            store,
            &mut persistence,
            should_prove,
        )
        .await;
    }

    async fn run_manifold_with_inspection_store<Store>(&mut self, store: &Store)
    where
        Store: InterfaceInspectionStore,
    {
        let mut persistence = NoManifoldPersistence;
        self.run_manifold_with_inspection_store_and_persistence(store, &mut persistence)
            .await;
    }

    pub async fn run_manifold_with_persistence_and_interface_store<
        Fl,
        Keys,
        Observe,
        const PENDING: usize,
        const INTERFACES: usize,
        const PACKET_PHY_CAPACITY: usize,
        const PACKET_PHY_INDEX_BUCKETS: usize,
    >(
        &mut self,
        store: &EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>,
        persistence: &mut EmbeddedFlashPersistence<Fl, Keys, Observe, PENDING>,
    ) where
        M: Sync,
        Fl: NorFlash,
        Keys: RouteSnapshotKeys,
        Observe: FnMut(EmbeddedPersistenceDiagnostic),
    {
        const {
            assert!(
                INTERFACES >= INTERFACE_CAPACITY,
                "EmbassyInterfaceStore INTERFACES must cover PrnsNode INTERFACE_CAPACITY"
            );
        }
        self.run_manifold_with_inspection_store_and_persistence(store, persistence)
            .await;
    }

    pub async fn run_manifold_with_persistence_and_interface_store_and_proof_decider<
        Fl,
        Keys,
        Observe,
        Decide,
        const PENDING: usize,
        const INTERFACES: usize,
        const PACKET_PHY_CAPACITY: usize,
        const PACKET_PHY_INDEX_BUCKETS: usize,
    >(
        &mut self,
        store: &EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>,
        persistence: &mut EmbeddedFlashPersistence<Fl, Keys, Observe, PENDING>,
        should_prove: Decide,
    ) where
        M: Sync,
        Fl: NorFlash,
        Keys: RouteSnapshotKeys,
        Observe: FnMut(EmbeddedPersistenceDiagnostic),
        Decide: FnMut(&ProofRequest) -> bool,
    {
        const {
            assert!(
                INTERFACES >= INTERFACE_CAPACITY,
                "EmbassyInterfaceStore INTERFACES must cover PrnsNode INTERFACE_CAPACITY"
            );
        }
        self.run_manifold_with_inspection_store_and_persistence_and_proof_decider(
            store,
            persistence,
            should_prove,
        )
        .await;
    }

    pub async fn restore_embedded_persistence<Fl, Keys, Observe, const PENDING: usize>(
        &mut self,
        persistence: &mut EmbeddedFlashPersistence<Fl, Keys, Observe, PENDING>,
    ) -> EmbeddedPersistenceRestoreReport
    where
        Fl: NorFlash,
        Keys: RouteSnapshotKeys,
        Observe: FnMut(EmbeddedPersistenceDiagnostic),
        H: ResumableHost,
    {
        let report = persistence
            .restore(&mut self.node.engine, self.host.now())
            .await;
        self.host.resume_at(report.logical_start);
        report
    }

    async fn run_manifold_with_inspection_store_and_persistence<Store, P>(
        &mut self,
        store: &Store,
        persistence: &mut P,
    ) where
        Store: InterfaceInspectionStore,
        P: ManifoldPersistence<S>,
    {
        self.run_manifold_with_inspection_store_and_persistence_and_proof_decider(
            store,
            persistence,
            |_| false,
        )
        .await;
    }

    async fn run_manifold_with_inspection_store_and_persistence_and_proof_decider<
        Store,
        P,
        Decide,
    >(
        &mut self,
        store: &Store,
        persistence: &mut P,
        should_prove: Decide,
    ) where
        Store: InterfaceInspectionStore,
        P: ManifoldPersistence<S>,
        Decide: FnMut(&ProofRequest) -> bool,
    {
        let PrnsNode {
            node,
            inbound,
            frame_accounting_statuses,
            egress,
            notify,
            commands,
            lifecycle,
            handle,
            host,
            descriptors,
            ifacs,
        } = self;
        let AssembledNode {
            engine,
            state,
            on_event,
            request_endpoints: _,
        } = node;
        let request_channel =
            Channel::<M, RunnerRequest<ROUTED_REQUEST_BYTES>, ROUTED_REQUESTS>::new();
        let request_sender = request_channel.sender();
        let manifold = run_pooled(
            engine,
            host,
            PooledWiring {
                descriptors,
                ifacs,
                inbound,
                frame_accounting_statuses,
                egress,
                notify: *notify,
                commands: *commands,
                lifecycle: *lifecycle,
            },
            |journaled| {
                if let JournalRoute::Awaiter = handle.route_journaled(&journaled) {
                    return;
                }
                if let Some(request) = RunnerRequest::copy_from(&journaled) {
                    let _ = request_sender.try_send(request);
                }
                on_event(PrnsEvent::from(journaled), state);
            },
            crate::manifold::AppDeciders {
                should_prove,
                should_accept_resource: |_| false,
            },
            store,
            persistence,
        );
        let router = run_router::<
            St,
            R,
            M,
            COMMANDS,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
            ROUTED_REQUESTS,
            ROUTED_REQUEST_BYTES,
        >(state, request_channel.receiver(), *handle);
        join(manifold, router).await;
    }
}

#[cfg(test)]
mod tests;
