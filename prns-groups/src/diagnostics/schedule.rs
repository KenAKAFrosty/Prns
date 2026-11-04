use super::{attempt::correlation_tag, enabled};
use prns_groups_core::sync::{PeerSchedule, SyncActivity};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Mutex,
};
use tokio::time::Instant;

const TARGET: &str = "prns.groups.schedule";
const LIMIT: u32 = 1024;
static COUNT: AtomicU32 = AtomicU32::new(0);
static UNAVAILABLE: AtomicBool = AtomicBool::new(false);
static EMISSION: Mutex<()> = Mutex::new(());
static WATERMARK: AtomicU32 = AtomicU32::new(0);
static HEALTHY: AtomicBool = AtomicBool::new(true);

/// Nonlogging process-lifetime schedule coverage watermark. False health means
/// producer exhaustion, entropy failure or poisoned emission ordering.
pub fn metadata_schedule_snapshot() -> (u32, bool) {
    (
        WATERMARK.load(Ordering::Acquire),
        HEALTHY.load(Ordering::Acquire),
    )
}

pub(crate) struct Decision {
    pub category: &'static str,
    pub activity: Option<SyncActivity>,
    pub pending: Option<bool>,
    pub fresh: bool,
    pub repair: bool,
    pub accelerated: bool,
}

pub(crate) fn schedule(
    group: &[u8],
    peer: &[u8],
    state: &PeerSchedule,
    due: Instant,
    floor: Instant,
    decision: Decision,
) {
    emit(group, peer, |sequence, group, peer| {
        let now = Instant::now();
        let due_in_ms = crate::schedule::millis(due.saturating_duration_since(now));
        let floor_in_ms = crate::schedule::millis(floor.saturating_duration_since(now));
        let activity = match decision.activity {
            Some(SyncActivity::Progress) => "progress",
            Some(SyncActivity::Idle) => "idle",
            Some(SyncActivity::Failed) => "failed",
            None => "none",
        };
        let pending = match decision.pending {
            Some(true) => "true",
            Some(false) => "false",
            None => "unknown",
        };
        log::info!(target: TARGET,
            "stage=schedule category={} sequence={sequence} group={group:016x} peer={peer:016x} activity={activity} due_in_ms={due_in_ms} floor_in_ms={floor_in_ms} failures={} pending={pending} fresh={} repair={} accelerated={}",
            decision.category, state.failures(), decision.fresh, decision.repair, decision.accelerated);
    });
}

/// Raw hint disposition is independent of whether a deadline was shortened.
/// Share the ordered schedule stream and its cap/watermark, without inventing
/// due times or failure counts for a hint that has no scheduler entry yet.
pub(crate) fn recovery(
    group: &[u8],
    peer: &[u8],
    disposition: crate::schedule::RecoveryDisposition,
) {
    emit(group, peer, |sequence, group, peer| {
        log::info!(target: TARGET,
            "stage=recovery category={} sequence={sequence} group={group:016x} peer={peer:016x}",
            disposition.name());
    });
}

fn emit(group: &[u8], peer: &[u8], output: impl FnOnce(u32, u64, u64)) {
    if !enabled() || !log::log_enabled!(target: TARGET, log::Level::Info) {
        return;
    }
    let Ok(_emission) = EMISSION.lock() else {
        HEALTHY.store(false, Ordering::Release);
        return;
    };
    let Some((group, peer)) = correlation_tag(b"group", group).zip(correlation_tag(b"peer", peer))
    else {
        HEALTHY.store(false, Ordering::Release);
        if !UNAVAILABLE.swap(true, Ordering::Relaxed) {
            log::info!(target: TARGET, "stage=diagnostics category=schedule_unavailable family=schedule");
        }
        return;
    };
    let Ok(previous) = ({
        let mut previous = COUNT.load(Ordering::Relaxed);
        loop {
            let Some(next) = (previous <= LIMIT).then(|| previous + 1) else {
                break Err(previous);
            };
            match COUNT.compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(previous) => break Ok(previous),
                Err(current) => previous = current,
            }
        }
    }) else {
        return;
    };
    if previous == LIMIT {
        HEALTHY.store(false, Ordering::Release);
        log::info!(target: TARGET, "stage=diagnostics category=schedule_limit family=schedule limit={LIMIT} dropped_at_least=1");
        return;
    }
    let sequence = previous + 1;
    output(sequence, group, peer);
    WATERMARK.store(sequence, Ordering::Release);
}
