use std::sync::Arc;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::engine::{EngineState, InstantMillis, Journaled, NextWake, ProofRequest, WakeReason};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::manifold::kernel::{fire_due_reason, merge_wake_schedules_delta};
use crate::manifold::AppDeciders;
use crate::manifold::Host;
use crate::routing::links::resources::streamed_open::ResourceOpenLane;
use crate::routing::links::resources::ResourceOffer;
use crate::runtime::InterfaceStore;
use crate::storage::{DirtyInterfaceSet, StorageLayout};

mod command_dispatch;
mod crypto_dispatch;
mod crypto_pool;
mod egress;
mod host;
mod host_protocol;
mod inbound_dispatch;
mod interface_seam;
mod interface_status;
mod interface_topology;
mod journal_delivery;

pub use super::grant_lane::{
    tokio_grant_lane, HeapFrameSlot, TokioGrantConsumer, TokioGrantProducer,
};
pub use crypto_pool::{CryptoPoolConfig, PoolWorkers};
pub use egress::Egress;
pub(crate) use host::TokioEntropy;
pub use host::TokioHost;
pub use host_protocol::{
    AddInterfaceCommand, HostCommand, HostResourceMetadata, HostResourcePayload,
    HostResourcePayloadError, ProvideDecompressedHostCommand, RequestAnyHostCommand,
    ResourceInbound, RespondAnyHostCommand, SendResourceHostCommand,
    SendResourceSegmentHostCommand, StreamInbound,
};
pub use interface_seam::TokioInterfaceSeam;
pub use interface_status::TokioInterfaceStatus;
pub use prns_runtime::runtime::{
    PersistedStateSnapshot, SelfRatchetSnapshot, SelfRatchetsSnapshot,
};

use command_dispatch::{CommandDispatch, CommandEffect};
use crypto_dispatch::{dispatch_open_spans, CryptoCompletionEffect, CryptoDispatch};
use crypto_pool::CryptoPool;
use egress::{flush_due_pacers, route_reaction, soonest_pacer_release, WireScratch};
use host::ManifoldClock;
use inbound_dispatch::{InboundContext, InboundDispatch};
use interface_topology::InterfaceTopology;
use journal_delivery::JournalDispatch;

/// Everything the manifold is wired to for one run: the interface topology snapshot, per-interface IFAC state, the wake and command channels, the inbound grant lanes, and the egress fan-out.
pub struct ManifoldWiring {
    pub interfaces: std::vec::Vec<InterfaceDescriptor>,
    pub ifacs: std::vec::Vec<InterfaceIfac>,
    pub notify: UnboundedReceiver<InterfaceId>,
    pub inbound_lanes: std::vec::Vec<(InterfaceId, TokioGrantConsumer)>,
    pub commands: UnboundedReceiver<HostCommand>,
    pub egress: Egress,
}

pub async fn run<S, H, J>(engine: EngineState<S>, host: H, wiring: ManifoldWiring, on_journaled: J)
where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
{
    run_with_deciders(
        engine,
        host,
        wiring,
        on_journaled,
        crate::manifold::decline_all(),
    )
    .await
}

pub async fn run_with_deciders<S, H, J, P, A>(
    engine: EngineState<S>,
    host: H,
    wiring: ManifoldWiring,
    on_journaled: J,
    deciders: AppDeciders<P, A>,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        deciders,
        None,
        CryptoPoolConfig::host_default(),
    )
    .await
}

pub async fn run_with_store<S, H, J>(
    engine: EngineState<S>,
    host: H,
    wiring: ManifoldWiring,
    on_journaled: J,
    store: InterfaceStore,
    crypto_pool_config: CryptoPoolConfig,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
{
    run_with_store_and_deciders(
        engine,
        host,
        wiring,
        on_journaled,
        store,
        crypto_pool_config,
        crate::manifold::decline_all(),
    )
    .await
}

pub async fn run_with_store_and_deciders<S, H, J, P, A>(
    engine: EngineState<S>,
    host: H,
    wiring: ManifoldWiring,
    on_journaled: J,
    store: InterfaceStore,
    crypto_pool_config: CryptoPoolConfig,
    deciders: AppDeciders<P, A>,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        deciders,
        Some(store),
        crypto_pool_config,
    )
    .await
}

async fn run_inner<S, H, J, P, A>(
    mut engine: EngineState<S>,
    mut host: H,
    wiring: ManifoldWiring,
    on_journaled: J,
    deciders: AppDeciders<P, A>,
    store: Option<InterfaceStore>,
    crypto_pool_config: CryptoPoolConfig,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    let AppDeciders {
        mut should_prove,
        mut should_accept_resource,
    } = deciders;
    let ManifoldWiring {
        interfaces,
        ifacs,
        mut notify,
        inbound_lanes,
        mut commands,
        egress,
    } = wiring;
    let mut topology =
        InterfaceTopology::new(interfaces, ifacs, inbound_lanes, egress, &mut engine, &host);
    let mut wake_schedules = engine.wake_schedules(topology.view());
    let frame_capacity = topology.frame_cap();
    let mut wire_scratch = WireScratch::new(frame_capacity);
    let mut inbound = InboundDispatch::new(frame_capacity);
    let mut journal = JournalDispatch::new(on_journaled);
    macro_rules! journaled_sink {
        () => {
            |journaled| journal.route(journaled)
        };
    }
    const MAX_INBOUND_BATCH: usize = 64;
    const MAX_COMMAND_BATCH: usize = 64;
    // Keep the single-owner manifold hot while work remains durable, but regularly return control
    // to the sibling interface and request futures that replenish its SPSC lanes.
    const HOT_TURNS_BEFORE_YIELD: usize = 16;
    let crypto_completion_wake = Arc::new(tokio::sync::Notify::new());
    let crypto_pool = crypto_pool_config
        .resolved_worker_count()
        .and_then(|workers| CryptoPool::spawn(workers.get(), crypto_completion_wake.clone()));
    if crypto_pool.is_some() {
        engine.resource_open_lane = ResourceOpenLane::PoolWhenContended;
    }
    let mut clock = ManifoldClock::new(&host);
    let due_timer = tokio::time::sleep_until(clock.immediate_deadline());
    tokio::pin!(due_timer);
    let mut armed: Option<(InstantMillis, WakeReason)> = None;
    let pacer_timer = tokio::time::sleep_until(clock.immediate_deadline());
    tokio::pin!(pacer_timer);
    let mut pacer_armed: Option<InstantMillis> = None;
    let mut pending_command = None;
    let mut hot_turns = 0usize;
    loop {
        match soonest_pacer_release(&topology.pacers) {
            None => pacer_armed = None,
            Some(at) => {
                if pacer_armed != Some(at) {
                    pacer_timer.as_mut().reset(clock.timer_deadline(at));
                }
                pacer_armed = Some(at);
            }
        }
        match wake_schedules.soonest(clock.now()) {
            NextWake::Idle => armed = None,
            NextWake::Due(reason) => {
                due_timer.as_mut().reset(clock.immediate_deadline());
                armed = Some((InstantMillis(0), reason));
            }
            NextWake::At { at, reason } => {
                if armed.map(|(deadline, _)| deadline) != Some(at) {
                    due_timer.as_mut().reset(clock.timer_deadline(at));
                }
                armed = Some((at, reason));
            }
        }
        // Announcements carry only lane identity. Pull every already-durable notification without
        // registering a Tokio waiter; the SPSC lane remains the source of truth for frame data.
        inbound.collect_ready(&mut notify);
        if pending_command.is_none() {
            pending_command = commands.try_recv().ok();
        }

        let mut progressed = false;
        if let Some(pool) = crypto_pool.as_ref().filter(|pool| pool.has_completion()) {
            pool.disarm_completion_wait();
            let mut next = pool.pop_completion();
            let now = clock.observe_step(&host);
            let mut seal_buf = [0u8; crate::wire::BROADCAST_MTU];
            while let Some(result) = next {
                let effect = CryptoDispatch {
                    engine: &mut engine,
                    host: &mut host,
                    topology: &mut topology,
                    wire_scratch: &mut wire_scratch,
                    journal: &mut journal,
                    crypto_pool: crypto_pool.as_ref(),
                }
                .complete(result, now, &mut seal_buf, &mut should_prove);
                match effect {
                    CryptoCompletionEffect::NoWakeChange => {}
                    CryptoCompletionEffect::WakeSchedules(delta) => {
                        merge_wake_schedules_delta(
                            &mut wake_schedules,
                            delta,
                            &engine,
                            topology.view(),
                        );
                    }
                    CryptoCompletionEffect::OpenSpanAdvanced(delta) => {
                        merge_wake_schedules_delta(
                            &mut wake_schedules,
                            delta,
                            &engine,
                            topology.view(),
                        );
                        dispatch_open_spans(&mut engine, crypto_pool.as_ref());
                    }
                }
                next = pool.pop_completion();
            }
            dispatch_open_spans(&mut engine, crypto_pool.as_ref());
            progressed = true;
        }

        if inbound.has_ready_lanes() {
            let now = clock.observe_step(&host);
            inbound.process(InboundContext {
                engine: &mut engine,
                host: &mut host,
                topology: &mut topology,
                wire_scratch: &mut wire_scratch,
                journal: &mut journal,
                crypto_pool: crypto_pool.as_ref(),
                packet_phy_store: store.as_ref(),
                wake_schedules: &mut wake_schedules,
                should_prove: &mut should_prove,
                should_accept_resource: &mut should_accept_resource,
                max_frames_per_lane: MAX_INBOUND_BATCH,
                now,
            });
            progressed = true;
        }

        if !inbound.has_ready_lanes() && engine.owed_staged_seal_link().is_some() {
            let now = clock.observe_step(&host);
            CryptoDispatch {
                engine: &mut engine,
                host: &mut host,
                topology: &mut topology,
                wire_scratch: &mut wire_scratch,
                journal: &mut journal,
                crypto_pool: crypto_pool.as_ref(),
            }
            .dispatch_staged_seal(now);
            progressed = true;
        }

        if let Some(mut issued) = pending_command.take() {
            let now = clock.observe_step(&host);
            let mut command_budget = MAX_COMMAND_BATCH;
            loop {
                let effect = CommandDispatch {
                    engine: &mut engine,
                    host: &mut host,
                    topology: &mut topology,
                    wire_scratch: &mut wire_scratch,
                    journal: &mut journal,
                    crypto_pool: crypto_pool.as_ref(),
                }
                .dispatch(issued, now);
                match effect {
                    CommandEffect::Delta(delta) => merge_wake_schedules_delta(
                        &mut wake_schedules,
                        delta,
                        &engine,
                        topology.view(),
                    ),
                    CommandEffect::RecomputeWakeSchedules => {
                        wake_schedules = engine.wake_schedules(topology.view());
                    }
                    CommandEffect::InterfaceAttached { id, frame_capacity } => {
                        inbound.grow_frame_capacity(frame_capacity);
                        inbound.mark_ready(id);
                        wire_scratch.grow(frame_capacity);
                        wake_schedules = engine.wake_schedules(topology.view());
                    }
                }
                command_budget -= 1;
                if command_budget == 0 {
                    break;
                }
                match commands.try_recv() {
                    Ok(next) => issued = next,
                    Err(_) => break,
                }
            }
            progressed = true;
        }

        if armed.is_some_and(|(deadline, _)| deadline <= clock.now()) {
            if let Some((_, reason)) = armed.take() {
                let now = clock.observe_step(&host);
                let wake_schedules_delta = fire_due_reason(
                    &mut engine,
                    reason,
                    now,
                    topology.interfaces.view(),
                    &mut |bytes| host.fill_entropy(bytes),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            &mut wire_scratch,
                            now,
                            &mut journaled_sink!(),
                        )
                    },
                );
                merge_wake_schedules_delta(
                    &mut wake_schedules,
                    wake_schedules_delta,
                    &engine,
                    topology.view(),
                );
                progressed = true;
            }
        }

        if pacer_armed.is_some_and(|deadline| deadline <= clock.now()) {
            pacer_armed = None;
            let now = clock.observe_step(&host);
            flush_due_pacers(
                &mut topology.pacers,
                now,
                &mut topology.egress,
                &topology.ifacs,
            );
            progressed = true;
        }

        if let Some(store) = &store {
            let mut dirty_interfaces = engine.take_dirty_interfaces();
            let mut changed = false;
            dirty_interfaces.drain(|interface| {
                if topology.view().descriptor_for(interface).is_some() {
                    store.set(interface, engine.interface_counts(interface));
                } else {
                    store.forget(interface);
                }
                changed = true;
            });
            if changed {
                store.bump();
            }
        }

        if progressed {
            hot_turns += 1;
            if hot_turns >= HOT_TURNS_BEFORE_YIELD {
                hot_turns = 0;
                tokio::task::yield_now().await;
            }
            continue;
        }

        hot_turns = 0;
        if crypto_pool
            .as_ref()
            .is_some_and(CryptoPool::take_packet_verdict_hot_turn)
        {
            tokio::task::yield_now().await;
            continue;
        }

        // `Notify` is only the cold hole-punch into Tokio. Ring ownership and this durable count
        // carry the actual completion, so permit coalescing cannot strand work. Arming happens only
        // after every synchronously observable source has reported cold.
        if crypto_pool
            .as_ref()
            .is_some_and(CryptoPool::prepare_completion_wait)
        {
            continue;
        }

        tokio::select! {
            arrived = notify.recv() => {
                let Some(source) = arrived else { return };
                inbound.mark_ready(source);
            }
            issued = commands.recv() => {
                let Some(issued) = issued else { return };
                pending_command = Some(issued);
            }
            () = &mut due_timer, if armed.is_some() => {
                armed = None;
                clock.observe_step(&host);
            }
            () = &mut pacer_timer, if pacer_armed.is_some() => {
                pacer_armed = None;
                clock.observe_step(&host);
            }
            () = crypto_completion_wake.notified(), if crypto_pool.is_some() => {}
        }
    }
}

#[cfg(test)]
mod tests;
