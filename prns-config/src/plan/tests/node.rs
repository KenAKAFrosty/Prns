use super::*;

#[test]
fn global_flags_follow_the_reticulum_section() {
    let plan = plan_of(STOCK);
    assert!(plan.transport.routing_enabled());
    assert_eq!(
        plan.transport.identity_policy(),
        TransportIdentityPolicy::Persistent
    );
    assert_eq!(
        plan.shared_instance,
        SharedInstance::Enabled {
            name: "default".to_string(),
            transport: SharedInstanceTransport::Unix,
            instance_port: 37_428,
            control_port: 37_429,
            rpc_key: None,
            forced_bitrate: None,
        }
    );
    assert_eq!(
        named(&plan, "Default Interface").policy.announce_rate_limit,
        Some(AnnounceRateLimit {
            target_ms: 3_600_000,
            grace: 5,
            penalty_ms: 0,
        })
    );
}

#[test]
fn transport_is_off_and_sharing_on_by_default() {
    let plan = plan_of("[interfaces]\n[[A]]\ntype = AutoInterface\nenabled = Yes\n");
    assert!(!plan.transport.routing_enabled());
    assert_eq!(
        plan.transport.identity_policy(),
        TransportIdentityPolicy::Ephemeral
    );
    assert_eq!(plan.discovery, InterfaceDiscoveryPolicy::Disabled);
    assert!(matches!(
        plan.shared_instance,
        SharedInstance::Enabled { .. }
    ));
    assert_eq!(named(&plan, "A").policy.announce_rate_limit, None);
}

#[test]
fn log_levels_cannot_represent_values_outside_the_stock_range() {
    assert_eq!(LogLevel::new(7).map(LogLevel::get), Some(7));
    assert_eq!(LogLevel::new(8), None);
}

#[test]
fn global_protocol_identity_logging_and_shared_instance_settings_are_typed() {
    let plan = plan_of(
        "[reticulum]\n\
             enable_transport = No\n\
             static_transport_identity = Yes\n\
             local_hops_delta = Yes\n\
             link_mtu_discovery = No\n\
             use_implicit_proof = No\n\
             panic_on_interface_error = Yes\n\
             instance_name = field\n\
             shared_instance_type = TCP\n\
             shared_instance_port = 41_000\n\
             instance_control_port = 41_001\n\
             rpc_key = 00112233\n\
             force_shared_instance_bitrate = 250_000_000\n\
             [logging]\n\
             loglevel = 7\n\
             logtimestamps = No\n",
    );
    assert_eq!(
        plan.transport,
        TransportPlan::Leaf(TransportIdentityPolicy::Persistent)
    );
    assert_eq!(
        plan.protocol,
        ProtocolPlan {
            randomize_local_hop_count: true,
            link_mtu_discovery: false,
            use_implicit_proof: false,
        }
    );
    assert_eq!(
        plan.logging,
        LoggingPlan {
            level: LogLevel::new(7).unwrap(),
            timestamps: false,
        }
    );
    assert!(plan.panic_on_interface_error);
    assert_eq!(
        plan.shared_instance,
        SharedInstance::Enabled {
            name: "field".to_string(),
            transport: SharedInstanceTransport::Tcp,
            instance_port: 41_000,
            control_port: 41_001,
            rpc_key: Some(vec![0x00, 0x11, 0x22, 0x33]),
            forced_bitrate: BitrateBps::new(250_000_000),
        }
    );
}

#[test]
fn grouped_global_controls_reach_the_effective_interface_policy() {
    let plan = plan_of(
        "[reticulum]\n\
             enable_transport = Yes\n\
             ic_max_held_announces = 1_024\n\
             ic_burst_freq = 12_500.5\n\
             default_ar_target = 3_600\n\
             [interfaces]\n\
             [[Hub]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = hub\n\
             target_port = 4242\n",
    );
    let policy = named(&plan, "Hub").policy;
    assert_eq!(policy.common.ingress_control.max_held_announces, 1_024);
    assert_eq!(
        policy.common.ingress_control.announce_burst_frequency.get(),
        12_500_500
    );
    assert_eq!(policy.announce_rate_limit.unwrap().target_ms, 3_600_000);
}

#[test]
fn internal_outgoing_and_common_controls_form_one_effective_policy() {
    let plan = plan_of(
        "[reticulum]\n\
             ic_burst_freq = 12.5\n\
             egress_control = Yes\n\
             [interfaces]\n\
             [[Inside]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = inside\n\
             target_port = 4242\n\
             mode = internal\n\
             outgoing = No\n\
             recursive_prs = Yes\n\
             announces_from_internal = No\n\
             ingress_control = No\n\
             ec_pr_freq = 0\n\
             ic_max_held_announces = 0\n",
    );
    let policy = named(&plan, "Inside").policy;
    assert_eq!(policy.mode, InterfaceMode::Internal);
    assert_eq!(
        policy.capabilities.ingress,
        prns_core::interfaces::IngressCapability::Enabled
    );
    assert_eq!(policy.capabilities.egress, EgressCapability::Disabled);
    assert!(policy.common.forwarding.recursive_path_requests);
    assert!(!policy.common.forwarding.announces_from_internal);
    assert!(!policy.common.ingress_control.enabled);
    assert_eq!(policy.common.ingress_control.max_held_announces, 0);
    assert_eq!(
        policy.common.ingress_control.announce_burst_frequency.get(),
        12_500
    );
    assert!(policy.common.path_request_egress.enabled);
    assert_eq!(policy.common.path_request_egress.frequency.get(), 0);
}

#[test]
fn sharing_off_when_disabled_and_carries_explicit_ports() {
    let plan = plan_of(
            "[reticulum]\nshare_instance = No\n[interfaces]\n[[A]]\ntype = AutoInterface\nenabled = Yes\n",
        );
    assert_eq!(plan.shared_instance, SharedInstance::Disabled);

    let ported = plan_of(
        "[reticulum]\nshared_instance_port = 40000\ninstance_control_port = 40001\n\
             [interfaces]\n[[A]]\ntype = AutoInterface\nenabled = Yes\n",
    );
    assert_eq!(
        ported.shared_instance,
        SharedInstance::Enabled {
            name: "default".to_string(),
            transport: SharedInstanceTransport::Unix,
            instance_port: 40_000,
            control_port: 40_001,
            rpc_key: None,
            forced_bitrate: None,
        }
    );
}
