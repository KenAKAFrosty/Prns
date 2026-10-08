use prns_host_uniffi::transport as t;

#[test]
fn counter_preserves_all_128_bits() {
    for value in [0, 1, 1 << 64, u128::MAX] {
        assert_eq!(u128::from(t::WideCounter::from(value)), value);
    }
    assert_eq!(
        t::WideCounter::from(u128::MAX),
        t::WideCounter {
            high: u64::MAX,
            low: u64::MAX
        }
    );
    let batch = prns_host::DiagnosticBatch {
        events: vec![],
        dropped_newest: u128::MAX,
    };
    assert_eq!(
        t::diagnostic_batch(batch),
        vec![t::DiagnosticEvent::DiagnosticsDropped {
            count: u128::MAX.into()
        }]
    );
}

#[test]
fn native_validators_remain_authoritative() {
    let name = t::DestinationName {
        app_name: String::new(),
        aspects: vec!["inbox".into()],
    };
    assert!(prns_host::DestinationName::try_from(name).is_err());
    let limits = t::PrnsLimits {
        pending_commands: 0,
        application_events: 1,
        retained_event_bytes: 1,
        diagnostics: 1,
    };
    assert!(prns_host::PrnsLimits::try_from(limits).is_err());
}

#[test]
fn scalar_bounds_apply_inside_optional_records_and_commands() {
    let routing = t::InterfaceRoutingPolicy {
        gravity: Some(-9_007_199_254_740_992),
        mode: None,
        recursive_path_requests: None,
        announces_from_internal: None,
        announces_to_internal: None,
    };
    assert!(prns_host::InterfaceRoutingPolicy::try_from(routing).is_err());
    let command = t::HostCommand::Request {
        link_id: vec![0; 16],
        path_hash: vec![0; 16],
        payload: vec![],
        timeout: t::ResponseTimeout::LinkDefault,
        maximum_response_bytes: Some(9_007_199_254_740_992),
    };
    assert!(prns_host::HostCommand::try_from(command).is_err());
    let command = t::HostCommand::Request {
        link_id: vec![0; 16],
        path_hash: vec![0; 16],
        payload: vec![],
        timeout: t::ResponseTimeout::LinkDefault,
        maximum_response_bytes: Some(9_007_199_254_740_991),
    };
    assert!(prns_host::HostCommand::try_from(command).is_ok());
}

#[test]
fn identity_inputs_are_validated_and_owned_by_the_zeroizing_core_type() {
    assert!(t::IdentitySecretInput::try_from(vec![7; 63]).is_err());
    let actual = prns_host::IdentityConfig::try_from(t::IdentityConfig::Existing {
        secret: t::IdentitySecretInput::try_from(vec![7; 64]).unwrap(),
    })
    .unwrap();
    let prns_host::IdentityConfig::Existing(secret) = actual else {
        panic!("wrong identity variant")
    };
    assert_eq!(secret.expose(), &[7; 64]);
}

#[test]
fn output_projection_preserves_resource_ownership_metadata() {
    let event = prns_host::ApplicationEvent::ResourceAvailable(prns_host::ResourceAvailable {
        stream_id: prns_host::ResourceStreamId::new(42),
        link_id: prns_host::LinkId::new([2; 16]),
        hash: prns_host::ResourceHash::new([3; 32]),
        metadata: Some(vec![4]),
        total_bytes: u64::MAX,
    });
    let t::ApplicationEvent::ResourceAvailable {
        resource, metadata, ..
    } = t::ApplicationEvent::from(event)
    else {
        panic!("wrong event variant")
    };
    assert_eq!(
        resource,
        t::ResourceDescriptor {
            stream_id: 42,
            total_bytes: u64::MAX
        }
    );
    assert_eq!(metadata, Some(vec![4]));
}

#[test]
fn delivery_proofs_cannot_lose_their_hash() {
    let malformed = t::CommandOutcome::PacketDelivered {
        rtt_millis: 1,
        evidence: t::DeliveryEvidenceKind::ExplicitProof,
        packet_hash: None,
    };
    assert!(prns_host::CommandOutcome::try_from(malformed).is_err());
    let original = prns_host::CommandOutcome::PacketDelivered {
        rtt_millis: 1,
        evidence: prns_host::DeliveryEvidence::ImplicitProof(prns_host::PacketHash::new([9; 32])),
    };
    let restored =
        prns_host::CommandOutcome::try_from(t::CommandOutcome::from(original.clone())).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn remote_control_generated_inputs_use_canonical_bounded_constructors() {
    use prns_host_uniffi::remote_control as rc;
    let request = rc::RemoteControlRequest::ActivateWifiCredentials {
        revision: rc::RemoteControlWifiCredentialRevision { value: 0 },
    };
    assert!(prns_core::remote_control::RemoteControlRequest::try_from(request).is_err());
    let invalid_group = rc::RemoteControlDiscoveryGroups {
        groups: vec!["same".into(), "same".into()],
    };
    assert!(
        prns_core::remote_control::RemoteControlDiscoveryGroups::try_from(invalid_group).is_err()
    );
    let page = rc::RemoteControlInterfacePage::After {
        value: rc::RemoteControlInterfaceCursor {
            value: rc::RemoteControlInterfaceId { value: vec![0; 15] },
        },
    };
    assert!(prns_core::remote_control::RemoteControlInterfacePage::try_from(page).is_err());
}

#[test]
fn remote_control_output_keeps_counter_width_and_nested_failure() {
    use prns_host_uniffi::remote_control as rc;
    let entry = prns_core::remote_control::RemoteControlInterfaceEntry {
        id: prns_core::interfaces::InterfaceId::new([1; 8]),
        kind: prns_core::interfaces::InterfaceKind::Loopback,
        mode: prns_core::interfaces::InterfaceMode::Full,
        connection: prns_core::interfaces::ConnectionState::Connected,
        enabled: true,
        tx_bytes: u64::MAX,
        rx_bytes: (1 << 53) + 1,
        links: 1,
        rate_bytes_per_sec: core::num::NonZeroU32::new(1),
    };
    let projected: rc::RemoteControlInterfaceEntry = entry.into();
    assert_eq!(projected.tx_bytes, u64::MAX);
    assert_eq!(projected.rx_bytes, (1 << 53) + 1);
    assert_eq!(projected.rate_bytes_per_sec, Some(1));
    let failure = prns_host_native::NativeRemoteControlError::Admission(
        prns_host_native::NativeSubmitError::Busy,
    );
    let rc::RemoteControlNativeRemoteControlError::Admission {
        value: rc::RemoteControlNativeSubmitError::Busy,
    } = failure.into()
    else {
        panic!("typed failure changed")
    };
}

#[test]
fn remote_control_app_message_projection_preserves_bytes_and_enforces_the_core_bound() {
    use prns_core::remote_control::{
        RemoteControlAppMessage, RemoteControlRequest, RemoteControlResponse,
        REMOTE_CONTROL_APP_MESSAGE_CAP,
    };
    use prns_host_uniffi::remote_control as rc;
    for length in [0, REMOTE_CONTROL_APP_MESSAGE_CAP] {
        let bytes = vec![0xa5; length];
        let lowered = RemoteControlRequest::try_from(rc::RemoteControlRequest::AppMessage {
            value: rc::RemoteControlAppMessage {
                value: bytes.clone(),
            },
        })
        .unwrap();
        assert_eq!(
            lowered,
            RemoteControlRequest::AppMessage(RemoteControlAppMessage::from_slice(&bytes).unwrap())
        );
        let projected = rc::RemoteControlResponse::from(RemoteControlResponse::AppMessage(
            RemoteControlAppMessage::from_slice(&bytes).unwrap(),
        ));
        let rc::RemoteControlResponse::AppMessage { value } = projected else {
            panic!("app-message response changed")
        };
        assert_eq!(value.value, bytes);
    }
    assert!(
        RemoteControlRequest::try_from(rc::RemoteControlRequest::AppMessage {
            value: rc::RemoteControlAppMessage {
                value: vec![0; REMOTE_CONTROL_APP_MESSAGE_CAP + 1]
            },
        })
        .is_err()
    );
}

#[test]
fn remote_control_watch_projection_enforces_the_core_stream_id_range() {
    use prns_core::remote_control::RemoteControlRequest;
    use prns_core::routing::links::channel::byte_stream::{StreamId, STREAM_ID_MAX};
    use prns_host_uniffi::remote_control as rc;
    let request = RemoteControlRequest::try_from(rc::RemoteControlRequest::WatchInterfaces {
        stream_id: rc::RemoteControlStreamId {
            value: STREAM_ID_MAX,
        },
    })
    .unwrap();
    assert_eq!(
        request,
        RemoteControlRequest::WatchInterfaces {
            stream_id: StreamId::new(STREAM_ID_MAX).unwrap()
        }
    );
    assert!(
        RemoteControlRequest::try_from(rc::RemoteControlRequest::WatchInterfaces {
            stream_id: rc::RemoteControlStreamId {
                value: STREAM_ID_MAX + 1
            },
        })
        .is_err()
    );
}

#[test]
fn remote_control_node_name_projection_retains_canonical_validation() {
    use prns_core::remote_control::{RemoteControlNodeName, REMOTE_CONTROL_NODE_NAME_CAP};
    use prns_host_uniffi::remote_control as rc;
    for name in [
        String::new(),
        " leading".into(),
        "trailing ".into(),
        "line\nbreak".into(),
        "x".repeat(REMOTE_CONTROL_NODE_NAME_CAP + 1),
    ] {
        assert!(
            RemoteControlNodeName::try_from(rc::RemoteControlNodeName { value: name }).is_err()
        );
    }
    let name = "n".repeat(REMOTE_CONTROL_NODE_NAME_CAP);
    let lowered = RemoteControlNodeName::try_from(rc::RemoteControlNodeName {
        value: name.clone(),
    })
    .unwrap();
    assert_eq!(lowered.as_str(), name);
    assert_eq!(rc::RemoteControlNodeName::from(lowered).value, name);
}
