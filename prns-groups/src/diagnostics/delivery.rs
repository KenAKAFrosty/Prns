//! Non-empty delivery and durable receipt events have their own budgets. Idle
//! discovery/inventory traffic cannot exhaust these before a physical trial.
use super::{attempt::correlation_tag, enabled};
const TARGET: &str = "prns.groups.delivery";
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

const LIMIT: u32 = 256;
static PUSHES: AtomicU32 = AtomicU32::new(0);
static RECEIPTS: AtomicU32 = AtomicU32::new(0);

fn admit(counter: &AtomicU32, family: &'static str) -> Option<u32> {
    let prior = {
        let mut previous = counter.load(Ordering::Relaxed);
        loop {
            let Some(next) = (previous <= LIMIT).then(|| previous + 1) else {
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
    if prior == LIMIT {
        log::info!(target: TARGET, "stage=delivery_limit family={family} limit={LIMIT}");
        None
    } else {
        Some(prior + 1)
    }
}

pub(crate) struct DeliveryTrace(Option<Active>);
struct Active {
    id: u32,
    started: Instant,
    group: u64,
    record: u64,
    peer: u64,
}
impl DeliveryTrace {
    pub(crate) fn start(group: &[u8], record: &[u8], peer: &[u8], bytes: usize) -> Self {
        if !enabled() || !log::log_enabled!(target: TARGET, log::Level::Info) {
            return Self(None);
        }
        let active = (|| {
            let group = correlation_tag(b"group", group)?;
            let record = correlation_tag(b"record", record)?;
            let peer = correlation_tag(b"peer", peer)?;
            let id = admit(&PUSHES, "push")?;
            log::info!(target: TARGET, "stage=push attempt={id} group={group:016x} record={record:016x} peer={peer:016x} phase=start frame_bytes={bytes}");
            Some(Active {
                id,
                started: Instant::now(),
                group,
                record,
                peer,
            })
        })();
        Self(active)
    }
    pub(crate) fn finish(&mut self, category: &'static str, receipts: usize) {
        if let Some(active) = self.0.take() {
            log::info!(target: TARGET, "stage=push attempt={} group={:016x} record={:016x} peer={:016x} phase=finish category={category} accepted_receipts={receipts} elapsed_ms={}", active.id, active.group, active.record, active.peer, active.started.elapsed().as_millis());
        }
    }
}
impl Drop for DeliveryTrace {
    fn drop(&mut self) {
        self.finish("cancelled", 0);
    }
}

pub(crate) fn receipt_transition(group: &[u8], record: &[u8], category: &'static str) {
    if !enabled() || !log::log_enabled!(target: TARGET, log::Level::Info) {
        return;
    }
    let (Some(group), Some(record)) = (
        correlation_tag(b"group", group),
        correlation_tag(b"record", record),
    ) else {
        return;
    };
    if let Some(sequence) = admit(&RECEIPTS, "receipt") {
        log::info!(target: TARGET, "stage=receipt_transition sequence={sequence} group={group:016x} record={record:016x} category={category}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delivery_and_receipt_budgets_are_independent_and_saturate() {
        let delivery = AtomicU32::new(0);
        let receipt = AtomicU32::new(0);
        for i in 1..=LIMIT {
            assert_eq!(admit(&delivery, "test"), Some(i));
        }
        assert_eq!(admit(&delivery, "test"), None);
        assert_eq!(admit(&delivery, "test"), None);
        assert_eq!(admit(&receipt, "test"), Some(1));
        assert_eq!(delivery.load(Ordering::Relaxed), LIMIT + 1);
        delivery.store(u32::MAX, Ordering::Relaxed);
        assert_eq!(admit(&delivery, "test"), None);
    }
}
