use personal_rns::engine::{
    AnnounceCommandOutcome, AnnounceIngressOutcome, AnnounceOrigin, AnnounceSourceKind,
    IgnoreReasonKind,
};
use personal_rns::interfaces::InterfaceKind;
use personal_rns::node_introspection::InterfaceInventoryEntry;
use personal_rns::runtime::{
    AnnounceEgressOutcome, RuntimeLinkClosure, RuntimeOperation, RuntimeOperationOutcome,
    RuntimeResourceFailure, RuntimeRouteRemoval,
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
    }
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

    #[test]
    fn i2p_and_weave_interfaces_have_stable_metric_names() {
        assert_eq!(interface_kind_name(InterfaceKind::I2p), "i2p");
        assert_eq!(interface_kind_name(InterfaceKind::I2pPeer), "i2p_peer");
        assert_eq!(interface_kind_name(InterfaceKind::Weave), "weave");
        assert_eq!(interface_kind_name(InterfaceKind::WeavePeer), "weave_peer");
    }
}
