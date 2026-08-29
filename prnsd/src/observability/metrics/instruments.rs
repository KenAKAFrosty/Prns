use opentelemetry::metrics::{Counter, Gauge, MeterProvider as _};
use opentelemetry_sdk::metrics::SdkMeterProvider;

pub(super) struct Instruments {
    pub(super) runtime_up: Gauge<u64>,
    pub(super) uptime_seconds: Gauge<u64>,
    pub(super) interfaces: Gauge<u64>,
    pub(super) routes: Gauge<u64>,
    pub(super) links: Gauge<u64>,
    pub(super) shared_clients: Gauge<u64>,
    pub(super) io_bits_per_second: Gauge<u64>,
    pub(super) io_bytes: Gauge<u64>,
    pub(super) interface_state: Gauge<u64>,
    pub(super) interface_routes: Gauge<u64>,
    pub(super) interface_links: Gauge<u64>,
    pub(super) interface_io_bits_per_second: Gauge<u64>,
    pub(super) interface_io_bytes: Gauge<u64>,
    pub(super) interface_receive_frames: Counter<u64>,
    pub(super) interface_announce_ingress: Counter<u64>,
    pub(super) interface_announce_egress: Counter<u64>,
    pub(super) interface_announce_backpressure: Counter<u64>,
    pub(super) interface_announce_egress_bytes: Counter<u64>,
    pub(super) interface_announce_queue_depth: Gauge<u64>,
    pub(super) interface_announce_oldest_deferred_age_ms: Gauge<u64>,
    pub(super) engine_packets: Counter<u64>,
    pub(super) engine_commands: Counter<u64>,
    pub(super) ignored_packets: Counter<u64>,
    pub(super) egress_frames: Counter<u64>,
    pub(super) egress_lane_capacity: Gauge<u64>,
    pub(super) egress_lane_occupancy: Gauge<u64>,
    pub(super) announce_ingress: Counter<u64>,
    pub(super) announce_accepted_by_interface: Counter<u64>,
    pub(super) announce_commands: Counter<u64>,
    pub(super) announce_egress: Counter<u64>,
    pub(super) announce_backpressure: Counter<u64>,
    pub(super) announce_egress_by_interface: Counter<u64>,
    pub(super) announce_egress_bytes: Counter<u64>,
    pub(super) announce_held: Gauge<u64>,
    pub(super) announce_scheduled: Gauge<u64>,
    pub(super) announce_pacer_queue_depth: Gauge<u64>,
    pub(super) announce_pacer_deferred_depth: Gauge<u64>,
    pub(super) announce_pacer_oldest_deferred_age_ms: Gauge<u64>,
    pub(super) path_request_ingress: Counter<u64>,
    pub(super) path_request_relays: Counter<u64>,
    pub(super) path_request_pending_discoveries: Gauge<u64>,
    pub(super) resource_buffer_bytes: Gauge<u64>,
    pub(super) resource_buffer_budget_bytes: Gauge<u64>,
    pub(super) resource_active_rows: Gauge<u64>,
    pub(super) resource_pending_depth: Gauge<u64>,
    pub(super) resource_admission_events: Counter<u64>,
    pub(super) crypto_jobs: Counter<u64>,
    pub(super) crypto_queue_depth: Gauge<u64>,
    pub(super) crypto_maximum_queue_depth: Gauge<u64>,
    pub(super) crypto_backpressure_deferrals: Counter<u64>,
    pub(super) crypto_packet_verdicts_owed: Gauge<u64>,
    pub(super) operations: Counter<u64>,
    pub(super) resource_failures: Counter<u64>,
    pub(super) link_closures: Counter<u64>,
    pub(super) link_interface_mismatches: Counter<u64>,
    pub(super) route_removals: Counter<u64>,
}

impl Instruments {
    pub(super) fn new(provider: &SdkMeterProvider) -> Self {
        let meter = provider.meter("prnsd");
        Self {
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
            interface_receive_frames: meter.u64_counter("prns.interface.receive.frames").build(),
            interface_announce_ingress: meter
                .u64_counter("prns.interface.announces.ingress")
                .build(),
            interface_announce_egress: meter.u64_counter("prns.interface.announces.egress").build(),
            interface_announce_backpressure: meter
                .u64_counter("prns.interface.announces.backpressure")
                .build(),
            interface_announce_egress_bytes: meter
                .u64_counter("prns.interface.announces.egress_bytes")
                .build(),
            interface_announce_queue_depth: meter
                .u64_gauge("prns.interface.announces.queue_depth")
                .build(),
            interface_announce_oldest_deferred_age_ms: meter
                .u64_gauge("prns.interface.announces.oldest_deferred_age_ms")
                .build(),
            engine_packets: meter.u64_counter("prns.engine.packets").build(),
            engine_commands: meter.u64_counter("prns.engine.commands").build(),
            ignored_packets: meter.u64_counter("prns.engine.ignored_packets").build(),
            egress_frames: meter.u64_counter("prns.egress.frames").build(),
            egress_lane_capacity: meter.u64_gauge("prns.egress.lane.capacity").build(),
            egress_lane_occupancy: meter.u64_gauge("prns.egress.lane.occupancy").build(),
            announce_ingress: meter.u64_counter("prns.announces.ingress").build(),
            announce_accepted_by_interface: meter
                .u64_counter("prns.announces.accepted_by_interface")
                .build(),
            announce_commands: meter.u64_counter("prns.announces.commands").build(),
            announce_egress: meter.u64_counter("prns.announces.egress").build(),
            announce_backpressure: meter.u64_counter("prns.announces.backpressure").build(),
            announce_egress_by_interface: meter
                .u64_counter("prns.announces.egress_by_interface")
                .build(),
            announce_egress_bytes: meter.u64_counter("prns.announces.egress_bytes").build(),
            announce_held: meter.u64_gauge("prns.announces.held").build(),
            announce_scheduled: meter.u64_gauge("prns.announces.scheduled").build(),
            announce_pacer_queue_depth: meter.u64_gauge("prns.announces.pacer_queue_depth").build(),
            announce_pacer_deferred_depth: meter
                .u64_gauge("prns.announces.pacer_deferred_depth")
                .build(),
            announce_pacer_oldest_deferred_age_ms: meter
                .u64_gauge("prns.announces.pacer_oldest_deferred_age_ms")
                .build(),
            path_request_ingress: meter.u64_counter("prns.path_requests.ingress").build(),
            path_request_relays: meter.u64_counter("prns.path_requests.relays").build(),
            path_request_pending_discoveries: meter
                .u64_gauge("prns.path_requests.pending_discoveries")
                .build(),
            resource_buffer_bytes: meter.u64_gauge("prns.resources.buffer_bytes").build(),
            resource_buffer_budget_bytes: meter
                .u64_gauge("prns.resources.buffer_budget_bytes")
                .build(),
            resource_active_rows: meter.u64_gauge("prns.resources.active_rows").build(),
            resource_pending_depth: meter.u64_gauge("prns.resources.pending_depth").build(),
            resource_admission_events: meter.u64_counter("prns.resources.admission_events").build(),
            crypto_jobs: meter.u64_counter("prns.crypto.jobs").build(),
            crypto_queue_depth: meter.u64_gauge("prns.crypto.queue_depth").build(),
            crypto_maximum_queue_depth: meter.u64_gauge("prns.crypto.maximum_queue_depth").build(),
            crypto_backpressure_deferrals: meter
                .u64_counter("prns.crypto.backpressure_deferrals")
                .build(),
            crypto_packet_verdicts_owed: meter
                .u64_gauge("prns.crypto.packet_verdicts_owed")
                .build(),
            operations: meter.u64_counter("prns.operations").build(),
            resource_failures: meter.u64_counter("prns.resources.failures").build(),
            link_closures: meter.u64_counter("prns.links.closures").build(),
            link_interface_mismatches: meter.u64_counter("prns.links.interface_mismatches").build(),
            route_removals: meter.u64_counter("prns.routes.removals").build(),
        }
    }
}
