//! Process-local correlation for opt-in trials. Tags cannot be compared across
//! process restarts, and neither source identifiers nor the random salt are logged.
use super::{enabled, TARGET};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    OnceLock,
};
use std::time::Instant;

const ATTEMPT_LIMIT: u32 = 128;
const DELIVERY_TARGET: &str = "prns.groups.delivery";
const PHASE_LIMIT: u8 = 8;
static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
static COMMITS: AtomicU32 = AtomicU32::new(0);
static SALT: OnceLock<Option<[u8; 32]>> = OnceLock::new();

fn salt() -> Option<&'static [u8; 32]> {
    SALT.get_or_init(|| {
        let mut value = [0; 32];
        getrandom::getrandom(&mut value).ok().map(|()| value)
    })
    .as_ref()
}

fn tag(salt: &[u8; 32], domain: &[u8], value: &[u8]) -> u64 {
    let mut digest = Sha256::new();
    digest.update(salt);
    digest.update(domain);
    digest.update(value);
    let bytes = digest.finalize();
    let mut short = [0; 8];
    short.copy_from_slice(&bytes[..8]);
    u64::from_be_bytes(short)
}

pub(super) fn correlation_tag(domain: &[u8], value: &[u8]) -> Option<u64> {
    salt().map(|salt| tag(salt, domain, value))
}

fn admit(counter: &AtomicU32, target: &'static str, family: &'static str) -> Option<u32> {
    let previous = {
        let mut previous = counter.load(Ordering::Relaxed);
        loop {
            let Some(next) = (previous <= ATTEMPT_LIMIT).then(|| previous + 1) else {
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
    }
    .ok()?;
    if previous == ATTEMPT_LIMIT {
        log::info!(target: target, "stage=diagnostics category=correlation_limit family={family} limit={ATTEMPT_LIMIT} dropped_at_least=1");
        None
    } else {
        Some(previous + 1)
    }
}

/// A caller observed a durable result, not an exhaustive commit journal: a
/// dropped waiter omits this event and an idempotent retry can repeat it.
pub(crate) fn local_send_result(group: &[u8], record: &[u8]) {
    if !enabled() || !log::log_enabled!(target: DELIVERY_TARGET, log::Level::Info) {
        return;
    }
    let Some(salt) = salt() else { return };
    if admit(&COMMITS, DELIVERY_TARGET, "local_send_result").is_some() {
        log::info!(target: DELIVERY_TARGET, "stage=local_send_result category=durable_observed group={:016x} record={:016x}",
            tag(salt, b"group", group), tag(salt, b"record", record));
    }
}

pub(crate) struct AttemptTrace(Option<Active>);
struct Active {
    id: u32,
    group: u64,
    peer: u64,
    direction: &'static str,
    started: Instant,
    phases: u8,
    finished: bool,
}

impl AttemptTrace {
    pub(crate) fn start(direction: &'static str, group: &[u8], peer: &[u8]) -> Self {
        let active = (|| {
            if !enabled() || !log::log_enabled!(target: TARGET, log::Level::Info) {
                return None;
            }
            let salt = salt()?;
            Some(Active {
                id: admit(&ATTEMPTS, TARGET, "sync_attempt")?,
                group: tag(salt, b"group", group),
                peer: tag(salt, b"peer", peer),
                direction,
                started: Instant::now(),
                phases: 0,
                finished: false,
            })
        })();
        let mut trace = Self(active);
        trace.phase("start", "started");
        trace
    }

    pub(crate) fn phase(&mut self, phase: &'static str, category: &'static str) {
        if let Some(active) = self.0.as_mut() {
            if active.phases < PHASE_LIMIT {
                active.phases += 1;
                active.write(phase, category);
            }
        }
    }

    pub(crate) fn finish(&mut self, category: &'static str) {
        if let Some(active) = self.0.as_mut() {
            if !active.finished {
                active.finished = true;
                active.write("finish", category);
            }
        }
    }
}

impl Active {
    fn write(&self, phase: &'static str, category: &'static str) {
        log::info!(target: TARGET,
            "stage=sync_attempt direction={} attempt={} group={:016x} peer={:016x} phase={} category={} elapsed_ms={}",
            self.direction, self.id, self.group, self.peer, phase, category, self.started.elapsed().as_millis());
    }
}

impl Drop for AttemptTrace {
    fn drop(&mut self) {
        self.finish("cancelled");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_domain_and_process_separated_but_stable_within_a_process() {
        let value = [3; 32];
        assert_eq!(
            tag(&[1; 32], b"group", &value),
            tag(&[1; 32], b"group", &value)
        );
        assert_ne!(
            tag(&[1; 32], b"group", &value),
            tag(&[2; 32], b"group", &value)
        );
        assert_ne!(
            tag(&[1; 32], b"group", &value),
            tag(&[1; 32], b"peer", &value)
        );
    }

    #[test]
    fn correlation_admission_is_bounded_and_cannot_wrap() {
        let counter = AtomicU32::new(0);
        for id in 1..=ATTEMPT_LIMIT {
            assert_eq!(admit(&counter, TARGET, "sync_attempt"), Some(id));
        }
        assert_eq!(admit(&counter, TARGET, "sync_attempt"), None);
        assert_eq!(admit(&counter, TARGET, "sync_attempt"), None);
        assert_eq!(counter.load(Ordering::Relaxed), ATTEMPT_LIMIT + 1);
        counter.store(u32::MAX, Ordering::Relaxed);
        assert_eq!(admit(&counter, TARGET, "sync_attempt"), None);
    }
}
