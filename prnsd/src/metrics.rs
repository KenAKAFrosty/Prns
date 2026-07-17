use std::time::{Duration, Instant};

use opentelemetry::metrics::{Counter, Gauge, MeterProvider as _};
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use personal_rns::engine::{
    AnnounceCommandOutcome, AnnounceIngressOutcome, AnnounceOrigin, AnnounceSourceKind,
    IgnoreReasonKind,
};
use personal_rns::node_introspection::{logical_interface_inventory, InterfaceInventoryEntry};
use personal_rns::interfaces::InterfaceKind;
use personal_rns::runtime::{
    AnnounceEgressOutcome, RuntimeHealth, RuntimeLinkClosure, RuntimeMetricsSnapshot,
    RuntimeOperation, RuntimeOperationOutcome, RuntimeResourceFailure, RuntimeRouteRemoval,
    TokioPrnsHandle,
};

const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);

pub struct MetricsReporter {
    instruments: Instruments,
    previous: Option<RuntimeMetricsSnapshot>,
}

struct Instruments {
    runtime_up: Gauge<u64>,
    uptime_seconds: Gauge<u64>,
    interfaces: Gauge<u64>,
    routes: Gauge<u64>,
    links: Gauge<u64>,
    shared_clients: Gauge<u64>,
    io_bits_per_second: Gauge<u64>,
    io_bytes: Gauge<u64>,
    interface_state: Gauge<u64>,
    interface_routes: Gauge<u64>,
    interface_links: Gauge<u64>,
    interface_io_bits_per_second: Gauge<u64>,
    interface_io_bytes: Gauge<u64>,
    interface_announce_ingress: Counter<u64>,
    interface_announce_egress: Counter<u64>,
    interface_announce_egress_bytes: Counter<u64>,
    interface_announce_queue_depth: Gauge<u64>,
    engine_packets: Counter<u64>,
    engine_commands: Counter<u64>,
    ignored_packets: Counter<u64>,
    egress_frames: Counter<u64>,
    announce_ingress: Counter<u64>,
    announce_accepted_by_interface: Counter<u64>,
    announce_commands: Counter<u64>,
    announce_egress: Counter<u64>,
    announce_egress_by_interface: Counter<u64>,
    announce_egress_bytes: Counter<u64>,
    announce_held: Gauge<u64>,
    announce_scheduled: Gauge<u64>,
    announce_pacer_queue_depth: Gauge<u64>,
    crypto_jobs: Counter<u64>,
    crypto_queue_depth: Gauge<u64>,
    crypto_maximum_queue_depth: Gauge<u64>,
    crypto_backpressure_deferrals: Counter<u64>,
    crypto_packet_verdicts_owed: Gauge<u64>,
    operations: Counter<u64>,
    resource_failures: Counter<u64>,
    link_closures: Counter<u64>,
    link_interface_mismatches: Counter<u64>,
    route_removals: Counter<u64>,
}

impl MetricsReporter {
    pub fn new(provider: &SdkMeterProvider) -> Self {
        let meter = provider.meter("prnsd");
        Self {
            instruments: Instruments {
                runtime_up: meter.u64_gauge("prns.runtime.up").build(),
                uptime_seconds: meter.u64_gauge("prns.runtime.uptime_seconds").build(),
                interfaces: meter.u64_gauge("prns.runtime.interfaces").build(),
                routes: meter.u64_gauge("prns.runtime.routes").build(),
                links: meter.u64_gauge("prns.runtime.links").build(),
                shared_clients: meter.u64_gauge("prns.runtime.shared_clients").build(),
                io_bits_per_second: meter.u64_gauge("prns.runtime.io_bits_per_second").build(),
                io_bytes: meter.u64_gauge("prns.runtime.io_bytes").build(),
                interface_state: meter.u64_gauge("prns.interface.state").build(),
                interface_routes: meter.u64_gauge("prns.interface.routes").build(),
                interface_links: meter.u64_gauge("prns.interface.links").build(),
                interface_io_bits_per_second: meter
                    .u64_gauge("prns.interface.io_bits_per_second")
                    .build(),
                interface_io_bytes: meter.u64_gauge("prns.interface.io_bytes").build(),
                interface_announce_ingress: meter
                    .u64_counter("prns.interface.announces.ingress")
                    .build(),
                interface_announce_egress: meter
                    .u64_counter("prns.interface.announces.egress")
                    .build(),
                interface_announce_egress_bytes: meter
                    .u64_counter("prns.interface.announces.egress_bytes")
                    .build(),
                interface_announce_queue_depth: meter
                    .u64_gauge("prns.interface.announces.queue_depth")
                    .build(),
                engine_packets: meter.u64_counter("prns.engine.packets").build(),
                engine_commands: meter.u64_counter("prns.engine.commands").build(),
                ignored_packets: meter.u64_counter("prns.engine.ignored_packets").build(),
                egress_frames: meter.u64_counter("prns.egress.frames").build(),
                announce_ingress: meter.u64_counter("prns.announces.ingress").build(),
                announce_accepted_by_interface: meter
                    .u64_counter("prns.announces.accepted_by_interface")
                    .build(),
                announce_commands: meter.u64_counter("prns.announces.commands").build(),
                announce_egress: meter.u64_counter("prns.announces.egress").build(),
                announce_egress_by_interface: meter
                    .u64_counter("prns.announces.egress_by_interface")
                    .build(),
                announce_egress_bytes: meter.u64_counter("prns.announces.egress_bytes").build(),
                announce_held: meter.u64_gauge("prns.announces.held").build(),
                announce_scheduled: meter.u64_gauge("prns.announces.scheduled").build(),
                announce_pacer_queue_depth: meter
                    .u64_gauge("prns.announces.pacer_queue_depth")
                    .build(),
                crypto_jobs: meter.u64_counter("prns.crypto.jobs").build(),
                crypto_queue_depth: meter.u64_gauge("prns.crypto.queue_depth").build(),
                crypto_maximum_queue_depth: meter
                    .u64_gauge("prns.crypto.maximum_queue_depth")
                    .build(),
                crypto_backpressure_deferrals: meter
                    .u64_counter("prns.crypto.backpressure_deferrals")
                    .build(),
                crypto_packet_verdicts_owed: meter
                    .u64_gauge("prns.crypto.packet_verdicts_owed")
                    .build(),
                operations: meter.u64_counter("prns.operations").build(),
                resource_failures: meter.u64_counter("prns.resources.failures").build(),
                link_closures: meter.u64_counter("prns.links.closures").build(),
                link_interface_mismatches: meter
                    .u64_counter("prns.links.interface_mismatches")
                    .build(),
                route_removals: meter.u64_counter("prns.routes.removals").build(),
            },
            previous: None,
        }
    }

    pub async fn run(mut self, handle: TokioPrnsHandle, started: Instant) {
        let mut interval = tokio::time::interval(SNAPSHOT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(snapshot) = handle.metrics_snapshot().await else {
                return;
            };
            let interfaces = logical_interface_inventory(&handle.interface_inventory());
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

    fn record(
        &mut self,
        health: RuntimeHealth,
        interfaces: &[InterfaceInventoryEntry],
        snapshot: RuntimeMetricsSnapshot,
    ) {
        self.record_health(health);
        self.record_interfaces(interfaces, &snapshot);
        self.record_engine(&snapshot);
        self.record_egress(&snapshot);
        self.record_crypto(&snapshot);
        self.record_reliability(&snapshot);
        self.previous = Some(snapshot);
    }

    fn record_health(&self, health: RuntimeHealth) {
        self.instruments.runtime_up.record(1, &[]);
        self.instruments
            .uptime_seconds
            .record(health.uptime_millis / 1_000, &[]);
        self.instruments.interfaces.record(
            u64::from(health.interface_count),
            &[KeyValue::new("state", "total")],
        );
        self.instruments.interfaces.record(
            u64::from(health.online_interface_count),
            &[KeyValue::new("state", "online")],
        );
        self.instruments
            .routes
            .record(u64::from(health.route_count), &[]);
        self.instruments.links.record(
            u64::from(health.link_count),
            &[KeyValue::new("kind", "local")],
        );
        self.instruments.links.record(
            u64::from(health.transported_link_count),
            &[KeyValue::new("kind", "transported")],
        );
        self.instruments
            .shared_clients
            .record(u64::from(health.local_client_count), &[]);
        self.instruments
            .io_bits_per_second
            .record(health.rx_bps, &[KeyValue::new("direction", "receive")]);
        self.instruments
            .io_bits_per_second
            .record(health.tx_bps, &[KeyValue::new("direction", "transmit")]);
        self.instruments
            .io_bytes
            .record(health.rx_bytes, &[KeyValue::new("direction", "receive")]);
        self.instruments
            .io_bytes
            .record(health.tx_bytes, &[KeyValue::new("direction", "transmit")]);
    }

    fn record_interfaces(
        &self,
        interfaces: &[InterfaceInventoryEntry],
        snapshot: &RuntimeMetricsSnapshot,
    ) {
        for interface in interfaces {
            let kind = interface
                .snapshot
                .id
                .kind()
                .map_or("unknown", interface_kind_name);
            let attributes = [
                KeyValue::new("interface", metric_interface_name(interface)),
                KeyValue::new("interface_kind", kind),
                KeyValue::new("interface_origin", interface.origin.as_str()),
            ];
            self.instruments.interface_state.record(
                u64::from(interface.snapshot.connection.as_u8()),
                &attributes,
            );
            self.instruments
                .interface_routes
                .record(u64::from(interface.snapshot.destinations), &attributes);
            for (link_kind, count) in [
                ("local", interface.snapshot.links),
                ("transported", interface.snapshot.transported_links),
            ] {
                let link_attributes = [
                    attributes[0].clone(),
                    attributes[1].clone(),
                    attributes[2].clone(),
                    KeyValue::new("kind", link_kind),
                ];
                self.instruments
                    .interface_links
                    .record(u64::from(count), &link_attributes);
            }
            let rates = interface.snapshot.transfer_rates.unwrap_or(
                personal_rns::interfaces::TransferRates {
                    rx_bps: 0,
                    tx_bps: 0,
                },
            );
            for (direction, bits_per_second, bytes) in [
                (
                    "receive",
                    u64::from(rates.rx_bps),
                    interface.snapshot.rx_bytes,
                ),
                (
                    "transmit",
                    u64::from(rates.tx_bps),
                    interface.snapshot.tx_bytes,
                ),
            ] {
                let io_attributes = [
                    attributes[0].clone(),
                    attributes[1].clone(),
                    attributes[2].clone(),
                    KeyValue::new("direction", direction),
                ];
                self.instruments
                    .interface_io_bits_per_second
                    .record(bits_per_second, &io_attributes);
                self.instruments
                    .interface_io_bytes
                    .record(bytes, &io_attributes);
            }

            let ingress = snapshot
                .engine
                .announces
                .interfaces
                .iter()
                .find(|metrics| metrics.interface == interface.snapshot.id);
            let previous_ingress = self.previous.as_ref().and_then(|previous| {
                previous
                    .engine
                    .announces
                    .interfaces
                    .iter()
                    .find(|metrics| metrics.interface == interface.snapshot.id)
            });
            if let Some(ingress) = ingress {
                for (source, outcome, current) in ingress.ingress.iter() {
                    let prior =
                        previous_ingress.map(|metrics| metrics.ingress.get(source, outcome));
                    let announce_attributes = [
                        attributes[0].clone(),
                        attributes[1].clone(),
                        attributes[2].clone(),
                        KeyValue::new("source", announce_source_name(source)),
                        KeyValue::new("outcome", announce_ingress_outcome_name(outcome)),
                    ];
                    add_delta(
                        &self.instruments.interface_announce_ingress,
                        current,
                        prior,
                        &announce_attributes,
                    );
                }
            }
            for (queue, depth) in [
                ("held", ingress.map_or(0, |metrics| metrics.held_depth)),
                (
                    "scheduled",
                    ingress.map_or(0, |metrics| metrics.scheduled_depth),
                ),
            ] {
                let queue_attributes = [
                    attributes[0].clone(),
                    attributes[1].clone(),
                    attributes[2].clone(),
                    KeyValue::new("queue", queue),
                ];
                self.instruments
                    .interface_announce_queue_depth
                    .record(u64::from(depth), &queue_attributes);
            }

            let egress = snapshot
                .egress
                .announces
                .interfaces
                .iter()
                .find(|metrics| metrics.interface == interface.snapshot.id);
            let previous_egress = self.previous.as_ref().and_then(|previous| {
                previous
                    .egress
                    .announces
                    .interfaces
                    .iter()
                    .find(|metrics| metrics.interface == interface.snapshot.id)
            });
            if let Some(egress) = egress {
                for (origin, outcome, current) in egress.outcomes.iter() {
                    let prior =
                        previous_egress.map(|metrics| metrics.outcomes.get(origin, outcome));
                    let announce_attributes = [
                        attributes[0].clone(),
                        attributes[1].clone(),
                        attributes[2].clone(),
                        KeyValue::new("origin", announce_origin_name(origin)),
                        KeyValue::new("outcome", announce_egress_outcome_name(outcome)),
                    ];
                    add_delta(
                        &self.instruments.interface_announce_egress,
                        current,
                        prior,
                        &announce_attributes,
                    );
                }
                for (origin, current) in egress.enqueued_bytes_by_origin.iter() {
                    let prior =
                        previous_egress.map(|metrics| metrics.enqueued_bytes_by_origin.get(origin));
                    let announce_attributes = [
                        attributes[0].clone(),
                        attributes[1].clone(),
                        attributes[2].clone(),
                        KeyValue::new("origin", announce_origin_name(origin)),
                    ];
                    add_delta(
                        &self.instruments.interface_announce_egress_bytes,
                        current,
                        prior,
                        &announce_attributes,
                    );
                }
            }
            let queue_attributes = [
                attributes[0].clone(),
                attributes[1].clone(),
                attributes[2].clone(),
                KeyValue::new("queue", "pacer"),
            ];
            self.instruments.interface_announce_queue_depth.record(
                u64::from(egress.map_or(0, |metrics| metrics.pacer_queue_depth)),
                &queue_attributes,
            );
        }
    }

    fn record_engine(&self, snapshot: &RuntimeMetricsSnapshot) {
        let previous = self.previous.as_ref().map(|previous| &previous.engine);
        add_delta(
            &self.instruments.engine_packets,
            snapshot.engine.ingested_packets,
            previous.map(|metrics| metrics.ingested_packets),
            &[],
        );
        add_delta(
            &self.instruments.engine_commands,
            snapshot.engine.ingested_commands,
            previous.map(|metrics| metrics.ingested_commands),
            &[],
        );
        for (reason, current) in snapshot.engine.ignored_packets.iter() {
            let prior = previous.map(|metrics| metrics.ignored_packets.get(reason));
            add_delta(
                &self.instruments.ignored_packets,
                current,
                prior,
                &[KeyValue::new("reason", ignore_reason_name(reason))],
            );
        }
        for (source, outcome, current) in snapshot.engine.announces.ingress.iter() {
            let prior = previous.map(|metrics| metrics.announces.ingress.get(source, outcome));
            add_delta(
                &self.instruments.announce_ingress,
                current,
                prior,
                &[
                    KeyValue::new("source", announce_source_name(source)),
                    KeyValue::new("outcome", announce_ingress_outcome_name(outcome)),
                ],
            );
        }
        for (kind, current) in snapshot.engine.announces.accepted_by_interface_kind.iter() {
            let prior =
                previous.map(|metrics| metrics.announces.accepted_by_interface_kind.get(kind));
            add_delta(
                &self.instruments.announce_accepted_by_interface,
                current,
                prior,
                &[KeyValue::new("interface_kind", interface_kind_name(kind))],
            );
        }
        let current_unknown = snapshot
            .engine
            .announces
            .accepted_by_interface_kind
            .unknown();
        let prior_unknown =
            previous.map(|metrics| metrics.announces.accepted_by_interface_kind.unknown());
        add_delta(
            &self.instruments.announce_accepted_by_interface,
            current_unknown,
            prior_unknown,
            &[KeyValue::new("interface_kind", "unknown")],
        );
        for (outcome, current) in snapshot.engine.announces.commands.iter() {
            let prior = previous.map(|metrics| metrics.announces.commands.get(outcome));
            add_delta(
                &self.instruments.announce_commands,
                current,
                prior,
                &[KeyValue::new(
                    "outcome",
                    announce_command_outcome_name(outcome),
                )],
            );
        }
        self.instruments
            .announce_held
            .record(u64::from(snapshot.engine.announces.held_depth), &[]);
        self.instruments
            .announce_scheduled
            .record(u64::from(snapshot.engine.announces.scheduled_depth), &[]);
    }

    fn record_egress(&self, snapshot: &RuntimeMetricsSnapshot) {
        let previous = self.previous.as_ref().map(|previous| &previous.egress);
        for (outcome, current, prior) in [
            (
                "enqueued",
                snapshot.egress.enqueued_frames,
                previous.map(|metrics| metrics.enqueued_frames),
            ),
            (
                "interface_unavailable",
                snapshot.egress.unavailable_frame_skips,
                previous.map(|metrics| metrics.unavailable_frame_skips),
            ),
            (
                "lane_full",
                snapshot.egress.full_lane_drops,
                previous.map(|metrics| metrics.full_lane_drops),
            ),
            (
                "lane_missing",
                snapshot.egress.missing_lane_drops,
                previous.map(|metrics| metrics.missing_lane_drops),
            ),
        ] {
            add_delta(
                &self.instruments.egress_frames,
                current,
                prior,
                &[KeyValue::new("outcome", outcome)],
            );
        }
        for (origin, outcome, current) in snapshot.egress.announces.outcomes.iter() {
            let prior = previous.map(|metrics| metrics.announces.outcomes.get(origin, outcome));
            add_delta(
                &self.instruments.announce_egress,
                current,
                prior,
                &[
                    KeyValue::new("origin", announce_origin_name(origin)),
                    KeyValue::new("outcome", announce_egress_outcome_name(outcome)),
                ],
            );
        }
        for (kind, current) in snapshot.egress.announces.enqueued_by_interface_kind.iter() {
            let prior =
                previous.map(|metrics| metrics.announces.enqueued_by_interface_kind.get(kind));
            add_delta(
                &self.instruments.announce_egress_by_interface,
                current,
                prior,
                &[KeyValue::new("interface_kind", interface_kind_name(kind))],
            );
        }
        let current_unknown = snapshot
            .egress
            .announces
            .enqueued_by_interface_kind
            .unknown();
        let prior_unknown =
            previous.map(|metrics| metrics.announces.enqueued_by_interface_kind.unknown());
        add_delta(
            &self.instruments.announce_egress_by_interface,
            current_unknown,
            prior_unknown,
            &[KeyValue::new("interface_kind", "unknown")],
        );
        for (origin, current) in snapshot.egress.announces.enqueued_bytes_by_origin.iter() {
            let prior =
                previous.map(|metrics| metrics.announces.enqueued_bytes_by_origin.get(origin));
            add_delta(
                &self.instruments.announce_egress_bytes,
                current,
                prior,
                &[KeyValue::new("origin", announce_origin_name(origin))],
            );
        }
        self.instruments
            .announce_pacer_queue_depth
            .record(u64::from(snapshot.egress.announces.pacer_queue_depth), &[]);
    }

    fn record_crypto(&self, snapshot: &RuntimeMetricsSnapshot) {
        let Some(current) = snapshot.crypto else {
            return;
        };
        let previous = self.previous.as_ref().and_then(|previous| previous.crypto);
        add_delta(
            &self.instruments.crypto_jobs,
            current.submitted_jobs,
            previous.map(|metrics| metrics.submitted_jobs),
            &[KeyValue::new("outcome", "submitted")],
        );
        add_delta(
            &self.instruments.crypto_jobs,
            current.completed_jobs,
            previous.map(|metrics| metrics.completed_jobs),
            &[KeyValue::new("outcome", "completed")],
        );
        self.instruments
            .crypto_queue_depth
            .record(u64::from(current.queue_depth), &[]);
        self.instruments
            .crypto_maximum_queue_depth
            .record(u64::from(current.maximum_queue_depth), &[]);
        add_delta(
            &self.instruments.crypto_backpressure_deferrals,
            current.backpressure_deferrals,
            previous.map(|metrics| metrics.backpressure_deferrals),
            &[],
        );
        self.instruments
            .crypto_packet_verdicts_owed
            .record(u64::from(current.packet_verdicts_owed), &[]);
    }

    fn record_reliability(&self, snapshot: &RuntimeMetricsSnapshot) {
        let previous = self.previous.as_ref().map(|previous| &previous.reliability);
        for (operation, outcome, current) in snapshot.reliability.operations.iter() {
            let prior = previous.map(|metrics| metrics.operations.get(operation, outcome));
            add_delta(
                &self.instruments.operations,
                current,
                prior,
                &[
                    KeyValue::new("operation", runtime_operation_name(operation)),
                    KeyValue::new("outcome", runtime_operation_outcome_name(outcome)),
                ],
            );
        }
        for (cause, current) in snapshot.reliability.resource_failures.iter() {
            let prior = previous.map(|metrics| metrics.resource_failures.get(cause));
            add_delta(
                &self.instruments.resource_failures,
                current,
                prior,
                &[KeyValue::new("cause", resource_failure_name(cause))],
            );
        }
        for (reason, current) in snapshot.reliability.link_closures.iter() {
            let prior = previous.map(|metrics| metrics.link_closures.get(reason));
            add_delta(
                &self.instruments.link_closures,
                current,
                prior,
                &[KeyValue::new("reason", link_closure_name(reason))],
            );
        }
        add_delta(
            &self.instruments.link_interface_mismatches,
            snapshot.reliability.link_interface_mismatches,
            previous.map(|metrics| metrics.link_interface_mismatches),
            &[],
        );
        for (cause, current) in snapshot.reliability.route_removals.iter() {
            let prior = previous.map(|metrics| metrics.route_removals.get(cause));
            add_delta(
                &self.instruments.route_removals,
                current,
                prior,
                &[KeyValue::new("cause", route_removal_name(cause))],
            );
        }
    }
}

fn delta(current: u64, previous: Option<u64>) -> u64 {
    current.saturating_sub(previous.unwrap_or(0))
}

fn add_delta(counter: &Counter<u64>, current: u64, previous: Option<u64>, attributes: &[KeyValue]) {
    let value = delta(current, previous);
    if value != 0 {
        counter.add(value, attributes);
    }
}

fn announce_source_name(source: AnnounceSourceKind) -> &'static str {
    match source {
        AnnounceSourceKind::Network => "network",
        AnnounceSourceKind::SharedClient => "shared_client",
    }
}

fn announce_ingress_outcome_name(outcome: AnnounceIngressOutcome) -> &'static str {
    match outcome {
        AnnounceIngressOutcome::Accepted => "accepted",
        AnnounceIngressOutcome::Held => "held",
        AnnounceIngressOutcome::Ignored => "ignored",
        AnnounceIngressOutcome::HeldDroppedInterfaceAtCap => "held_dropped_interface_at_cap",
        AnnounceIngressOutcome::HeldDroppedPoolFull => "held_dropped_pool_full",
        AnnounceIngressOutcome::HeldDroppedArenaFull => "held_dropped_arena_full",
        AnnounceIngressOutcome::Blackholed => "blackholed",
    }
}

fn announce_command_outcome_name(outcome: AnnounceCommandOutcome) -> &'static str {
    match outcome {
        AnnounceCommandOutcome::Succeeded => "succeeded",
        AnnounceCommandOutcome::Rejected => "rejected",
        AnnounceCommandOutcome::WriteFailed => "write_failed",
    }
}

fn announce_origin_name(origin: AnnounceOrigin) -> &'static str {
    match origin {
        AnnounceOrigin::Local => "local",
        AnnounceOrigin::SharedClient => "shared_client",
        AnnounceOrigin::Relay => "relay",
    }
}

fn announce_egress_outcome_name(outcome: AnnounceEgressOutcome) -> &'static str {
    match outcome {
        AnnounceEgressOutcome::Enqueued => "enqueued",
        AnnounceEgressOutcome::InterfaceUnavailable => "interface_unavailable",
        AnnounceEgressOutcome::LaneFull => "lane_full",
        AnnounceEgressOutcome::LaneMissing => "lane_missing",
        AnnounceEgressOutcome::IfacRejected => "ifac_rejected",
        AnnounceEgressOutcome::PacerRejected => "pacer_rejected",
    }
}

fn ignore_reason_name(reason: IgnoreReasonKind) -> &'static str {
    match reason {
        IgnoreReasonKind::Consumed => "consumed",
        IgnoreReasonKind::Malformed => "malformed",
        IgnoreReasonKind::UnhandledContext => "unhandled_context",
        IgnoreReasonKind::Duplicate => "duplicate",
        IgnoreReasonKind::Superseded => "superseded",
        IgnoreReasonKind::NotForUs => "not_for_us",
        IgnoreReasonKind::NoRoute => "no_route",
        IgnoreReasonKind::HopLimitReached => "hop_limit_reached",
        IgnoreReasonKind::LoopPrevented => "loop_prevented",
        IgnoreReasonKind::RouteUnresponsive => "route_unresponsive",
        IgnoreReasonKind::OtherInstance => "other_instance",
        IgnoreReasonKind::UnknownLink => "unknown_link",
        IgnoreReasonKind::LinkPhaseMismatch => "link_phase_mismatch",
        IgnoreReasonKind::LinkRttMalformed => "link_rtt_malformed",
        IgnoreReasonKind::LinkRttInvalidToken => "link_rtt_invalid_token",
        IgnoreReasonKind::LinkRttBufferTooShort => "link_rtt_buffer_too_short",
        IgnoreReasonKind::DecryptFailed => "decrypt_failed",
        IgnoreReasonKind::ProofInvalid => "proof_invalid",
        IgnoreReasonKind::UnknownIdentity => "unknown_identity",
        IgnoreReasonKind::LinkRequestsRefused => "link_requests_refused",
        IgnoreReasonKind::PermissionDenied => "permission_denied",
        IgnoreReasonKind::RateLimited => "rate_limited",
        IgnoreReasonKind::CapacityExhausted => "capacity_exhausted",
        IgnoreReasonKind::StrategyDeclined => "strategy_declined",
        IgnoreReasonKind::UnmatchedResponse => "unmatched_response",
        IgnoreReasonKind::IfacRefused => "ifac_refused",
    }
}

fn metric_interface_name(interface: &InterfaceInventoryEntry) -> String {
    interface
        .name
        .clone()
        .unwrap_or_else(|| match interface.snapshot.id.kind() {
            Some(InterfaceKind::LocalServer | InterfaceKind::LocalClient) => {
                String::from("Shared instance")
            }
            Some(kind) => String::from(interface_kind_name(kind)),
            None => String::from("unknown"),
        })
}

fn interface_kind_name(kind: InterfaceKind) -> &'static str {
    match kind {
        InterfaceKind::Loopback => "loopback",
        InterfaceKind::TcpClient => "tcp_client",
        InterfaceKind::TcpServer => "tcp_server",
        InterfaceKind::Udp => "udp",
        InterfaceKind::Serial => "serial",
        InterfaceKind::UsbAutoHost => "usb_auto_host",
        InterfaceKind::UsbAutoDevice => "usb_auto_device",
        InterfaceKind::AutoWifi => "auto_wifi",
        InterfaceKind::WifiPeer => "wifi_peer",
        InterfaceKind::LocalServer => "local_server",
        InterfaceKind::LocalClient => "local_client",
        InterfaceKind::TcpServerPeer => "tcp_server_peer",
        InterfaceKind::BluetoothAuto => "bluetooth_auto",
        InterfaceKind::BluetoothPeer => "bluetooth_peer",
        InterfaceKind::LoRa => "lora",
        InterfaceKind::Kiss => "kiss",
        InterfaceKind::Ax25Kiss => "ax25_kiss",
        InterfaceKind::Pipe => "pipe",
        InterfaceKind::Rnode => "rnode",
        InterfaceKind::BackboneServer => "backbone_server",
        InterfaceKind::BackboneServerPeer => "backbone_server_peer",
        InterfaceKind::BackboneClient => "backbone_client",
        InterfaceKind::EspNow => "esp_now",
        InterfaceKind::WebSocketClient => "websocket_client",
        InterfaceKind::WebSocketServer => "websocket_server",
        InterfaceKind::WebSocketServerPeer => "websocket_server_peer",
        InterfaceKind::WifiDirect => "wifi_direct",
        InterfaceKind::WifiDirectPeer => "wifi_direct_peer",
        InterfaceKind::WifiAware => "wifi_aware",
        InterfaceKind::WifiAwarePeer => "wifi_aware_peer",
    }
}

fn runtime_operation_name(operation: RuntimeOperation) -> &'static str {
    match operation {
        RuntimeOperation::AnnounceNow => "announce_now",
        RuntimeOperation::SendSinglePacket => "send_single_packet",
        RuntimeOperation::SendGroup => "send_group",
        RuntimeOperation::RequestPath => "request_path",
        RuntimeOperation::EstablishLink => "establish_link",
        RuntimeOperation::SendToLink => "send_to_link",
        RuntimeOperation::Identify => "identify",
        RuntimeOperation::SendRequest => "send_request",
        RuntimeOperation::Respond => "respond",
        RuntimeOperation::CloseLink => "close_link",
        RuntimeOperation::SendResource => "send_resource",
        RuntimeOperation::SetResourceStrategy => "set_resource_strategy",
        RuntimeOperation::SendToChannel => "send_to_channel",
        RuntimeOperation::AllowRequester => "allow_requester",
        RuntimeOperation::Inspection => "inspection",
    }
}

fn runtime_operation_outcome_name(outcome: RuntimeOperationOutcome) -> &'static str {
    match outcome {
        RuntimeOperationOutcome::Succeeded => "succeeded",
        RuntimeOperationOutcome::Rejected => "rejected",
        RuntimeOperationOutcome::WriteFailed => "write_failed",
        RuntimeOperationOutcome::Timeout => "timeout",
        RuntimeOperationOutcome::Culled => "culled",
        RuntimeOperationOutcome::PeerRejected => "peer_rejected",
        RuntimeOperationOutcome::Sequencing => "sequencing",
        RuntimeOperationOutcome::DependencyFailed => "dependency_failed",
        RuntimeOperationOutcome::Backpressure => "backpressure",
        RuntimeOperationOutcome::Untrackable => "untrackable",
    }
}

fn resource_failure_name(failure: RuntimeResourceFailure) -> &'static str {
    match failure {
        RuntimeResourceFailure::CancelledBySender => "cancelled_by_sender",
        RuntimeResourceFailure::HashmapBeyondPartCount => "hashmap_beyond_part_count",
        RuntimeResourceFailure::HashmapSkipsAhead => "hashmap_skips_ahead",
        RuntimeResourceFailure::HashmapTooLong => "hashmap_too_long",
        RuntimeResourceFailure::HashmapRagged => "hashmap_ragged",
        RuntimeResourceFailure::RetriesExhausted => "retries_exhausted",
        RuntimeResourceFailure::LinkVanished => "link_vanished",
        RuntimeResourceFailure::TransferUnopenable => "transfer_unopenable",
        RuntimeResourceFailure::TransferCorrupt => "transfer_corrupt",
        RuntimeResourceFailure::ProofUnsendable => "proof_unsendable",
        RuntimeResourceFailure::DecompressionFailed => "decompression_failed",
        RuntimeResourceFailure::DecompressionTimedOut => "decompression_timed_out",
        RuntimeResourceFailure::OpenTimedOut => "open_timed_out",
        RuntimeResourceFailure::MetadataOverrun => "metadata_overrun",
    }
}

fn link_closure_name(reason: RuntimeLinkClosure) -> &'static str {
    match reason {
        RuntimeLinkClosure::Timeout => "timeout",
        RuntimeLinkClosure::PeerClosed => "peer_closed",
        RuntimeLinkClosure::MalformedRtt => "malformed_rtt",
    }
}

fn route_removal_name(cause: RuntimeRouteRemoval) -> &'static str {
    match cause {
        RuntimeRouteRemoval::Expired => "expired",
        RuntimeRouteRemoval::Evicted => "evicted",
        RuntimeRouteRemoval::InterfaceGone => "interface_gone",
        RuntimeRouteRemoval::Dropped => "dropped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cumulative_snapshots_export_only_the_new_work() {
        assert_eq!(delta(10, None), 10);
        assert_eq!(delta(15, Some(10)), 5);
        assert_eq!(delta(3, Some(10)), 0);
    }

    #[test]
    fn every_metric_dimension_has_a_stable_name() {
        for source in AnnounceSourceKind::ALL {
            assert!(!announce_source_name(source).is_empty());
        }
        for outcome in AnnounceIngressOutcome::ALL {
            assert!(!announce_ingress_outcome_name(outcome).is_empty());
        }
        for outcome in AnnounceCommandOutcome::ALL {
            assert!(!announce_command_outcome_name(outcome).is_empty());
        }
        for origin in AnnounceOrigin::ALL {
            assert!(!announce_origin_name(origin).is_empty());
        }
        for outcome in AnnounceEgressOutcome::ALL {
            assert!(!announce_egress_outcome_name(outcome).is_empty());
        }
        for reason in IgnoreReasonKind::ALL {
            assert!(!ignore_reason_name(reason).is_empty());
        }
        for kind in InterfaceKind::ALL {
            assert!(!interface_kind_name(kind).is_empty());
        }
        for operation in RuntimeOperation::ALL {
            assert!(!runtime_operation_name(operation).is_empty());
        }
        for outcome in RuntimeOperationOutcome::ALL {
            assert!(!runtime_operation_outcome_name(outcome).is_empty());
        }
        for failure in RuntimeResourceFailure::ALL {
            assert!(!resource_failure_name(failure).is_empty());
        }
        for reason in RuntimeLinkClosure::ALL {
            assert!(!link_closure_name(reason).is_empty());
        }
        for cause in RuntimeRouteRemoval::ALL {
            assert!(!route_removal_name(cause).is_empty());
        }
    }
}
