//! The host-agnostic core of the reactor's timer edge: pick the scheduled lane that came
//! due and fire it. Both the tokio and embassy drivers share this — the only thing that
//! differs between hosts is the channel and select primitives, never which engine method a
//! [`DueLane`] names. Keeping the dispatch here is what makes "honor the shape" literal:
//! one table, two hosts.

use super::Host;
use crate::engine::{
    DueLane, EngineReaction, EngineState, InstantMillis, ScheduledWake, WakeOutlook,
};
use crate::interfaces::InterfaceDescriptor;
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::storage::EngineStorage;

/// Park until the named scheduled lane comes due, then name it. `Due` fires at once; `At`
/// sleeps until its deadline — the engine is frozen while the reactor parks, so the lane
/// that owned the deadline is still the one now due. `Idle` parks forever, leaving the
/// select to rest on its channels until one wakes the loop.
pub async fn wait_for_due_lane<H: Host>(host: &H, wake: ScheduledWake) -> DueLane {
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
/// `on_reaction` and returning that lane's [`WakeOutlook`] delta. One arm per [`DueLane`],
/// each naming a single engine method — the wake does the work it woke for and nothing
/// else. Jitter is drawn lazily here, only on the held-recovery lane that actually needs it.
pub fn fire_due_lane<S, H>(
    engine: &mut EngineState<S>,
    lane: DueLane,
    now: InstantMillis,
    view: &[InterfaceDescriptor],
    host: &mut H,
    on_reaction: &mut impl FnMut(EngineReaction<'_>),
) -> WakeOutlook
where
    S: EngineStorage,
    H: Host,
{
    match lane {
        DueLane::HeldAnnounces => {
            engine.recover_held_announces(draw_jitter(host), view, on_reaction)
        }
        DueLane::Rebroadcast => engine.fire_due_announce_rebroadcasts(now, view, on_reaction),
        DueLane::SendSingleTimeout => engine.settle_timed_out_send_singles(now, on_reaction),
        DueLane::PathRequestTimeout => engine.settle_timed_out_path_requests(now, on_reaction),
    }
}

/// Draw eight fresh entropy bytes and seed a rebroadcast jitter from them. Pulled lazily,
/// only on the wakes that actually schedule a rebroadcast — an inbound packet or a held
/// recovery — so an idle, command, or timeout wake draws no entropy at all.
pub fn draw_jitter<H: Host>(host: &mut H) -> JitterSeed {
    let mut bytes = [0u8; core::mem::size_of::<u64>()];
    host.fill_entropy(&mut bytes);
    JitterSeed(u64::from_le_bytes(bytes))
}

/// Fold an engine method's [`WakeOutlook`] delta into the live outlook. In debug builds it
/// then re-derives the full outlook and asserts the two agree — the incremental bookkeeping
/// is only as correct as every method's footprint, so the full recompute stands guard as
/// the oracle. Release builds trust the delta and skip the probe.
pub fn advance<S: EngineStorage>(
    outlook: &mut WakeOutlook,
    delta: WakeOutlook,
    engine: &EngineState<S>,
) {
    outlook.merge(delta);
    debug_assert_eq!(
        *outlook,
        engine.wake_outlook(),
        "the incremental wake outlook drifted from a full recompute",
    );
}
