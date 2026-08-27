use opentelemetry::metrics::Counter;
use opentelemetry::KeyValue;
use personal_rns::node_introspection::InterfaceInventoryEntry;
use personal_rns::runtime::{RuntimeHealth, RuntimeMetricsSnapshot};

use super::dimensions::*;
use super::MetricsReporter;

impl MetricsReporter {
    pub(super) fn record(
        &mut self,
        health: RuntimeHealth,
        interfaces: &[InterfaceInventoryEntry],
        snapshot: RuntimeMetricsSnapshot,
    ) {
        self.record_health(health);
        self.record_interfaces(interfaces, &snapshot);
        self.record_engine(&snapshot);
        self.record_egress(interfaces, &snapshot);
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
        &mut self,
        interfaces: &[InterfaceInventoryEntry],
        snapshot: &RuntimeMetricsSnapshot,
    ) {
        let mut current_frame_accounting = std::collections::HashMap::new();
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

            if let Some(current) = interface.frame_accounting.complete() {
                let previous = &self.previous_frame_accounting;
                let previous = frame_accounting_previous(
                    previous,
                    interface.snapshot.id,
                    interface.attachment_epoch,
                );
                for (event, current, previous) in [
                    (
                        "received",
                        current.frames_in,
                        previous.map(|accounting| accounting.frames_in),
                    ),
                    (
                        "delivered",
                        current.delivered,
                        previous.map(|accounting| accounting.delivered),
                    ),
                    (
                        "malformed",
                        current.malformed,
                        previous.map(|accounting| accounting.malformed),
                    ),
                    (
                        "protocol_violation",
                        current.protocol_violations,
                        previous.map(|accounting| accounting.protocol_violations),
                    ),
                    (
                        "undecodable",
                        current.undecodable,
                        previous.map(|accounting| accounting.undecodable),
                    ),
                ] {
                    let frame_attributes = [
                        attributes[0].clone(),
                        attributes[1].clone(),
                        attributes[2].clone(),
                        KeyValue::new("event", event),
                    ];
                    let value = reset_aware_delta(current, previous);
                    if value != 0 {
                        self.instruments
                            .interface_receive_frames
                            .add(value, &frame_attributes);
                    }
                }
                current_frame_accounting
                    .insert(interface.snapshot.id, (interface.attachment_epoch, current));
            } else if let Some(previous) = self
                .previous_frame_accounting
                .get(&interface.snapshot.id)
                .filter(|(epoch, _)| *epoch == interface.attachment_epoch)
            {
                current_frame_accounting.insert(interface.snapshot.id, *previous);
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
                for (origin, event, current) in egress.backpressure.iter() {
                    let prior =
                        previous_egress.map(|metrics| metrics.backpressure.get(origin, event));
                    let pressure_attributes = [
                        attributes[0].clone(),
                        attributes[1].clone(),
                        attributes[2].clone(),
                        KeyValue::new("origin", announce_origin_name(origin)),
                        KeyValue::new("event", announce_backpressure_event_name(event)),
                    ];
                    add_delta(
                        &self.instruments.interface_announce_backpressure,
                        current,
                        prior,
                        &pressure_attributes,
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
            for (queue, depth) in [
                (
                    "pacer",
                    egress.map_or(0, |metrics| metrics.pacer_queue_depth),
                ),
                (
                    "pacer_deferred",
                    egress.map_or(0, |metrics| metrics.pacer_deferred_depth),
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
            self.instruments
                .interface_announce_oldest_deferred_age_ms
                .record(
                    egress.map_or(0, |metrics| metrics.pacer_oldest_deferred_age_ms),
                    &attributes,
                );
        }
        self.previous_frame_accounting = current_frame_accounting;
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
        for (outcome, current) in snapshot.engine.path_requests.ingress.iter() {
            let prior = previous.map(|metrics| metrics.path_requests.ingress.get(outcome));
            add_delta(
                &self.instruments.path_request_ingress,
                current,
                prior,
                &[KeyValue::new(
                    "outcome",
                    path_request_ingress_outcome_name(outcome),
                )],
            );
        }
        for (outcome, current) in snapshot.engine.path_requests.relays.iter() {
            let prior = previous.map(|metrics| metrics.path_requests.relays.get(outcome));
            add_delta(
                &self.instruments.path_request_relays,
                current,
                prior,
                &[KeyValue::new(
                    "outcome",
                    path_request_relay_outcome_name(outcome),
                )],
            );
        }
        self.instruments.path_request_pending_discoveries.record(
            u64::from(snapshot.engine.path_requests.pending_discoveries),
            &[],
        );
        for (direction, metrics) in [
            ("incoming", snapshot.engine.resources.incoming),
            ("outgoing", snapshot.engine.resources.outgoing),
        ] {
            let attributes = [KeyValue::new("direction", direction)];
            self.instruments
                .resource_buffer_bytes
                .record(metrics.active_buffer_bytes, &attributes);
            self.instruments
                .resource_buffer_budget_bytes
                .record(metrics.buffer_budget_bytes, &attributes);
            self.instruments
                .resource_active_rows
                .record(u64::from(metrics.active_rows), &attributes);
        }
        self.instruments
            .resource_pending_depth
            .record(u64::from(snapshot.engine.resources.pending_depth), &[]);
        for (event, current) in snapshot.engine.resources.admission_events.iter() {
            let prior = previous.map(|metrics| metrics.resources.admission_events.get(event));
            add_delta(
                &self.instruments.resource_admission_events,
                current,
                prior,
                &[KeyValue::new("event", resource_admission_event_name(event))],
            );
        }
    }

    fn record_egress(
        &self,
        interfaces: &[InterfaceInventoryEntry],
        snapshot: &RuntimeMetricsSnapshot,
    ) {
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
            (
                "ifac_rejected",
                snapshot.egress.ifac_rejected_frames,
                previous.map(|metrics| metrics.ifac_rejected_frames),
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
        for (origin, event, current) in snapshot.egress.announces.backpressure.iter() {
            let prior = previous.map(|metrics| metrics.announces.backpressure.get(origin, event));
            add_delta(
                &self.instruments.announce_backpressure,
                current,
                prior,
                &[
                    KeyValue::new("origin", announce_origin_name(origin)),
                    KeyValue::new("event", announce_backpressure_event_name(event)),
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
        self.instruments.announce_pacer_deferred_depth.record(
            u64::from(snapshot.egress.announces.pacer_deferred_depth),
            &[],
        );
        self.instruments
            .announce_pacer_oldest_deferred_age_ms
            .record(snapshot.egress.announces.pacer_oldest_deferred_age_ms, &[]);
        for lane in &snapshot.egress.lanes {
            let logical_interface = interfaces
                .iter()
                .find(|interface| interface.snapshot.id == lane.logical_interface)
                .map_or_else(
                    || interface_id_name(lane.logical_interface),
                    metric_interface_name,
                );
            let attributes = [
                KeyValue::new("physical_lane", interface_id_name(lane.physical_interface)),
                KeyValue::new(
                    "physical_lane_kind",
                    lane.physical_interface
                        .kind()
                        .map_or("unknown", interface_kind_name),
                ),
                KeyValue::new("logical_interface", logical_interface),
                KeyValue::new(
                    "logical_interface_kind",
                    lane.logical_interface
                        .kind()
                        .map_or("unknown", interface_kind_name),
                ),
            ];
            self.instruments
                .egress_lane_capacity
                .record(u64::from(lane.capacity), &attributes);
            self.instruments
                .egress_lane_occupancy
                .record(u64::from(lane.occupancy), &attributes);
        }
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

fn reset_aware_delta(current: u64, previous: Option<u64>) -> u64 {
    match previous {
        Some(previous) if current >= previous => current - previous,
        Some(_) | None => current,
    }
}

fn frame_accounting_previous(
    previous: &std::collections::HashMap<
        personal_rns::interfaces::InterfaceId,
        (u64, personal_rns::interfaces::FrameAccounting),
    >,
    id: personal_rns::interfaces::InterfaceId,
    attachment_epoch: u64,
) -> Option<personal_rns::interfaces::FrameAccounting> {
    previous
        .get(&id)
        .filter(|(epoch, _)| *epoch == attachment_epoch)
        .map(|(_, accounting)| *accounting)
}

fn add_delta(counter: &Counter<u64>, current: u64, previous: Option<u64>, attributes: &[KeyValue]) {
    let value = delta(current, previous);
    if value != 0 {
        counter.add(value, attributes);
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
    fn frame_snapshots_restart_after_a_source_reset() {
        assert_eq!(reset_aware_delta(10, None), 10);
        assert_eq!(reset_aware_delta(15, Some(10)), 5);
        assert_eq!(reset_aware_delta(3, Some(10)), 3);
        assert_eq!(reset_aware_delta(0, Some(10)), 0);
    }

    #[test]
    fn detached_or_replaced_frame_sources_have_no_previous_snapshot() {
        let id = personal_rns::interfaces::InterfaceId::new([0x51; 8]);
        let accounting = personal_rns::interfaces::FrameAccounting {
            frames_in: 10,
            malformed: 1,
            protocol_violations: 3,
            undecodable: 2,
            delivered: 9,
        };
        let mut previous = std::collections::HashMap::from([(id, (7, accounting))]);

        assert_eq!(
            frame_accounting_previous(&previous, id, 7),
            Some(accounting)
        );
        assert_eq!(frame_accounting_previous(&previous, id, 8), None);
        previous.remove(&id);
        assert_eq!(frame_accounting_previous(&previous, id, 7), None);
    }

    #[test]
    fn unaccounted_and_partial_inventory_never_become_zero_samples() {
        use personal_rns::node_introspection::FrameAccountingCoverage;

        assert_eq!(FrameAccountingCoverage::Unavailable.complete(), None);
        assert_eq!(FrameAccountingCoverage::Incomplete.complete(), None);
    }
}
