use personal_rns::engine::{
    AnnounceCommandOutcome, AnnounceIngressOutcome, AnnounceOrigin, AnnounceSourceKind,
    IgnoreReasonKind, PathRequestIngressOutcome, PathRequestRelayOutcome, ResourceAdmissionEvent,
};
use personal_rns::interfaces::{InterfaceId, InterfaceKind};
use personal_rns::node_introspection::InterfaceInventoryEntry;
use personal_rns::runtime::{
    AnnounceBackpressureEvent, AnnounceEgressOutcome, RuntimeLinkClosure, RuntimeOperation,
    RuntimeOperationOutcome, RuntimeResourceFailure, RuntimeRouteRemoval,
};

pub(super) fn announce_source_name(source: AnnounceSourceKind) -> &'static str {
    match source {
        AnnounceSourceKind::Network => "network",
        AnnounceSourceKind::SharedClient => "shared_client",
    }
}

pub(super) fn announce_ingress_outcome_name(outcome: AnnounceIngressOutcome) -> &'static str {
    match outcome {
        AnnounceIngressOutcome::Accepted => "accepted",
        AnnounceIngressOutcome::AcceptedScheduleRejectedQueueFull => {
            "accepted_schedule_rejected_queue_full"
        }
        AnnounceIngressOutcome::Held => "held",
        AnnounceIngressOutcome::Ignored => "ignored",
        AnnounceIngressOutcome::HeldDroppedInterfaceAtCap => "held_dropped_interface_at_cap",
        AnnounceIngressOutcome::HeldDroppedPoolFull => "held_dropped_pool_full",
        AnnounceIngressOutcome::HeldDroppedArenaFull => "held_dropped_arena_full",
        AnnounceIngressOutcome::Blackholed => "blackholed",
    }
}

pub(super) fn announce_command_outcome_name(outcome: AnnounceCommandOutcome) -> &'static str {
    match outcome {
        AnnounceCommandOutcome::Succeeded => "succeeded",
        AnnounceCommandOutcome::Rejected => "rejected",
        AnnounceCommandOutcome::WriteFailed => "write_failed",
    }
}

pub(super) fn announce_origin_name(origin: AnnounceOrigin) -> &'static str {
    match origin {
        AnnounceOrigin::Local => "local",
        AnnounceOrigin::SharedClient => "shared_client",
        AnnounceOrigin::Relay => "relay",
    }
}

pub(super) fn announce_egress_outcome_name(outcome: AnnounceEgressOutcome) -> &'static str {
    match outcome {
        AnnounceEgressOutcome::Enqueued => "enqueued",
        AnnounceEgressOutcome::InterfaceUnavailable => "interface_unavailable",
        AnnounceEgressOutcome::LaneFull => "lane_full",
        AnnounceEgressOutcome::LaneMissing => "lane_missing",
        AnnounceEgressOutcome::IfacRejected => "ifac_rejected",
        AnnounceEgressOutcome::PacerRejected => "pacer_rejected",
        AnnounceEgressOutcome::PacerEvicted => "pacer_evicted",
        AnnounceEgressOutcome::PacerExpired => "pacer_expired",
    }
}

pub(super) fn announce_backpressure_event_name(event: AnnounceBackpressureEvent) -> &'static str {
    match event {
        AnnounceBackpressureEvent::Deferred => "deferred",
        AnnounceBackpressureEvent::Retry => "retry",
        AnnounceBackpressureEvent::Recovered => "recovered",
    }
}

pub(super) fn interface_id_name(id: InterfaceId) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut name = String::with_capacity(id.as_bytes().len() * 2);
    for byte in id.as_bytes() {
        name.push(char::from(HEX[usize::from(byte >> 4)]));
        name.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    name
}

pub(super) fn ignore_reason_name(reason: IgnoreReasonKind) -> &'static str {
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
        IgnoreReasonKind::RequestTooLarge => "request_too_large",
        IgnoreReasonKind::IfacRefused => "ifac_refused",
    }
}

pub(super) fn metric_interface_name(interface: &InterfaceInventoryEntry) -> String {
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

pub(super) fn interface_kind_name(kind: InterfaceKind) -> &'static str {
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
        InterfaceKind::I2p => "i2p",
        InterfaceKind::I2pPeer => "i2p_peer",
        InterfaceKind::Weave => "weave",
        InterfaceKind::WeavePeer => "weave_peer",
    }
}

pub(super) fn runtime_operation_name(operation: RuntimeOperation) -> &'static str {
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
        RuntimeOperation::SendPlainPacket => "send_plain_packet",
    }
}

pub(super) fn runtime_operation_outcome_name(outcome: RuntimeOperationOutcome) -> &'static str {
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
        RuntimeOperationOutcome::ResponseTooLarge => "response_too_large",
    }
}

pub(super) fn resource_failure_name(failure: RuntimeResourceFailure) -> &'static str {
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

pub(super) fn link_closure_name(reason: RuntimeLinkClosure) -> &'static str {
    match reason {
        RuntimeLinkClosure::Timeout => "timeout",
        RuntimeLinkClosure::PeerClosed => "peer_closed",
        RuntimeLinkClosure::MalformedRtt => "malformed_rtt",
    }
}

pub(super) fn path_request_ingress_outcome_name(
    outcome: PathRequestIngressOutcome,
) -> &'static str {
    match outcome {
        PathRequestIngressOutcome::Answered => "answered",
        PathRequestIngressOutcome::AnswerScheduled => "answer_scheduled",
        PathRequestIngressOutcome::AnswerScheduleRejected => "answer_schedule_rejected",
        PathRequestIngressOutcome::RelayedRecursive => "relayed_recursive",
        PathRequestIngressOutcome::RelayedAcrossBoundary => "relayed_across_boundary",
        PathRequestIngressOutcome::RelayedToTransports => "relayed_to_transports",
        PathRequestIngressOutcome::OfferedToLocalClients => "offered_to_local_clients",
        PathRequestIngressOutcome::IgnoredMalformed => "ignored_malformed",
        PathRequestIngressOutcome::IgnoredDuplicate => "ignored_duplicate",
        PathRequestIngressOutcome::IgnoredLoopPrevented => "ignored_loop_prevented",
        PathRequestIngressOutcome::IgnoredRouteUnresponsive => "ignored_route_unresponsive",
        PathRequestIngressOutcome::IgnoredRateLimited => "ignored_rate_limited",
        PathRequestIngressOutcome::IgnoredSuperseded => "ignored_superseded",
        PathRequestIngressOutcome::IgnoredNotForUs => "ignored_not_for_us",
        PathRequestIngressOutcome::IgnoredOther => "ignored_other",
    }
}

pub(super) fn path_request_relay_outcome_name(outcome: PathRequestRelayOutcome) -> &'static str {
    match outcome {
        PathRequestRelayOutcome::Sent => "sent",
        PathRequestRelayOutcome::RateLimited => "rate_limited",
    }
}

pub(super) fn resource_admission_event_name(event: ResourceAdmissionEvent) -> &'static str {
    match event {
        ResourceAdmissionEvent::Queued => "queued",
        ResourceAdmissionEvent::Promoted => "promoted",
        ResourceAdmissionEvent::Expired => "expired",
        ResourceAdmissionEvent::Rejected => "rejected",
    }
}

pub(super) fn route_removal_name(cause: RuntimeRouteRemoval) -> &'static str {
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
        for event in AnnounceBackpressureEvent::ALL {
            assert!(!announce_backpressure_event_name(event).is_empty());
        }
        for reason in IgnoreReasonKind::ALL {
            assert!(!ignore_reason_name(reason).is_empty());
        }
        for outcome in PathRequestIngressOutcome::ALL {
            assert!(!path_request_ingress_outcome_name(outcome).is_empty());
        }
        for outcome in PathRequestRelayOutcome::ALL {
            assert!(!path_request_relay_outcome_name(outcome).is_empty());
        }
        for event in ResourceAdmissionEvent::ALL {
            assert!(!resource_admission_event_name(event).is_empty());
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

    #[test]
    fn i2p_and_weave_interfaces_have_stable_metric_names() {
        assert_eq!(interface_kind_name(InterfaceKind::I2p), "i2p");
        assert_eq!(interface_kind_name(InterfaceKind::I2pPeer), "i2p_peer");
        assert_eq!(interface_kind_name(InterfaceKind::Weave), "weave");
        assert_eq!(interface_kind_name(InterfaceKind::WeavePeer), "weave_peer");
    }

    #[test]
    fn interface_ids_have_fixed_width_lowercase_names() {
        assert_eq!(
            interface_id_name(InterfaceId::new([
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef
            ])),
            "0123456789abcdef"
        );
    }

    #[test]
    fn dashboard_classifies_bounded_announce_shedding_as_pressure() {
        let source = include_str!("../../../observability/grafana/prnsd.json");
        let dashboard: serde_json::Value = serde_json::from_str(source).unwrap();
        let panels = dashboard["panels"].as_array().unwrap();
        let panel = |title: &str| {
            panels
                .iter()
                .find(|panel| panel["title"].as_str() == Some(title))
                .unwrap()
        };
        let target_expression = |title: &str| panel(title)["targets"][0]["expr"].as_str().unwrap();

        let operational = target_expression("Operational state");
        let hard_signals = target_expression("Hard signals · 5m");
        for expression in [operational, hard_signals] {
            assert!(!expression.contains("lane_full"));
            assert!(!expression.contains("pacer_rejected"));
            assert!(!expression.contains("pacer_evicted"));
            assert!(expression.contains("lane_missing|ifac_rejected|pacer_expired"));
        }

        let breakdown = panel("Pressure and failure breakdown · 5m");
        assert_eq!(breakdown["type"].as_str(), Some("bargauge"));
        let breakdown_expression = breakdown["targets"][0]["expr"].as_str().unwrap();
        assert!(breakdown_expression.contains("prns_interface_announces_egress_total"));
        assert!(breakdown_expression.contains("prns_interface_announces_backpressure_total"));
        assert!(breakdown_expression.contains("lane_full|pacer_rejected|pacer_evicted"));
        assert!(breakdown.to_string().contains("\"fixedColor\":\"blue\""));

        for title in ["Announce terminal egress outcomes", "Announce backpressure"] {
            let detail = panel(title);
            assert_eq!(
                detail["fieldConfig"]["defaults"]["color"]["mode"].as_str(),
                Some("palette-classic")
            );
            assert!(detail["fieldConfig"]["overrides"]
                .as_array()
                .unwrap()
                .is_empty());
        }

        assert!(source.contains("prns_announces_pacer_deferred_depth"));
        assert!(source.contains("prns_egress_lane_occupancy"));
        assert!(!source.contains(
            "prns_announces_egress_total{outcome!~\\\"enqueued|interface_unavailable\\\"}"
        ));
    }

    #[test]
    fn dashboard_surfaces_receive_evidence_without_promoting_it_to_health() {
        let source = include_str!("../../../observability/grafana/prnsd.json");
        let dashboard: serde_json::Value = serde_json::from_str(source).unwrap();
        let panels = dashboard["panels"].as_array().unwrap();
        let panel = |title: &str| {
            panels
                .iter()
                .find(|panel| panel["title"].as_str() == Some(title))
                .unwrap()
        };

        let failures = panel("Interface receive failures");
        assert_eq!(failures["type"].as_str(), Some("timeseries"));
        let failures_query = failures["targets"][0]["expr"].as_str().unwrap();
        assert!(failures_query.contains("prns_interface_receive_frames_total"));
        assert!(failures_query.contains("event=~\"protocol_violation|malformed|undecodable\""));

        let evidence = panel("Interface receive evidence · selected range");
        assert_eq!(evidence["type"].as_str(), Some("table"));
        let events = [
            "protocol_violation",
            "malformed",
            "undecodable",
            "received",
            "delivered",
        ];
        for (target, event) in evidence["targets"].as_array().unwrap().iter().zip(events) {
            let query = target["expr"].as_str().unwrap();
            assert!(query.contains("prns_interface_receive_frames_total"));
            assert!(query.contains(&format!("event=\"{event}\"")));
        }
        let ordering = &evidence["transformations"][1]["options"]["indexByName"];
        assert_eq!(ordering["interface"].as_u64(), Some(0));
        assert_eq!(ordering["Value #A"].as_u64(), Some(1));
        assert_eq!(ordering["Value #B"].as_u64(), Some(2));
        assert_eq!(ordering["Value #C"].as_u64(), Some(3));
        assert_eq!(ordering["Value #D"].as_u64(), Some(4));
        assert_eq!(ordering["Value #E"].as_u64(), Some(5));

        for title in ["Operational state", "Hard signals · 5m"] {
            assert!(!panel(title)["targets"][0]["expr"]
                .as_str()
                .unwrap()
                .contains("prns_interface_receive_frames_total"));
        }
    }
}
