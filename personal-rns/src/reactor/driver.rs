//! The host-agnostic core of the reactor's timer edge: pick the scheduled lane that came
//! due and fire it. Both the tokio and embassy drivers share this — the only thing that
//! differs between hosts is the channel and select primitives, never which engine method a
//! [`DueLane`] names. Keeping the dispatch here is what makes "honor the shape" literal:
//! one table, two hosts.

use super::Host;
use crate::engine::{
    DueLane, EngineReaction, EngineState, InstantMillis, ScheduledWake, WakeSchedules,
};
use crate::interfaces::InterfaceConfig;
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::storage::EngineStorage;

pub async fn wait_for_due_lane<H: Host>(host: &H, scheduled_wake: ScheduledWake) -> DueLane {
    match scheduled_wake {
        ScheduledWake::Idle => core::future::pending().await,
        ScheduledWake::Due(lane) => lane,
        ScheduledWake::At { at, lane } => {
            host.sleep_until(at).await;
            lane
        }
    }
}

pub async fn wait_for_pacer<H: Host>(host: &H, deadline: Option<InstantMillis>) {
    match deadline {
        Some(at) => host.sleep_until(at).await,
        None => core::future::pending().await,
    }
}

pub fn fire_due_lane<S>(
    engine: &mut EngineState<S>,
    lane: DueLane,
    now: InstantMillis,
    interfaces: &[InterfaceConfig],
    on_reaction: &mut impl FnMut(EngineReaction<'_>),
) -> WakeSchedules
where
    S: EngineStorage,
{
    match lane {
        DueLane::RebroadcastAnnounces => {
            engine.fire_due_announce_rebroadcasts(now, interfaces, on_reaction)
        }
        DueLane::SendSingleTimeout => engine.settle_timed_out_send_singles(now, on_reaction),
        DueLane::PathRequestTimeout => engine.settle_timed_out_path_requests(now, on_reaction),
        DueLane::ExpiredRoutes => engine.cull_expired_routes(now, interfaces, on_reaction),
    }
}

pub fn draw_jitter<H: Host>(host: &mut H) -> JitterSeed {
    let mut bytes = [0u8; core::mem::size_of::<u64>()];
    host.fill_entropy(&mut bytes);
    JitterSeed(u64::from_le_bytes(bytes))
}

pub fn merge_wake_schedules_delta<S: EngineStorage>(
    source_wake_schedules: &mut WakeSchedules,
    delta: WakeSchedules,
    engine: &EngineState<S>,
    view: &[InterfaceConfig],
) {
    source_wake_schedules.merge(delta);
    debug_assert_eq!(
        *source_wake_schedules,
        engine.wake_schedules(view),
        "the incremental wake schedules drifted from a full recompute",
    );
}
