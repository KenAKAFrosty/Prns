use super::*;
use crate::plan::error::{GlobalPlanError, PlanningError};
use crate::plan::node::{build_plan, planning_diagnostic};
use crate::reference::keys::{common as common_key, global as global_key};
use crate::reference::{
    ReferenceConfig, ReferenceConfigParams, ReferenceInterface, ReferenceValue,
};
use crate::SourceLocations;
use prns_core::routing::links::resources::ResourceMemoryLimits;

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
fn typed_reference_interfaces_use_the_same_effective_planner() {
    let mut interface = ReferenceInterface::enabled(
        "Typed",
        "TCPClientInterface",
        ReferenceConfigParams::TcpClient {
            target_host: Some("127.0.0.1".to_string()),
            target_port: Some(4242),
            kiss_framing: None,
            i2p_tunneled: None,
            connect_timeout: None,
            max_reconnect_tries: None,
            fixed_mtu: None,
        },
    );
    interface.bitrate = Some(1_000_000);
    let mut config = ReferenceConfig::default();
    config.interfaces.push(interface);
    let plan = plan_reference_config(&config).unwrap();
    let planned = named(&plan, "Typed");
    assert_eq!(planned.policy.bitrate.get(), 1_000_000);
    assert!(matches!(planned.medium, PlannedMedium::TcpClient { .. }));
}

#[test]
fn unrepresentable_global_policy_values_return_source_keyed_errors() {
    for (key, value) in [
        (common_key::IC_NEW_TIME, "1e30"),
        (global_key::DEFAULT_AR_TARGET, "9223372036854775807"),
        (global_key::DEFAULT_AR_GRACE, "65536"),
        (global_key::DEFAULT_AR_PENALTY, "9223372036854775807"),
    ] {
        let mut config = ReferenceConfig::default();
        config
            .globals
            .insert(key.to_string(), ReferenceValue::Scalar(value.to_string()));
        let errors = build_plan(&config).expect_err("the value is not representable");
        assert_eq!(errors, vec![PlanningError::Global(GlobalPlanError { key })]);
        let diagnostic =
            planning_diagnostic("/tmp/rns/config", &SourceLocations::default(), &errors[0]);
        assert_eq!(diagnostic.code(), ConfigDiagnosticCode::InvalidValue);
        assert_eq!(diagnostic.path(), format!("[reticulum] > {key}"));
        assert!(diagnostic.correction().contains(key));
    }

    let mut config = ReferenceConfig::default();
    config.blackhole_exchange.update_interval_minutes = Some(f64::NAN);
    let errors = build_plan(&config).expect_err("non-finite intervals cannot reach the runtime");
    assert_eq!(
        errors,
        vec![PlanningError::Global(GlobalPlanError {
            key: global_key::BLACKHOLE_UPDATE_INTERVAL,
        })]
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
    assert_eq!(plan.probe_responder, ProbeResponderPlan::Disabled);
    assert_eq!(
        plan.resource_memory_limits,
        prns_core::routing::links::resources::ResourceMemoryLimits::DEFAULT_HOST
    );
    assert_eq!(named(&plan, "A").policy.announce_rate_limit, None);
}

#[test]
fn prns_resource_memory_limits_reach_the_daemon_plan_independently() {
    let incoming = plan_of("[prns]\nresource_mem_in = 2 MiB\n");
    assert_eq!(
        incoming.resource_memory_limits,
        ResourceMemoryLimits {
            incoming_bytes: 2 * 1024 * 1024,
            outgoing_bytes: ResourceMemoryLimits::DEFAULT_HOST.outgoing_bytes,
        }
    );

    let outgoing = plan_of("[prns]\nresource_mem_out = 0\n");
    assert_eq!(
        outgoing.resource_memory_limits,
        ResourceMemoryLimits {
            incoming_bytes: ResourceMemoryLimits::DEFAULT_HOST.incoming_bytes,
            outgoing_bytes: 0,
        }
    );
}

#[test]
fn probe_responder_follows_the_explicit_stock_flag() {
    let enabled = plan_of("[reticulum]\nrespond_to_probes = Yes\n");
    assert_eq!(enabled.probe_responder, ProbeResponderPlan::Enabled);
    assert!(enabled.probe_responder.is_enabled());

    let disabled = plan_of("[reticulum]\nrespond_to_probes = No\n");
    assert_eq!(disabled.probe_responder, ProbeResponderPlan::Disabled);
    assert!(!disabled.probe_responder.is_enabled());
}

#[test]
fn blackhole_exchange_is_typed_with_stock_defaults_and_minimum_interval() {
    let defaults = plan_of("");
    assert_eq!(
        defaults.blackhole_exchange.publication(),
        BlackholePublicationPlan::Disabled
    );
    assert!(defaults.blackhole_exchange.sources().is_empty());
    assert_eq!(
        defaults.blackhole_exchange.update_interval(),
        BlackholeUpdateInterval::DEFAULT
    );

    let source = "00112233445566778899aabbccddeeff";
    let configured = plan_of(&format!(
        "[reticulum]\npublish_blackhole = Yes\nblackhole_sources = {source}, {source}\nblackhole_update_interval = -5\n"
    ));
    assert!(configured.blackhole_exchange.publication().is_enabled());
    assert_eq!(configured.blackhole_exchange.sources().len(), 1);
    assert_eq!(
        configured.blackhole_exchange.update_interval(),
        BlackholeUpdateInterval::MINIMUM
    );
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
fn remote_management_is_a_role_complete_plan_with_a_deduplicated_acl() {
    let allowed = "00112233445566778899aabbccddeeff";
    let plan = plan_of(&format!(
        "[reticulum]\nenable_remote_management = Yes\nremote_management_allowed = {allowed}, {allowed}, ffeeddccbbaa99887766554433221100\n"
    ));
    let identities = plan
        .remote_management
        .allowed()
        .expect("remote management is enabled");
    assert_eq!(identities.len(), 2);
    assert_eq!(identities[0].as_bytes(), &hex_16(allowed));
    assert_eq!(
        identities[1].as_bytes(),
        &hex_16("ffeeddccbbaa99887766554433221100")
    );

    let disabled = plan_of(
        "[reticulum]\nenable_remote_management = No\nremote_management_allowed = 00112233445566778899aabbccddeeff\n",
    );
    assert_eq!(disabled.remote_management, RemoteManagementPlan::Disabled);
}

fn hex_16(value: &str) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap();
    }
    bytes
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
fn a_zero_global_announce_rate_target_disables_the_transport_default() {
    let plan = plan_of(
        "[reticulum]\n\
             enable_transport = Yes\n\
             default_ar_target = 0\n\
             [interfaces]\n\
             [[Hub]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = hub\n\
             target_port = 4242\n\
             [[Pinned]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = pinned\n\
             target_port = 4242\n\
             announce_rate_target = 120\n",
    );
    assert_eq!(named(&plan, "Hub").policy.announce_rate_limit, None);
    assert_eq!(
        named(&plan, "Pinned")
            .policy
            .announce_rate_limit
            .unwrap()
            .target_ms,
        120_000
    );
}

#[test]
fn explicit_off_global_announce_rate_target_spellings_disable_the_transport_default() {
    for spelling in ["off", "NO", "False"] {
        let plan = plan_of(&format!(
            "[reticulum]\n\
                 enable_transport = Yes\n\
                 default_ar_target = {spelling}\n\
                 [interfaces]\n\
                 [[Hub]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = hub\n\
                 target_port = 4242\n"
        ));
        assert_eq!(named(&plan, "Hub").policy.announce_rate_limit, None);
    }
}

#[test]
fn an_interface_opts_out_of_the_transport_announce_rate_default() {
    let plan = plan_of(
        "[reticulum]\n\
             enable_transport = Yes\n\
             [interfaces]\n\
             [[Muted]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = muted\n\
             target_port = 4242\n\
             announce_rate_target = off\n\
             [[Zeroed]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = zeroed\n\
             target_port = 4242\n\
             announce_rate_target = 0\n\
             [[Defaulted]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = defaulted\n\
             target_port = 4242\n",
    );
    assert_eq!(named(&plan, "Muted").policy.announce_rate_limit, None);
    assert_eq!(named(&plan, "Zeroed").policy.announce_rate_limit, None);
    assert_eq!(
        named(&plan, "Defaulted").policy.announce_rate_limit,
        Some(AnnounceRateLimit {
            target_ms: 3_600_000,
            grace: 5,
            penalty_ms: 0,
        })
    );
}

#[test]
fn signed_interface_gravity_inherits_and_overrides_the_global_default() {
    let plan = plan_of(
        "[reticulum]\n\
             default_gravity = -23\n\
             [interfaces]\n\
             [[Inherited]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = inherited\n\
             target_port = 4242\n\
             [[Overridden]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = overridden\n\
             target_port = 4242\n\
             gravity = 17\n",
    );

    assert_eq!(
        named(&plan, "Inherited").policy.gravity,
        InterfaceGravity::new(-23)
    );
    assert_eq!(
        named(&plan, "Overridden").policy.gravity,
        InterfaceGravity::new(17)
    );
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
             announces_to_internal = Yes\n\
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
    assert_eq!(
        policy.common.forwarding.recursive_path_requests,
        prns_core::interfaces::RecursivePathRequestPolicy::Enabled
    );
    assert!(!policy.common.forwarding.announces_from_internal);
    assert!(policy.common.forwarding.announces_to_internal);
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
fn recursive_path_configuration_distinguishes_inheritance_from_disable() {
    let plan = plan_of(
        "[interfaces]\n\
             [[Inherited]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = inherited\n\
             target_port = 4242\n\
             [[Disabled]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = disabled\n\
             target_port = 4242\n\
             recursive_prs = No\n",
    );

    assert_eq!(
        named(&plan, "Inherited")
            .policy
            .common
            .forwarding
            .recursive_path_requests,
        prns_core::interfaces::RecursivePathRequestPolicy::InheritNode
    );
    assert_eq!(
        named(&plan, "Disabled")
            .policy
            .common
            .forwarding
            .recursive_path_requests,
        prns_core::interfaces::RecursivePathRequestPolicy::Disabled
    );
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
