use std::time::{Duration, Instant};

use opentelemetry::metrics::Gauge;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use personal_rns::node_introspection::logical_interface_inventory;
use personal_rns::runtime::{PrnsNodeHandle, RuntimeHealth, RuntimeMetricsSnapshot};

use instruments::Instruments;

mod dimensions;
mod instruments;
mod snapshot;

const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) struct MetricsReporter {
    instruments: Instruments,
    previous: Option<RuntimeMetricsSnapshot>,
}

impl MetricsReporter {
    pub(super) fn new(provider: &SdkMeterProvider) -> Self {
        Self {
            instruments: Instruments::new(provider),
            previous: None,
        }
    }

    pub(crate) async fn run(mut self, handle: PrnsNodeHandle, started: Instant) {
        let mut interval = tokio::time::interval(SNAPSHOT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(snapshot) = handle.metrics_snapshot().await else {
                return;
            };
            let interfaces = logical_interface_inventory(handle.interface_inventory());
            let logical_snapshots = interfaces
                .iter()
                .map(|interface| interface.snapshot)
                .collect::<Vec<_>>();
            let mut health = RuntimeHealth::from_snapshots(started.elapsed(), &logical_snapshots);
            health.route_count = snapshot.engine.route_count;
            health.link_count = snapshot.engine.link_count;
            health.transported_link_count = snapshot.engine.transported_link_count;
            self.record(health, &interfaces, snapshot);
        }
    }

    pub(crate) fn runtime_up_handle(&self) -> Gauge<u64> {
        self.instruments.runtime_up.clone()
    }
}
