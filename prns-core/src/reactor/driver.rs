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
use crate::storage::StorageLayout;

#[cfg_attr(not(feature = "embassy-host"), allow(dead_code))]
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

pub fn fire_due_lane<S, F>(
    engine: &mut EngineState<S>,
    lane: DueLane,
    now: InstantMillis,
    interfaces: &[InterfaceConfig],
    fill_entropy: &mut F,
    on_reaction: &mut impl FnMut(EngineReaction<'_>),
) -> WakeSchedules
where
    S: StorageLayout,
    F: FnMut(&mut [u8]),
{
    match lane {
        DueLane::ScheduledAnnounces => {
            engine.fire_due_scheduled_announces(now, interfaces, on_reaction)
        }
        DueLane::ReceiptTimeouts => engine.settle_timed_out_receipts(now, on_reaction),
        DueLane::PathRequestTimeout => engine.settle_timed_out_path_requests(now, on_reaction),
        DueLane::ExpiredRoutes => engine.cull_expired_routes(now, interfaces, on_reaction),
        DueLane::LinkDeadlines => {
            engine.fire_due_link_deadlines(now, interfaces, fill_entropy, on_reaction)
        }
        DueLane::ResourceDeadlines => {
            engine.fire_due_resource_deadlines(now, fill_entropy, on_reaction)
        }
        DueLane::ChannelTimeouts => {
            engine.fire_due_channel_timeouts(now, interfaces, fill_entropy, on_reaction)
        }
        DueLane::HeldAnnounceRelease => {
            engine.fire_due_held_announces(now, interfaces, fill_entropy, on_reaction)
        }
    }
}

pub fn draw_jitter<H: Host>(host: &mut H) -> JitterSeed {
    let mut bytes = [0u8; core::mem::size_of::<u64>()];
    host.fill_entropy(&mut bytes);
    JitterSeed(u64::from_le_bytes(bytes))
}

pub fn merge_wake_schedules_delta<S: StorageLayout>(
    source_wake_schedules: &mut WakeSchedules,
    delta: WakeSchedules,
    engine: &EngineState<S>,
    view: &[InterfaceConfig],
) {
    source_wake_schedules.merge(delta);
    #[cfg(debug_assertions)]
    {
        let truth = engine.wake_schedules(view);
        debug_assert_eq!(
            source_wake_schedules.scheduled_announces, truth.scheduled_announces,
            "the rebroadcast lane drifted from a full recompute",
        );
        debug_assert_eq!(
            source_wake_schedules.receipt_timeouts, truth.receipt_timeouts,
            "the send-timeout lane drifted from a full recompute",
        );
        debug_assert_eq!(
            source_wake_schedules.path_request_timeout, truth.path_request_timeout,
            "the path-timeout lane drifted from a full recompute",
        );
        debug_assert!(
            never_late(source_wake_schedules.link_deadlines, truth.link_deadlines),
            "the link-deadline lane must never sit later than the truth: cached {:?}, truth {:?}",
            source_wake_schedules.link_deadlines,
            truth.link_deadlines,
        );
        debug_assert_eq!(
            source_wake_schedules.resource_deadlines, truth.resource_deadlines,
            "the resource-deadline lane drifted from a full recompute",
        );
        debug_assert_eq!(
            source_wake_schedules.channel_timeouts, truth.channel_timeouts,
            "the channel-timeout lane drifted from a full recompute",
        );
        debug_assert!(
            never_late(source_wake_schedules.expired_routes, truth.expired_routes),
            "the expired-routes lane must never sit later than the truth: cached {:?}, truth {:?}",
            source_wake_schedules.expired_routes,
            truth.expired_routes,
        );
        debug_assert_eq!(
            source_wake_schedules.held_announce_release, truth.held_announce_release,
            "the held-announce-release lane drifted from a full recompute",
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = (engine, view);
}

/// The expired-routes lane runs on `AtMost` deltas, so its cached deadline may sit
/// EARLY of the truth (a removal or refresh pushed the true deadline later) but never
/// late — a premature wake costs one no-op cull whose full recompute resyncs the lane;
/// a late one would miss a deadline.
#[cfg(debug_assertions)]
fn never_late(cached: crate::engine::LaneWake, truth: crate::engine::LaneWake) -> bool {
    use crate::engine::LaneWake::{At, Idle};
    match (cached, truth) {
        (At(cached_at), At(truth_at)) => cached_at <= truth_at,
        (At(_), Idle) => true,
        (Idle, Idle) => true,
        _ => cached == truth,
    }
}
