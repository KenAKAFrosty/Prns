//! The host-agnostic core of the reactor's timer edge: pick the scheduled lane that came
//! due and fire it. Both the tokio and embassy drivers share this — the only thing that
//! differs between hosts is the channel and select primitives, never which engine method a
//! [`DueLane`] names. Keeping the dispatch here is what makes "honor the shape" literal:
//! one table, two hosts.

use super::Host;
use crate::engine::{DueLane, EngineReaction, EngineState, InstantMillis, ScheduledWake};
use crate::interfaces::InterfaceDescriptor;
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::storage::EngineStorage;

/// Park until the named scheduled lane comes due, then name it. `Due` fires at once; `At`
/// sleeps until its deadline — the engine is frozen while the reactor parks, so the lane
/// that owned the deadline is still the one now due. `Idle` parks forever, leaving the
/// select to rest on its channels until one wakes the loop.
pub(crate) async fn wait_for_due_lane<H: Host>(host: &H, wake: ScheduledWake) -> DueLane {
    match wake {
        ScheduledWake::Idle => core::future::pending().await,
        ScheduledWake::Due(lane) => lane,
        ScheduledWake::At { at, lane } => {
            host.sleep_until(at).await;
            lane
        }
    }
}

/// Fire exactly the one scheduled lane that came due, streaming whatever it owes to
/// `on_reaction`. One arm per [`DueLane`], each naming a single engine method — the wake
/// does the work it woke for and nothing else.
pub(crate) fn fire_due_lane<S, H>(
    engine: &mut EngineState<S>,
    lane: DueLane,
    now: InstantMillis,
    jitter: JitterSeed,
    view: &[InterfaceDescriptor],
    host: &mut H,
    on_reaction: &mut impl FnMut(EngineReaction<'_>),
) where
    S: EngineStorage,
    H: Host,
{
    match lane {
        DueLane::HeldAnnounces => {
            engine.recover_held_announces(jitter, view, on_reaction);
        }
        DueLane::SelfAnnounce => {
            engine.fire_due_self_announces(
                now,
                view,
                &mut |entropy| host.fill_entropy(entropy),
                on_reaction,
            );
        }
        DueLane::Rebroadcast => {
            engine.fire_due_announce_rebroadcasts(now, view, on_reaction);
        }
        DueLane::SendSingleTimeout => {
            engine.settle_timed_out_send_singles(now, on_reaction);
        }
        DueLane::PathRequestTimeout => {
            engine.settle_timed_out_path_requests(now, on_reaction);
        }
    }
}
