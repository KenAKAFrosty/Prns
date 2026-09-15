use std::collections::HashMap;
use std::time::{Duration, Instant};

use opentelemetry::metrics::Gauge;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use personal_rns::interfaces::InterfaceKind;
use personal_rns::interfaces::{FrameAccounting, InterfaceId};
use personal_rns::node_introspection::logical_interface_inventory;
use personal_rns::runtime::{PrnsNodeHandle, RuntimeHealth, RuntimeMetricsSnapshot};

use instruments::Instruments;

mod dimensions;
mod instruments;
mod snapshot;

const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);
const EGRESS_PENDING_STALL_WARNING: Duration = Duration::from_secs(30);

#[derive(Default)]
struct EgressPendingStall {
    since: Option<Instant>,
    flushed_pending_frames: u64,
    warned: bool,
}

impl EgressPendingStall {
    fn observe(&mut self, pending_frames: u32, flushed_pending_frames: u64, now: Instant) -> u64 {
        if pending_frames == 0 {
            self.since = None;
            self.flushed_pending_frames = flushed_pending_frames;
            self.warned = false;
            return 0;
        }

        if self.since.is_none() || self.flushed_pending_frames != flushed_pending_frames {
            self.since = Some(now);
            self.flushed_pending_frames = flushed_pending_frames;
            self.warned = false;
        }

        let stalled_for = self
            .since
            .map_or(Duration::ZERO, |since| now.duration_since(since));
        if stalled_for >= EGRESS_PENDING_STALL_WARNING && !self.warned {
            tracing::warn!(
                target: "prnsd::observability",
                event = "egress_pending_stalled",
                pending_frames,
                stalled_seconds = stalled_for.as_secs(),
                "egress pending queue has made no flush progress"
            );
            self.warned = true;
        }
        stalled_for.as_secs()
    }
}

pub(super) struct MetricsReporter {
    instruments: Instruments,
    previous: Option<RuntimeMetricsSnapshot>,
    previous_frame_accounting: HashMap<InterfaceId, (u64, FrameAccounting)>,
    egress_pending_stall: EgressPendingStall,
}

pub(crate) struct RunningMetricsReporter {
    task: tokio::task::JoinHandle<()>,
    runtime_up: Gauge<u64>,
}

impl RunningMetricsReporter {
    pub(crate) async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
        self.runtime_up.record(0, &[]);
    }
}

impl MetricsReporter {
    pub(super) fn new(provider: &SdkMeterProvider) -> Self {
        Self {
            instruments: Instruments::new(provider),
            previous: None,
            previous_frame_accounting: HashMap::new(),
            egress_pending_stall: EgressPendingStall::default(),
        }
    }

    async fn run(mut self, handle: PrnsNodeHandle, started: Instant) {
        let mut interval = tokio::time::interval(SNAPSHOT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(snapshot) = handle.metrics_snapshot().await else {
                return;
            };
            let raw_inventory = handle.interface_inventory();
            let local_client_count = raw_inventory
                .iter()
                .filter(|entry| entry.snapshot.id.kind() == Some(InterfaceKind::LocalClient))
                .count() as u32;
            let interfaces = logical_interface_inventory(raw_inventory);
            let logical_snapshots = interfaces
                .iter()
                .map(|interface| interface.snapshot)
                .collect::<Vec<_>>();
            let mut health = RuntimeHealth::from_snapshots(started.elapsed(), &logical_snapshots);
            health.local_client_count = local_client_count;
            health.route_count = snapshot.engine.route_count;
            health.link_count = snapshot.engine.link_count;
            health.transported_link_count = snapshot.engine.transported_link_count;
            self.record(health, &interfaces, snapshot);
        }
    }

    fn runtime_up_handle(&self) -> Gauge<u64> {
        self.instruments.runtime_up.clone()
    }

    pub(super) fn spawn(self, handle: PrnsNodeHandle, started: Instant) -> RunningMetricsReporter {
        let runtime_up = self.runtime_up_handle();
        RunningMetricsReporter {
            task: tokio::spawn(self.run(handle, started)),
            runtime_up,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_stall_age_resets_on_flush_progress_and_empty_queue() {
        let started = Instant::now();
        let mut stall = EgressPendingStall::default();

        assert_eq!(stall.observe(2, 0, started), 0);
        assert_eq!(stall.observe(2, 0, started + Duration::from_secs(31)), 31);
        assert!(stall.warned);

        assert_eq!(stall.observe(1, 1, started + Duration::from_secs(32)), 0);
        assert!(!stall.warned);
        assert_eq!(stall.observe(0, 1, started + Duration::from_secs(40)), 0);
        assert!(stall.since.is_none());
    }
}
