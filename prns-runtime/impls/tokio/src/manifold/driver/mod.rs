use std::future::poll_fn;
use std::sync::Arc;
use std::task::Poll;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::engine::{EngineState, InstantMillis, Journaled, NextWake, ProofRequest, WakeReason};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::manifold::wake_schedule::{fire_due_reason, merge_wake_schedules_delta};
use crate::manifold::AppDeciders;
use crate::manifold::Host;
use crate::routing::links::resources::send::ResourceSealWorkspacePolicy;
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
mod local_command_lane;
mod owed_work;
#[cfg(feature = "runtime-metrics")]
mod scheduling_metrics;
mod scheduling_policy;

pub use super::grant_lane::{
    tokio_grant_lane, HeapFrameSlot, TokioGrantConsumer, TokioGrantProducer,
};
pub use crypto_pool::{CryptoPoolConfig, PoolWorkers};
pub use egress::Egress;
pub(crate) use host::TokioEntropy;
pub use host::{TokioClock, TokioHost};
pub(crate) use host_protocol::HostResourceDigestPreparation;
pub use host_protocol::{
    AddInterfaceCommand, HostCommand, HostResourceMetadata, HostResourcePayload,
    HostResourcePayloadError, ProvideDecompressedHostCommand, RequestAnyHostCommand,
    ResourceInbound, RespondAnyHostCommand, SendResourceHostCommand,
    SendResourceSegmentHostCommand, StreamInbound,
};
pub use interface_seam::TokioInterfaceSeam;
pub use interface_status::TokioInterfaceStatus;
pub(crate) use local_command_lane::{
    local_command_lane, LocalCommandConsumer, LocalCommandProducer,
};
pub use prns_runtime::runtime::{
    PersistedStateSnapshot, SelfRatchetSnapshot, SelfRatchetsSnapshot,
};
#[cfg(not(feature = "scheduler-tuning"))]
pub(crate) use scheduling_policy::SchedulerPolicy;
#[cfg(feature = "scheduler-tuning")]
pub use scheduling_policy::{SchedulerPolicy, SchedulerPolicyError, SchedulerPolicyInput};

use command_dispatch::{CommandDispatch, CommandEffect};
use crypto_dispatch::{CryptoCompletionEffect, CryptoDispatch};
use crypto_pool::{CryptoCompletion, CryptoPool};
use egress::{flush_due_pacers, route_reaction, soonest_pacer_release, WireScratch};
use host::ManifoldClock;
use inbound_dispatch::{InboundContext, InboundDispatch};
use interface_topology::InterfaceTopology;
use journal_delivery::JournalDispatch;
use owed_work::PendingOwedWork;
#[cfg(feature = "runtime-metrics")]
use scheduling_metrics::{ManifoldMetrics, TurnActivity};

trait CommandLane {
    fn enabled(&self) -> bool;
    fn try_recv(&mut self) -> Option<HostCommand>;
    fn poll_recv(&mut self, context: &mut std::task::Context<'_>) -> Poll<Option<HostCommand>>;
}

struct NoLocalCommands;

impl CommandLane for NoLocalCommands {
    fn enabled(&self) -> bool {
        false
    }

    fn try_recv(&mut self) -> Option<HostCommand> {
        None
    }

    fn poll_recv(&mut self, _context: &mut std::task::Context<'_>) -> Poll<Option<HostCommand>> {
        Poll::Pending
    }
}

impl CommandLane for LocalCommandConsumer {
    fn enabled(&self) -> bool {
        true
    }

    fn try_recv(&mut self) -> Option<HostCommand> {
        LocalCommandConsumer::try_recv(self)
    }

    fn poll_recv(&mut self, context: &mut std::task::Context<'_>) -> Poll<Option<HostCommand>> {
        LocalCommandConsumer::poll_recv(self, context)
    }
}

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
        NoLocalCommands,
        CryptoPoolConfig::host_default(),
        SchedulerPolicy::production(),
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
        NoLocalCommands,
        crypto_pool_config,
        SchedulerPolicy::production(),
    )
    .await
}

pub(crate) async fn run_executor_local_with_store_and_deciders<S, H, J, P, A>(
    engine: EngineState<S>,
    host: H,
    wiring: ManifoldWiring,
    local_commands: LocalCommandConsumer,
    on_journaled: J,
    store: InterfaceStore,
    crypto_pool_config: CryptoPoolConfig,
    scheduler_policy: SchedulerPolicy,
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
        local_commands,
        crypto_pool_config,
        scheduler_policy,
    )
    .await
}

async fn run_inner<S, H, J, P, A, C>(
    mut engine: EngineState<S>,
    mut host: H,
    wiring: ManifoldWiring,
    on_journaled: J,
    deciders: AppDeciders<P, A>,
    store: Option<InterfaceStore>,
    mut local_commands: C,
    crypto_pool_config: CryptoPoolConfig,
    scheduler_policy: SchedulerPolicy,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
    C: CommandLane,
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
    let mut owed_work = PendingOwedWork::new();
    let mut inline_crypto_completions = std::vec::Vec::new();
    #[cfg(feature = "runtime-metrics")]
    let mut manifold_metrics = ManifoldMetrics::default();
    macro_rules! journaled_sink {
        () => {
            |journaled| journal.route(journaled)
        };
    }
    const LOCAL_COMMAND_BURST: usize = 32;
    let crypto_completion_wake = Arc::new(tokio::sync::Notify::new());
    let crypto_pool = crypto_pool_config
        .resolved_worker_count()
        .and_then(|workers| {
            CryptoPool::spawn_with_policy(
                workers.get(),
                crypto_completion_wake.clone(),
                scheduler_policy,
            )
        });
    engine.resource_seal_workspace_policy = match crypto_pool.as_ref() {
        Some(_) => ResourceSealWorkspacePolicy::TransferOwnership,
        None => ResourceSealWorkspacePolicy::Borrowed,
    };
    let mut clock = ManifoldClock::new(&host);
    let due_timer = tokio::time::sleep_until(clock.immediate_deadline());
    tokio::pin!(due_timer);
    let mut armed: Option<(InstantMillis, WakeReason)> = None;
    let pacer_timer = tokio::time::sleep_until(clock.immediate_deadline());
    tokio::pin!(pacer_timer);
    let mut pacer_armed: Option<InstantMillis> = None;
    let mut pending_command = None;
    let mut local_command_streak = 0usize;
    let mut local_commands_enabled = local_commands.enabled();
    let mut hot_turns = 0usize;
    loop {
        #[cfg(feature = "runtime-metrics")]
        let turn_started = std::time::Instant::now();
        #[cfg(feature = "runtime-metrics")]
        let mut turn_activity = TurnActivity::default();
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
            pending_command = next_command(
                &mut local_commands,
                &mut commands,
                &mut local_command_streak,
                LOCAL_COMMAND_BURST,
            );
        }

        let mut progressed = false;
        let mut work_remaining = scheduler_policy.turn_work();
        if let Some(pool) = crypto_pool.as_ref().filter(|pool| pool.has_completion()) {
            pool.disarm_completion_wait();
            let mut next = pool.pop_completion();
            let now = clock.observe_step(&host);
            let mut seal_buf = [0u8; crate::wire::BROADCAST_MTU];
            let mut completed = 0;
            while let Some(result) = next {
                let completed_work = result.work.max(1);
                let effect = CryptoDispatch {
                    engine: &mut engine,
                    host: &mut host,
                    topology: &mut topology,
                    wire_scratch: &mut wire_scratch,
                    journal: &mut journal,
                    crypto_pool: crypto_pool.as_ref(),
                    owed_work: &mut owed_work,
                    inbound: &mut inbound,
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
                    }
                }
                work_remaining = work_remaining.saturating_sub(completed_work);
                completed += 1;
                if work_remaining == 0 || completed == scheduler_policy.completion_batch() {
                    break;
                }
                next = pool.pop_completion();
            }
            #[cfg(feature = "runtime-metrics")]
            {
                turn_activity.completions = completed;
            }
            progressed = true;
        }

        if work_remaining > 0 && inbound.has_ready_lanes() {
            let now = clock.observe_step(&host);
            let processed = inbound.process(InboundContext {
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
                max_frames_per_lane: scheduler_policy.inbound_per_lane(),
                max_frames_total: work_remaining.min(scheduler_policy.inbound_total()),
                owed_work: &mut owed_work,
                now,
            });
            work_remaining = work_remaining.saturating_sub(processed);
            #[cfg(feature = "runtime-metrics")]
            {
                turn_activity.inbound_frames = processed;
            }
            progressed |= processed > 0;
        }

        if work_remaining > 0 {
            if let Some(mut issued) = pending_command.take() {
                let now = clock.observe_step(&host);
                let mut command_budget = work_remaining.min(scheduler_policy.command_batch());
                let initial_command_budget = command_budget;
                loop {
                    let effect = CommandDispatch {
                        engine: &mut engine,
                        host: &mut host,
                        topology: &mut topology,
                        wire_scratch: &mut wire_scratch,
                        journal: &mut journal,
                        crypto_pool: crypto_pool.as_ref(),
                        owed_work: &mut owed_work,
                        #[cfg(feature = "runtime-metrics")]
                        manifold_metrics: &manifold_metrics,
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
                    match next_command(
                        &mut local_commands,
                        &mut commands,
                        &mut local_command_streak,
                        LOCAL_COMMAND_BURST,
                    ) {
                        Some(next) => issued = next,
                        None => break,
                    }
                }
                let commands_dispatched = initial_command_budget.saturating_sub(command_budget);
                work_remaining = work_remaining.saturating_sub(commands_dispatched);
                #[cfg(feature = "runtime-metrics")]
                {
                    turn_activity.commands = commands_dispatched;
                }
                progressed = true;
            }
        }

        if armed.is_some_and(|(deadline, _)| deadline <= clock.now()) {
            if let Some((_deadline, reason)) = armed.take() {
                let now = clock.observe_step(&host);
                #[cfg(feature = "runtime-metrics")]
                manifold_metrics.record_timer_lateness(_deadline.0, now.0);
                let wake_schedules_delta = fire_due_reason(
                    &mut engine,
                    reason,
                    now,
                    topology.interfaces.view(),
                    &mut |bytes| host.fill_random(bytes),
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
            let _deadline = pacer_armed.take().unwrap_or(InstantMillis(0));
            let now = clock.observe_step(&host);
            #[cfg(feature = "runtime-metrics")]
            manifold_metrics.record_pacer_lateness(_deadline.0, now.0);
            flush_due_pacers(
                &mut topology.pacers,
                now,
                &mut topology.egress,
                &topology.ifacs,
            );
            progressed = true;
        }

        if work_remaining > 0 {
            #[cfg(feature = "runtime-metrics")]
            let inline_dispatch_started = crypto_pool.is_none().then(std::time::Instant::now);
            let dispatched = owed_work.dispatch(
                &mut host,
                crypto_pool.as_ref(),
                &mut inline_crypto_completions,
                if crypto_pool.is_some() {
                    work_remaining.min(scheduler_policy.owed_work_batch())
                } else {
                    1
                },
            );
            work_remaining = work_remaining.saturating_sub(dispatched);
            #[cfg(feature = "runtime-metrics")]
            {
                turn_activity.owed_work = dispatched;
                if dispatched > 0 {
                    if let Some(started_at) = inline_dispatch_started {
                        manifold_metrics.record_inline_work(started_at, dispatched);
                    }
                }
            }
            progressed |= dispatched > 0;
        }
        if !inline_crypto_completions.is_empty() {
            let now = clock.observe_step(&host);
            let mut seal_buf = [0u8; crate::wire::BROADCAST_MTU];
            for result in inline_crypto_completions.drain(..) {
                let effect = CryptoDispatch {
                    engine: &mut engine,
                    host: &mut host,
                    topology: &mut topology,
                    wire_scratch: &mut wire_scratch,
                    journal: &mut journal,
                    crypto_pool: crypto_pool.as_ref(),
                    owed_work: &mut owed_work,
                    inbound: &mut inbound,
                }
                .complete(
                    CryptoCompletion {
                        worker: None,
                        result,
                        class: crypto_pool::CryptoJobClass::Latency,
                        work: 0,
                        timing: crypto_pool::CompletedJobTiming::unmeasured(),
                    },
                    now,
                    &mut seal_buf,
                    &mut should_prove,
                );
                match effect {
                    CryptoCompletionEffect::NoWakeChange => {}
                    CryptoCompletionEffect::WakeSchedules(delta)
                    | CryptoCompletionEffect::OpenSpanAdvanced(delta) => {
                        merge_wake_schedules_delta(
                            &mut wake_schedules,
                            delta,
                            &engine,
                            topology.view(),
                        );
                    }
                }
            }
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

        #[cfg(feature = "runtime-metrics")]
        manifold_metrics.record_turn(turn_started, turn_activity, work_remaining == 0);

        if progressed {
            if work_remaining == 0 {
                hot_turns = 0;
                tokio::task::yield_now().await;
                continue;
            }
            hot_turns += 1;
            if hot_turns >= scheduler_policy.hot_turns() {
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
            local_issued = poll_fn(|context| local_commands.poll_recv(context)),
                if local_commands_enabled => {
                match local_issued {
                    Some(issued) => {
                        local_command_streak = local_command_streak.saturating_add(1);
                        pending_command = Some(issued);
                    }
                    None => local_commands_enabled = false,
                }
            }
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

fn next_command<C: CommandLane>(
    local: &mut C,
    shared: &mut UnboundedReceiver<HostCommand>,
    local_streak: &mut usize,
    local_burst: usize,
) -> Option<HostCommand> {
    if *local_streak >= local_burst {
        *local_streak = 0;
        if let Ok(command) = shared.try_recv() {
            return Some(command);
        }
    }
    if let Some(command) = local.try_recv() {
        *local_streak += 1;
        return Some(command);
    }
    *local_streak = 0;
    shared.try_recv().ok()
}

#[cfg(test)]
mod tests;
