use crate::engine::{EngineReaction, EngineState, InstantMillis, WakeReason, WakeSchedules};
use crate::interfaces::AttachedInterfaces;
use crate::storage::StorageLayout;

pub fn fire_due_reason<S, F>(
    engine: &mut EngineState<S>,
    reason: WakeReason,
    now: InstantMillis,
    interfaces: AttachedInterfaces<'_>,
    fill_random: &mut F,
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
        WakeReason::ExpiredDestinationIdentities => engine.cull_expired_destination_identities(now),
        WakeReason::ExpiredBlackholes => engine.cull_expired_blackholes(now),
        WakeReason::LinkDeadlines => {
            engine.fire_due_link_deadlines(now, interfaces, fill_random, on_reaction)
        }
        WakeReason::ResourceDeadlines => {
            engine.fire_due_resource_deadlines(now, fill_random, on_reaction)
        }
        WakeReason::ChannelTimeouts => {
            engine.fire_due_channel_timeouts(now, interfaces, fill_random, on_reaction)
        }
        WakeReason::HeldAnnounceRelease => {
            engine.fire_due_held_announces(now, interfaces, fill_random, on_reaction)
        }
        WakeReason::RemoteControlPairing => {
            engine.fire_due_remote_control_pairing(now, interfaces, fill_random, on_reaction)
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
                source_wake_schedules.expired_destination_identities,
                truth.expired_destination_identities,
            ),
            "the expired-destination-identities schedule must never sit later than the truth: cached {:?}, truth {:?}",
            source_wake_schedules.expired_destination_identities,
            truth.expired_destination_identities,
        );
        debug_assert_eq!(
            source_wake_schedules.expired_blackholes, truth.expired_blackholes,
            "the expired-blackholes schedule drifted from a full recompute",
        );
        debug_assert_eq!(
            source_wake_schedules.held_announce_release, truth.held_announce_release,
            "the held-announce-release schedule drifted from a full recompute",
        );
        debug_assert_eq!(
            source_wake_schedules.remote_control_pairing, truth.remote_control_pairing,
            "the RemoteControl pairing schedule drifted from a full recompute",
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = (engine, interfaces);
}

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
