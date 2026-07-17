use prns_core::interface_discovery::StampCost;

use crate::diagnostic::{ConfigDiagnosticCode, ConfigErrors};

use super::*;

const REALISTIC: &str = "[reticulum]\n\
    enable_transport = Yes\n\
    share_instance = Yes\n\
    [logging]\n\
    loglevel = 4\n\
    [interfaces]\n\
      [[Default Interface]]\n\
        type = AutoInterface\n\
        enabled = Yes\n\
      [[Hub]]\n\
        type = TCPClientInterface\n\
        interface_enabled = True\n\
        target_host = hub.example.com\n\
        target_port = 4965\n\
        mode = gw\n";

fn has_code(errors: &ConfigErrors, code: ConfigDiagnosticCode) -> bool {
    errors
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code() == code)
}

#[test]
fn parse_reads_globals_interfaces_and_other_sections() {
    let config = parse(REALISTIC).unwrap();
    assert_eq!(config.interfaces.len(), 2);
    assert_eq!(
        config.globals.get("enable_transport"),
        Some(&ReferenceValue::Scalar("Yes".to_string()))
    );
    assert_eq!(
        config.other_sections["logging"].get("loglevel"),
        Some(&ReferenceValue::Scalar("4".to_string()))
    );
}

#[test]
fn parse_coerces_typed_fields_and_folds_dual_keys_and_aliases() {
    let config = parse(REALISTIC).unwrap();
    let hub = &config.interfaces[1];
    assert_eq!(hub.enabled, Some(true));
    assert_eq!(hub.mode, Some(ReferenceMode::Gateway));
    assert_eq!(
        hub.params,
        ReferenceParams::TcpClient {
            target_host: Some("hub.example.com".to_string()),
            target_port: Some(4965),
            kiss_framing: None,
            connect_timeout: None,
            max_reconnect_tries: None,
            fixed_mtu: None,
        }
    );
}

#[test]
fn enabled_rnode_multi_is_an_explicit_unsupported_interface_error() {
    let errors = parse(
        "[interfaces]\n[[Radio]]\ntype = RNodeMultiInterface\nenabled = Yes\nport = /dev/ttyUSB0\n",
    )
    .unwrap_err();
    assert!(has_code(
        &errors,
        ConfigDiagnosticCode::UnsupportedInterface
    ));
}

#[test]
fn parse_types_every_stock_discovery_setting() {
    let config = parse(
        "[reticulum]\n\
           network_identity = ~/.reticulum/storage/identity/network\n\
           discover_interfaces = Yes\n\
           required_discovery_value = 18\n\
           interface_discovery_sources = 00112233445566778899aabbccddeeff, 00112233445566778899AABBCCDDEEFF\n\
           autoconnect_discovered_interfaces = 4\n\
         [interfaces]\n\
           [[Spine]]\n\
             type = BackboneInterface\n\
             enabled = Yes\n\
             listen_port = 4242\n\
             discoverable = Yes\n\
             announce_interval = 10\n\
             discovery_stamp_value = 19\n\
             discovery_name = Public Spine\n\
             discovery_encrypt = Yes\n\
             reachable_on = spine.example.com\n\
             publish_ifac = Yes\n\
             latitude = 41.88\n\
             longitude = -87.63\n\
             height = 181.5\n\
             discovery_frequency = 915000000\n\
             discovery_bandwidth = 125000\n\
             discovery_modulation = LoRa\n",
    )
    .unwrap();

    assert_eq!(
        config.network_identity_path.as_deref(),
        Some("~/.reticulum/storage/identity/network"),
    );
    assert_eq!(config.discovery.discover_interfaces, Some(true));
    assert_eq!(
        config.discovery.required_stamp_cost.map(StampCost::get),
        Some(18),
    );
    assert_eq!(config.discovery.interface_sources.len(), 1);
    assert_eq!(
        config.discovery.interface_sources[0].as_bytes(),
        &[
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff
        ],
    );
    assert_eq!(config.discovery.auto_connect_limit, Some(4));

    let spine = &config.interfaces[0];
    assert_eq!(spine.discovery.discoverable, Some(true));
    assert_eq!(spine.discovery.announce_interval_minutes, Some(10));
    assert_eq!(spine.discovery.stamp_cost.map(StampCost::get), Some(19));
    assert_eq!(spine.discovery.name.as_deref(), Some("Public Spine"));
    assert_eq!(spine.discovery.encrypt, Some(true));
    assert_eq!(
        spine.discovery.reachable_on.as_deref(),
        Some("spine.example.com"),
    );
    assert_eq!(spine.discovery.publish_ifac, Some(true));
    assert_eq!(spine.discovery.latitude, Some(41.88));
    assert_eq!(spine.discovery.longitude, Some(-87.63));
    assert_eq!(spine.discovery.height, Some(181.5));
    assert_eq!(spine.discovery.frequency_hz, Some(915_000_000));
    assert_eq!(spine.discovery.bandwidth_hz, Some(125_000));
    assert_eq!(spine.discovery.modulation.as_deref(), Some("LoRa"));
    assert!(spine.extra.is_empty());
}

#[test]
fn nonpositive_discovery_numbers_select_reference_defaults() {
    let config = parse(
        "[reticulum]\n\
           required_discovery_value = 0\n\
           autoconnect_discovered_interfaces = -2\n\
         [interfaces]\n\
           [[Spine]]\n\
             type = BackboneInterface\n\
             enabled = Yes\n\
             listen_port = 4242\n\
             discoverable = Yes\n\
             discovery_stamp_value = 0\n",
    )
    .unwrap();
    assert_eq!(config.discovery.required_stamp_cost, None);
    assert_eq!(config.discovery.auto_connect_limit, None);
    assert_eq!(config.interfaces[0].discovery.stamp_cost, None);
}

#[test]
fn disabled_publication_leaves_its_conditional_keys_uninterpreted() {
    let config = parse(
        "[interfaces]\n\
           [[Spine]]\n\
             type = BackboneInterface\n\
             enabled = Yes\n\
             discoverable = No\n\
             announce_interval = not-an-integer\n\
             discovery_stamp_value = not-an-integer\n",
    )
    .unwrap();
    let spine = &config.interfaces[0];
    assert_eq!(spine.discovery.discoverable, Some(false));
    assert!(spine.extra.contains_key("announce_interval"));
    assert!(spine.extra.contains_key("discovery_stamp_value"));
}

#[test]
fn malformed_discovery_trust_and_cost_values_are_rejected_in_context() {
    assert!(matches!(
        parse("[reticulum]\ninterface_discovery_sources = 1234\n"),
        Err(ref errors) if has_code(errors, ConfigDiagnosticCode::InvalidValue),
    ));
    assert!(matches!(
        parse("[reticulum]\nrequired_discovery_value = 256\n"),
        Err(ref errors) if has_code(errors, ConfigDiagnosticCode::InvalidValue),
    ));
    assert!(matches!(
        parse(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\ndiscoverable = Yes\ndiscovery_stamp_value = 256\n",
        ),
        Err(ref errors) if has_code(errors, ConfigDiagnosticCode::InvalidValue),
    ));
    assert!(matches!(
        parse(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\ndiscoverable = Yes\ndiscovery_stamp_value = -1\n",
        ),
        Err(ref errors) if has_code(errors, ConfigDiagnosticCode::InvalidValue),
    ));
}

#[test]
fn parse_lands_unmodeled_keys_in_extra() {
    let config = parse(
        "[interfaces]\n\
           [[Custom]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = host\n\
             announce_interval = 30\n\
             discovery_frequency = 867200000\n",
    )
    .unwrap();
    let extra = &config.interfaces[0].extra;
    assert!(extra.contains_key("announce_interval"));
    assert!(extra.contains_key("discovery_frequency"));
}

#[test]
fn enabled_external_module_type_is_an_explicit_unsupported_error() {
    let errors = parse(
        "[interfaces]\n\
           [[Custom]]\n\
             type = MyCustomInterface\n\
             enabled = Yes\n\
             secret = on\n",
    )
    .unwrap_err();
    assert!(has_code(
        &errors,
        ConfigDiagnosticCode::UnsupportedInterface
    ));
}

#[test]
fn parse_errors_on_missing_type() {
    let result = parse("[interfaces]\n[[Broken]]\nenabled = Yes\n");
    assert!(matches!(
        result,
        Err(ref errors) if has_code(errors, ConfigDiagnosticCode::MissingRequiredKey)
    ));
}

#[test]
fn parse_errors_on_an_uncoercible_value() {
    let result = parse(
        "[interfaces]\n\
           [[Hub]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_port = not-a-number\n",
    );
    assert!(matches!(
        result,
        Err(ref errors) if has_code(errors, ConfigDiagnosticCode::InvalidValue)
    ));
}

#[test]
fn bitrate_and_fixed_mtu_fail_with_their_operational_ranges() {
    let errors = parse(
        "[interfaces]\n\
           [[Hub]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             bitrate = 4\n\
             fixed_mtu = 0\n",
    )
    .unwrap_err();

    assert_eq!(errors.len(), 2);
    let rendered = errors.to_string();
    assert!(rendered.contains("integer from 5 through 18446744073709551615 bps"));
    assert!(rendered.contains("integer from 1 through 524288 bytes"));
}

#[test]
fn digit_grouping_underscores_parse_like_python_int() {
    let config = parse(
        "[interfaces]\n\
           [[Hub]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             bitrate = 1_000_000\n\
             target_port = 4_965\n",
    )
    .unwrap();
    let hub = &config.interfaces[0];
    assert_eq!(hub.bitrate, Some(1_000_000));
    assert!(matches!(
        hub.params,
        ReferenceParams::TcpClient {
            target_port: Some(4965),
            ..
        }
    ));
}

#[test]
fn malformed_underscores_are_rejected_like_python_int() {
    for bad in ["1__0", "_5", "5_", "1_"] {
        let config = format!(
            "[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\nbitrate = {bad}\n"
        );
        assert!(
            matches!(
                parse(&config),
                Err(ref errors) if has_code(errors, ConfigDiagnosticCode::InvalidValue)
            ),
            "expected {bad} to be rejected"
        );
    }
}

#[test]
fn globals_outside_reticulum_fail_instead_of_becoming_hidden_fallbacks() {
    let errors = parse_named("/tmp/rns/config", "enable_transport = Yes\n").unwrap_err();
    let diagnostic = &errors.diagnostics()[0];
    assert_eq!(diagnostic.code(), ConfigDiagnosticCode::MisplacedKey);
    assert_eq!(diagnostic.source(), "/tmp/rns/config");
    assert_eq!(diagnostic.line(), 1);
    assert!(diagnostic.to_string().contains("[reticulum]"));
}

#[test]
fn syntax_errors_include_the_source_line_and_concrete_fix() {
    let errors = parse_named("/tmp/rns/config", "[reticulum\n").unwrap_err();
    let diagnostic = &errors.diagnostics()[0];
    assert_eq!(diagnostic.code(), ConfigDiagnosticCode::Syntax);
    assert_eq!(diagnostic.line(), 1);
    assert!(diagnostic.to_string().contains("/tmp/rns/config:1"));
    assert!(diagnostic
        .to_string()
        .contains("correct the syntax on line 1"));
}

#[test]
fn disabled_stanzas_skip_type_and_medium_validation() {
    let report = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Later]]\nenabled = No\ntarget_port = not-a-number\n",
    )
    .unwrap();
    assert!(report.value.interfaces.is_empty());
    assert!(report.warnings.is_empty());
    assert_eq!(report.source, "/tmp/rns/config");
    assert_eq!(
        report
            .locations
            .line(["interfaces", "Later", "target_port"]),
        Some(4)
    );
}

#[test]
fn conflicting_aliases_fail_and_identical_aliases_warn() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Hub]]\ninterface_enabled = Yes\nenabled = No\n",
    )
    .unwrap_err();
    assert!(has_code(&errors, ConfigDiagnosticCode::ConflictingAliases));

    let report = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Hub]]\ntype = TCPClientInterface\ninterface_enabled = Yes\nenabled = true\n",
    )
    .unwrap();
    assert_eq!(report.value.interfaces.len(), 1);
    assert!(report
        .warnings
        .iter()
        .any(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::RedundantAliases));
}

#[test]
fn medium_override_aliases_cannot_silently_disagree() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Listener]]\ntype = TCPServerInterface\nenabled = Yes\nport = 4242\nlisten_port = 4965\n",
    )
    .unwrap_err();
    assert!(has_code(&errors, ConfigDiagnosticCode::ConflictingAliases));

    let report = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\nport = 4242\nlisten_port = 4242\nforward_port = 4242\n",
    )
    .unwrap();
    assert_eq!(
        report
            .warnings
            .iter()
            .filter(|diagnostic| { diagnostic.code() == ConfigDiagnosticCode::RedundantAliases })
            .count(),
        2
    );
}

#[test]
fn independent_semantic_errors_are_aggregated_with_actionable_context() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[reticulum]\ndiscover_interfaces = perhaps\n[logging]\nloglevel = 9\n[interfaces]\n[[Missing]]\nenabled = Yes\n[[Broken]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_port = many\noutgoing = sideways\n",
    )
    .unwrap_err();
    assert_eq!(errors.len(), 5);
    let rendered = errors.to_string();
    assert!(rendered.contains("/tmp/rns/config:2"));
    assert!(rendered.contains("[reticulum] > discover_interfaces"));
    assert!(rendered.contains("found \"perhaps\""));
    assert!(rendered.contains("accepted: yes, no"));
    assert!(rendered.contains("discover_interfaces = Yes"));
    assert!(rendered.contains("[interfaces] > [[Broken]] > target_port"));
    assert!(rendered.contains("[interfaces] > [[Broken]] > outgoing"));
}

#[test]
fn unknown_keys_warn_with_a_nearby_stock_spelling() {
    let report = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_hots = example.com\n",
    )
    .unwrap();
    let warning = report
        .warnings
        .iter()
        .find(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::UnknownKey)
        .unwrap();
    assert!(warning.to_string().contains("target_host"));
}

#[test]
fn unknown_sections_warn_with_a_nearby_stock_spelling() {
    let report = parse_named("/tmp/rns/config", "[reticlum]\nenable_transport = Yes\n").unwrap();
    let warning = &report.warnings[0];
    assert_eq!(warning.code(), ConfigDiagnosticCode::UnknownSection);
    assert!(warning
        .to_string()
        .contains("rename [reticlum] to [reticulum]"));
}

#[test]
fn warnings_are_preserved_when_other_values_make_the_config_invalid() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[reticulum]\ndiscover_interfaces = perhaps\ndiscover_interfases = Yes\n",
    )
    .unwrap_err();
    assert!(has_code(&errors, ConfigDiagnosticCode::InvalidValue));
    assert!(has_code(&errors, ConfigDiagnosticCode::UnknownKey));
}

#[test]
fn unsupported_rnode_uri_transport_is_a_focused_error() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = tcp://radio.example:7633\n",
    )
    .unwrap_err();
    let diagnostic = errors
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::UnsupportedTransport)
        .unwrap();
    assert!(diagnostic.to_string().contains("local serial device path"));
    assert!(diagnostic.to_string().contains("port = /dev/ttyUSB0"));
}

#[test]
fn an_absent_key_is_none_never_a_default() {
    let config = parse(
        "[interfaces]\n\
           [[Mesh]]\n\
             type = UDPInterface\n\
             enabled = Yes\n\
             listen_ip = 0.0.0.0\n\
             listen_port = 4242\n",
    )
    .unwrap();
    let mesh = &config.interfaces[0];
    assert_eq!(mesh.outgoing, None);
    assert_eq!(mesh.mode, None);
}
