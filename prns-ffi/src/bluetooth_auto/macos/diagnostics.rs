//! Bounded metadata for an embedding's opt-in Bluetooth logger. Never pass peer data here.
use std::fmt;
#[cfg(any(feature = "log", test))]
use std::sync::atomic::{AtomicU32, Ordering};

use super::discovery::{DiscoverDisposition, PeripheralLinkState};

#[derive(Clone, Copy)]
pub(super) enum Family {
    Manager,
    Scan,
    Restoration,
    Discovery,
    Dial,
    Gatt,
    Recovery,
}

impl Family {
    #[cfg(feature = "log")]
    fn name(self) -> &'static str {
        match self {
            Self::Manager => "manager",
            Self::Scan => "scan",
            Self::Restoration => "restoration",
            Self::Discovery => "discovery",
            Self::Dial => "dial",
            Self::Gatt => "gatt",
            Self::Recovery => "recovery",
        }
    }
}

#[cfg(any(feature = "log", test))]
const FAMILY_LIMIT: u32 = 64;
#[cfg(feature = "log")]
const TARGET: &str = "prns.bluetooth.diagnostics";
#[cfg(feature = "log")]
static COUNTERS: [AtomicU32; 7] = [const { AtomicU32::new(0) }; 7];

#[cfg(any(feature = "log", test))]
#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Record,
    Limit,
    Suppressed,
}

#[cfg(any(feature = "log", test))]
fn admit(counter: &AtomicU32, enabled: bool) -> Admission {
    if !enabled {
        return Admission::Suppressed;
    }
    match {
        let mut previous = counter.load(Ordering::Relaxed);
        loop {
            let Some(next) = (previous <= FAMILY_LIMIT).then(|| previous + 1) else {
                break Err(previous);
            };
            match counter.compare_exchange_weak(
                previous,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(previous) => break Ok(previous),
                Err(current) => previous = current,
            }
        }
    } {
        Ok(count) if count < FAMILY_LIMIT => Admission::Record,
        Ok(_) => Admission::Limit,
        Err(_) => Admission::Suppressed,
    }
}

pub(super) fn emit(family: Family, message: fmt::Arguments<'_>) {
    #[cfg(feature = "log")]
    match admit(
        &COUNTERS[family as usize],
        log::log_enabled!(target: TARGET, log::Level::Info),
    ) {
        Admission::Record => log::info!(target: TARGET, "{message}"),
        Admission::Limit => log::info!(target: TARGET,
            "stage=diagnostics category=family_limit family={} limit={FAMILY_LIMIT} dropped_at_least=1",
            family.name()),
        Admission::Suppressed => {}
    }
    #[cfg(not(feature = "log"))]
    let _ = (family, message);
}

pub(super) fn state_name(state: PeripheralLinkState) -> &'static str {
    match state {
        PeripheralLinkState::Disconnected => "disconnected",
        PeripheralLinkState::Connecting => "connecting",
        PeripheralLinkState::Connected => "connected",
        PeripheralLinkState::Disconnecting => "disconnecting",
        PeripheralLinkState::Unknown => "unknown",
    }
}

pub(super) fn disposition_name(disposition: DiscoverDisposition) -> &'static str {
    match disposition {
        DiscoverDisposition::Adopt => "adopt",
        DiscoverDisposition::IgnoreOwned => "ignore_owned",
        DiscoverDisposition::WaitForDisconnect => "wait_for_disconnect",
        DiscoverDisposition::CancelStale => "cancel_stale",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_diagnostics_do_not_consume_the_bounded_budget() {
        let count = AtomicU32::new(0);
        for _ in 0..100 {
            assert_eq!(admit(&count, false), Admission::Suppressed);
        }
        assert_eq!(count.load(Ordering::Relaxed), 0);
        for _ in 0..FAMILY_LIMIT {
            assert_eq!(admit(&count, true), Admission::Record);
        }
        assert_eq!(admit(&count, true), Admission::Limit);
        assert_eq!(admit(&count, true), Admission::Suppressed);
        count.store(u32::MAX, Ordering::Relaxed);
        assert_eq!(admit(&count, true), Admission::Suppressed);
    }

    #[test]
    fn diagnostic_admission_distinguishes_ios_local_acquisition_from_macos_cancellation() {
        use super::super::discovery::{
            discover_disposition, SessionPresence, StaleCancellation, StaleLinkRecovery,
        };
        let state = PeripheralLinkState::Connected;
        assert_eq!(state_name(state), "connected");
        for (recovery, expected) in [
            (StaleLinkRecovery::AcquireLocal, "adopt"),
            (StaleLinkRecovery::Enabled, "cancel_stale"),
        ] {
            assert_eq!(
                disposition_name(discover_disposition(
                    state,
                    SessionPresence::Absent,
                    StaleCancellation::Idle,
                    recovery
                )),
                expected
            );
        }
        assert_eq!(
            disposition_name(discover_disposition(
                state,
                SessionPresence::Present,
                StaleCancellation::Idle,
                StaleLinkRecovery::Enabled
            )),
            "ignore_owned"
        );
    }
}
