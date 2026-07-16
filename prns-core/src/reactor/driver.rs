//! The host-agnostic core of the reactor's timer edge: pick the wake reason that came due and fire it. The tokio and embassy drivers differ only in channel and select primitives, never in which engine method a [`WakeReason`] names: one table, two hosts.

use super::Host;
use crate::engine::{
    EngineReaction, EngineState, InstantMillis, NextWake, WakeReason, WakeSchedules,
};
use crate::interfaces::AttachedInterfaces;
use crate::storage::StorageLayout;

#[cfg_attr(not(feature = "embassy-host"), allow(dead_code))]
pub async fn wait_for_due_reason<H: Host>(host: &H, scheduled_wake: NextWake) -> WakeReason {
    match scheduled_wake {
        NextWake::Idle => core::future::pending().await,
        NextWake::Due(reason) => reason,
        NextWake::At { at, reason } => {
            host.sleep_until(at).await;
            reason
        }
    }
}

pub async fn wait_for_pacer<H: Host>(host: &H, deadline: Option<InstantMillis>) {
    match deadline {
        Some(at) => host.sleep_until(at).await,
        None => core::future::pending().await,
    }
}

pub fn fire_due_reason<S, F>(
    engine: &mut EngineState<S>,
    reason: WakeReason,
    now: InstantMillis,
    interfaces: AttachedInterfaces<'_>,
    fill_entropy: &mut F,
    on_reaction: &mut impl FnMut(EngineReaction<'_>),
) -> WakeSchedules
where
    S: StorageLayout,
    F: FnMut(&mut [u8]),
{
    match reason {
        WakeReason::ScheduledAnnounces => {
            engine.fire_due_scheduled_announces(now, interfaces, on_reaction)
        }
        WakeReason::ReceiptTimeouts => engine.settle_timed_out_receipts(now, on_reaction),
        WakeReason::PathRequestTimeouts => engine.settle_timed_out_path_requests(now, on_reaction),
        WakeReason::ExpiredRoutes => engine.cull_expired_routes(now, interfaces, on_reaction),
        WakeReason::ExpiredKnownDestinations => engine.cull_expired_known_destinations(now),
        WakeReason::ExpiredBlackholes => engine.cull_expired_blackholes(now),
        WakeReason::LinkDeadlines => {
            engine.fire_due_link_deadlines(now, interfaces, fill_entropy, on_reaction)
        }
        WakeReason::ResourceDeadlines => {
            engine.fire_due_resource_deadlines(now, fill_entropy, on_reaction)
        }
        WakeReason::ChannelTimeouts => {
            engine.fire_due_channel_timeouts(now, interfaces, fill_entropy, on_reaction)
        }
        WakeReason::HeldAnnounceRelease => {
            engine.fire_due_held_announces(now, interfaces, fill_entropy, on_reaction)
        }
    }
}

pub fn merge_wake_schedules_delta<S: StorageLayout>(
    source_wake_schedules: &mut WakeSchedules,
    delta: WakeSchedules,
    engine: &EngineState<S>,
    interfaces: AttachedInterfaces<'_>,
) {
    source_wake_schedules.merge(delta);
    #[cfg(debug_assertions)]
    {
        let truth = engine.wake_schedules(interfaces);
        debug_assert_eq!(
            source_wake_schedules.scheduled_announces, truth.scheduled_announces,
            "the scheduled-announces schedule drifted from a full recompute",
        );
        debug_assert_eq!(
            source_wake_schedules.receipt_timeouts, truth.receipt_timeouts,
            "the receipt-timeouts schedule drifted from a full recompute",
        );
        debug_assert_eq!(
            source_wake_schedules.path_request_timeouts, truth.path_request_timeouts,
            "the path-request-timeouts schedule drifted from a full recompute",
        );
        debug_assert!(
            never_late(source_wake_schedules.link_deadlines, truth.link_deadlines),
            "the link-deadlines schedule must never sit later than the truth: cached {:?}, truth {:?}",
            source_wake_schedules.link_deadlines,
            truth.link_deadlines,
        );
        debug_assert_eq!(
            source_wake_schedules.resource_deadlines, truth.resource_deadlines,
            "the resource-deadlines schedule drifted from a full recompute",
        );
        debug_assert_eq!(
            source_wake_schedules.channel_timeouts, truth.channel_timeouts,
            "the channel-timeouts schedule drifted from a full recompute",
        );
        debug_assert!(
            never_late(source_wake_schedules.expired_routes, truth.expired_routes),
            "the expired-routes schedule must never sit later than the truth: cached {:?}, truth {:?}",
            source_wake_schedules.expired_routes,
            truth.expired_routes,
        );
        debug_assert!(
            never_late(
                source_wake_schedules.expired_known_destinations,
                truth.expired_known_destinations,
            ),
            "the expired-known-destinations schedule must never sit later than the truth: cached {:?}, truth {:?}",
            source_wake_schedules.expired_known_destinations,
            truth.expired_known_destinations,
        );
        debug_assert_eq!(
            source_wake_schedules.expired_blackholes, truth.expired_blackholes,
            "the expired-blackholes schedule drifted from a full recompute",
        );
        debug_assert_eq!(
            source_wake_schedules.held_announce_release, truth.held_announce_release,
            "the held-announce-release schedule drifted from a full recompute",
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = (engine, interfaces);
}

/// The expired-routes schedule runs on `AtMost` deltas, so its cached deadline may sit EARLY of the truth (a removal or refresh pushed the true deadline later) but never late: a premature wake costs one no-op cull whose full recompute resyncs the schedule; a late one would miss a deadline.
#[cfg(debug_assertions)]
fn never_late(cached: crate::engine::WakeSchedule, truth: crate::engine::WakeSchedule) -> bool {
    use crate::engine::WakeSchedule::{At, Idle};
    match (cached, truth) {
        (At(cached_at), At(truth_at)) => cached_at <= truth_at,
        (At(_), Idle) => true,
        (Idle, Idle) => true,
        _ => cached == truth,
    }
}
