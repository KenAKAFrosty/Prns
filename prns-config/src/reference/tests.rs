use std::collections::BTreeMap;
use std::fmt::Write as _;

use prns_core::interface_discovery::StampCost;

use crate::diagnostic::{ConfigDiagnosticCode, ConfigErrors};

use super::keys::{
    global as global_key, interface as interface_key, logging as logging_key, prns as prns_key,
    section as section_key,
};
use super::schema::{
    interface_key_rule, known_interface_keys, KeyApplication, SUPPORTED_INTERFACES,
};
use super::validation::example_for_key;
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
        config.globals.get(global_key::ENABLE_TRANSPORT),
        Some(&ReferenceValue::Scalar("Yes".to_string()))
    );
    assert_eq!(
        config.other_sections[section_key::LOGGING].get(logging_key::LEVEL),
        Some(&ReferenceValue::Scalar("4".to_string()))
    );
}

#[test]
fn prns_resource_memory_limits_accept_binary_quantities_without_unknown_warnings() {
    let report = parse_named(
        "/tmp/rns/config",
        "[prns]\nresource_mem_in = 64 MiB\nresource_mem_out = 1_024 KiB\n",
    )
    .unwrap();

    assert_eq!(
        report.value.prns,
        ReferencePrnsConfig {
            resource_mem_in: Some(64 * 1024 * 1024),
            resource_mem_out: Some(1024 * 1024),
        }
    );
    assert!(report.warnings.is_empty());
    assert!(!report.value.other_sections.contains_key(section_key::PRNS));
}

#[test]
fn prns_resource_memory_limits_accept_bare_bytes_and_exact_unit_spellings() {
    for (configured, expected) in [
        ("0", 0),
        ("7 B", 7),
        ("2KiB", 2 * 1024),
        ("3 MiB", 3 * 1024 * 1024),
        ("1 GiB", 1024 * 1024 * 1024),
    ] {
        let config = parse(&format!(
            "[prns]\n{} = {configured}\n",
            prns_key::RESOURCE_MEM_IN
        ))
        .unwrap();
        assert_eq!(config.prns.resource_mem_in, Some(expected), "{configured}");
    }
}

#[test]
fn invalid_prns_resource_memory_limits_report_the_exact_setting() {
    for configured in ["-1", "1.5 MiB", "64 MB", "MiB", "1 mib"] {
        let errors = parse_named(
            "/tmp/rns/config",
            &format!("[prns]\nresource_mem_in = {configured}\n"),
        )
        .unwrap_err();
        let diagnostic = &errors.diagnostics()[0];
        assert_eq!(diagnostic.code(), ConfigDiagnosticCode::InvalidValue);
        assert_eq!(diagnostic.path(), "[prns] > resource_mem_in");
        assert_eq!(diagnostic.line(), 2);
        assert!(diagnostic.to_string().contains("KiB, MiB, or GiB"));
    }

    let overflow = (usize::MAX as u128) + 1;
    let errors = parse_named(
        "/tmp/rns/config",
        &format!("[prns]\nresource_mem_out = {overflow} B\n"),
    )
    .unwrap_err();
    assert_eq!(errors.diagnostics()[0].path(), "[prns] > resource_mem_out");
}

#[test]
fn parse_coerces_typed_fields_and_folds_dual_keys_and_aliases() {
    let config = parse(REALISTIC).unwrap();
    let hub = &config.interfaces[1];
    assert_eq!(hub.enabled, Some(true));
    assert_eq!(hub.mode, Some(ReferenceMode::Gateway));
    assert_eq!(
        hub.params,
        ReferenceConfigParams::TcpClient {
            target_host: Some("hub.example.com".to_string()),
            target_port: Some(4965),
            kiss_framing: None,
            i2p_tunneled: None,
            connect_timeout: None,
            max_reconnect_tries: None,
            fixed_mtu: None,
        }
    );
}

const RNODE_MULTI: &str = "[interfaces]\n\
    [[Dual Radio]]\n\
    type = RNodeMultiInterface\n\
    enabled = Yes\n\
    port = /dev/ttyACM0\n\
    id_callsign = N0CALL\n\
    id_interval = 600\n\
    [[[Sub GHz]]]\n\
    interface_enabled = Yes\n\
    vport = 0\n\
    frequency = 868000000\n\
    bandwidth = 125000\n\
    txpower = -4\n\
    spreadingfactor = 8\n\
    codingrate = 5\n\
    flow_control = Yes\n\
    outgoing = No\n\
    airtime_limit_short = 0\n\
    airtime_limit_long = 5.0\n\
    [[[2.4 GHz]]]\n\
    enabled = Yes\n\
    vport = 1\n\
    frequency = 2400000000\n\
    bandwidth = 812500\n\
    txpower = 10\n\
    spreadingfactor = 7\n\
    codingrate = 6\n";

#[test]
fn rnode_multi_types_each_enabled_child_setting() {
    let config = parse(RNODE_MULTI).expect("valid RNodeMulti config");
    let ReferenceConfigParams::RnodeMulti {
        port,
        id_callsign,
        id_interval,
        subinterfaces,
    } = &config.interfaces[0].params
    else {
        panic!("RNodeMulti parameters expected")
    };
    assert_eq!(port.as_deref(), Some("/dev/ttyACM0"));
    assert_eq!(id_callsign.as_deref(), Some("N0CALL"));
    assert_eq!(*id_interval, Some(600));
    assert_eq!(subinterfaces.len(), 2);
    let low = &subinterfaces[0];
    assert_eq!(low.name, "Sub GHz");
    assert_eq!(low.vport, Some(0));
    assert_eq!(low.radio.frequency, Some(868_000_000));
    assert_eq!(low.radio.txpower, Some(-4));
    assert_eq!(low.flow_control, Some(true));
    assert_eq!(low.outgoing, Some(false));
    assert_eq!(low.airtime_limit_short, Some(0.0));
    assert_eq!(low.airtime_limit_long, Some(5.0));
    assert!(low.extra.is_empty());
    let high = &subinterfaces[1];
    assert_eq!(high.vport, Some(1));
    assert_eq!(high.radio.frequency, Some(2_400_000_000));
    assert_eq!(high.flow_control, None);
    assert_eq!(high.outgoing, None);
}

#[test]
fn disabled_rnode_multi_children_are_skipped_before_their_contents() {
    let config = format!(
        "{RNODE_MULTI}[[[Unused]]]\ninterface_enabled = No\nfrquency = definitely-not-a-number\n"
    );
    let report = parse_named("/tmp/rns/config", &config).expect("disabled child is ignored");
    let ReferenceConfigParams::RnodeMulti { subinterfaces, .. } =
        &report.value.interfaces[0].params
    else {
        panic!("RNodeMulti parameters expected")
    };
    assert_eq!(subinterfaces.len(), 2);
    assert!(report.warnings.is_empty());
}

#[test]
fn rnode_multi_requires_an_enabled_complete_unique_vport() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Dual]]\ntype = RNodeMultiInterface\nenabled = Yes\nport = /dev/ttyACM0\n[[[First]]]\ninterface_enabled = Yes\nvport = 0\nfrequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n[[[Second]]]\ninterface_enabled = Yes\nvport = 0\n",
    )
    .unwrap_err();
    assert_eq!(
        errors
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::MissingRequiredKey)
            .count(),
        5
    );
    let duplicate = errors
        .diagnostics()
        .iter()
        .find(|diagnostic| {
            diagnostic.code() == ConfigDiagnosticCode::InvalidValue
                && diagnostic.path().ends_with("vport")
        })
        .expect("duplicate vport diagnostic");
    assert_eq!(duplicate.line(), 16);
    assert!(duplicate
        .message()
        .contains("already assigned to subinterface \"First\""));
    assert_eq!(duplicate.correction(), "set `vport = 1`");
}

#[test]
fn rnode_multi_child_ranges_are_hardware_specific_and_aggregated() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Dual]]\ntype = RNodeMultiInterface\nenabled = Yes\nport = /dev/ttyACM0\n[[[Bad]]]\ninterface_enabled = Yes\nvport = 11\nfrequency = 1500000000\nbandwidth = 7799\ntxpower = -10\nspreadingfactor = 13\ncodingrate = 9\nairtime_limit_short = -1\nairtime_limit_long = 101\n",
    )
    .unwrap_err();
    assert_eq!(errors.len(), 8);
    let rendered = errors.to_string();
    assert!(rendered.contains("an integer from 0 through 10"));
    assert!(rendered.contains("137000000 through 1000000000 Hz"));
    assert!(rendered.contains("2200000000 through 2600000000 Hz"));
    assert!(rendered.contains("an integer from -9 through 37 dBm"));
    assert!(rendered.contains("a percentage from 0 through 100"));
}

#[test]
fn rnode_multi_nested_aliases_and_typos_keep_exact_locations() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Dual]]\ntype = RNodeMultiInterface\nenabled = Yes\nport = /dev/ttyACM0\n[[[Radio]]]\ninterface_enabled = Yes\nenabled = No\nvport = 0\nfrequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\nflow_contol = Yes\n",
    )
    .unwrap_err();
    let conflict = errors
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::ConflictingAliases)
        .expect("conflicting child aliases");
    assert_eq!(conflict.line(), 8);
    assert_eq!(
        conflict.path(),
        "[interfaces] > [[Dual]] > [[[Radio]]] > interface_enabled"
    );
    let typo = errors
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::UnknownKey)
        .expect("nested typo warning");
    assert_eq!(typo.line(), 15);
    assert!(typo.correction().contains("flow_control"));
}

#[test]
fn rnode_multi_requires_at_least_one_enabled_child() {
    for children in ["", "[[[Off]]]\ninterface_enabled = No\n"] {
        let config = format!(
            "[interfaces]\n[[Dual]]\ntype = RNodeMultiInterface\nenabled = Yes\nport = /dev/ttyACM0\n{children}"
        );
        let errors = parse(&config).unwrap_err();
        assert!(errors.diagnostics().iter().any(|diagnostic| {
            diagnostic.code() == ConfigDiagnosticCode::MissingRequiredKey
                && diagnostic.path().ends_with("[[[subinterface]]]")
        }));
    }
}

#[test]
fn rnode_multi_parent_does_not_absorb_child_only_radio_controls() {
    let config = RNODE_MULTI.replacen(
        "port = /dev/ttyACM0\n",
        "port = /dev/ttyACM0\nflow_control = Yes\nairtime_limit_short = 2.0\n",
        1,
    );
    let report = parse_named("/tmp/rns/config", &config).expect("warnings do not reject config");
    let warnings = report
        .warnings
        .iter()
        .filter(|warning| warning.code() == ConfigDiagnosticCode::UnknownKey)
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 2);
    assert!(warnings.iter().all(|warning| {
        warning
            .path()
            .starts_with("[interfaces] > [[Dual Radio]] > ")
            && !warning.path().contains("[[[Sub GHz]]]")
    }));
}

#[test]
fn rnode_multi_reference_parsing_reaches_typed_member_planning() {
    let plan = crate::parse_and_plan(RNODE_MULTI)
        .expect("valid RNodeMulti planning")
        .value;
    assert_eq!(plan.interfaces.len(), 2);
    assert_eq!(plan.interfaces[0].name, "Dual Radio[Sub GHz]");
    assert_eq!(plan.interfaces[1].name, "Dual Radio[2.4 GHz]");
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
           autoconnect_interface_gravity = -31\n\
           autoconnect_announces_to_internal = Yes\n\
           default_gravity = -7\n\
         [interfaces]\n\
           [[Spine]]\n\
             type = BackboneInterface\n\
             enabled = Yes\n\
             gravity = 19\n\
             listen_port = 4242\n\
             discoverable = Yes\n\
             announce_interval = 10\n\
             discovery_stamp_value = 19\n\
             discovery_name = Public Spine\n\
             discovery_encrypt = Yes\n\
             reachable_on = spine.example.com\n\
             reachable_port = 18443\n\
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
    assert_eq!(config.discovery.auto_connect_gravity, Some(-31));
    assert_eq!(
        config.discovery.auto_connect_announces_to_internal,
        Some(true)
    );
    assert_eq!(
        config
            .globals
            .get("default_gravity")
            .and_then(ReferenceValue::as_scalar),
        Some("-7")
    );

    let spine = &config.interfaces[0];
    assert_eq!(spine.gravity, Some(19));
    assert_eq!(spine.discovery.discoverable, Some(true));
    assert_eq!(spine.discovery.announce_interval_minutes, Some(10));
    assert_eq!(spine.discovery.stamp_cost.map(StampCost::get), Some(19));
    assert_eq!(spine.discovery.name.as_deref(), Some("Public Spine"));
    assert_eq!(spine.discovery.encrypt, Some(true));
    assert_eq!(
        spine.discovery.reachable_on.as_deref(),
        Some("spine.example.com"),
    );
    assert_eq!(spine.discovery.reachable_port, Some(18443));
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
    let report = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n\
           [[Spine]]\n\
             type = BackboneInterface\n\
             enabled = Yes\n\
             listen_port = 4242\n\
             discoverable = No\n\
             announce_interval = not-an-integer\n\
             discovery_stamp_value = not-an-integer\n\
             discovery_name = anything\n\
             discovery_encrypt = not-a-boolean\n\
             reachable_on = anywhere\n\
             publish_ifac = not-a-boolean\n\
             latitude = not-a-number\n\
             longitude = not-a-number\n\
             height = not-a-number\n\
             discovery_frequency = not-an-integer\n\
             discovery_bandwidth = not-an-integer\n\
             discovery_modulation = anything\n",
    )
    .unwrap();
    let config = report.value;
    let spine = &config.interfaces[0];
    assert_eq!(spine.discovery.discoverable, Some(false));
    assert!(spine.extra.contains_key("announce_interval"));
    assert!(spine.extra.contains_key("discovery_stamp_value"));
    assert!(spine.extra.contains_key("discovery_encrypt"));
    assert!(spine.extra.contains_key("publish_ifac"));
    assert!(spine.extra.contains_key("latitude"));
    assert!(spine.extra.contains_key("discovery_frequency"));
    assert_eq!(
        report
            .warnings
            .iter()
            .filter(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::IneffectiveSetting)
            .count(),
        12
    );
    assert!(report
        .warnings
        .iter()
        .all(|warning| { warning.correction().contains("set `discoverable` = Yes") }));
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
    assert!(matches!(
        parse(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\ndiscoverable = Yes\nreachable_port = 0\n",
        ),
        Err(ref errors) if has_code(errors, ConfigDiagnosticCode::InvalidValue),
    ));
}

#[test]
fn inactive_discovery_keys_remain_in_extra() {
    let config = parse(
        "[interfaces]\n\
           [[Custom]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = host\n\
             target_port = 4242\n\
             announce_interval = 30\n\
             discovery_frequency = 867200000\n",
    )
    .unwrap();
    let extra = &config.interfaces[0].extra;
    assert!(extra.contains_key("announce_interval"));
    assert!(extra.contains_key("discovery_frequency"));
}

#[test]
fn every_active_interface_setting_leaves_the_loose_parser_remainder() {
    let mut checked = 0;
    for type_name in SUPPORTED_INTERFACES {
        for key in known_interface_keys(type_name) {
            if key == interface_key::TYPE || interface_key::ALIASES.contains(&key) {
                continue;
            }
            let rule = interface_key_rule(type_name, key)
                .unwrap_or_else(|| panic!("{type_name} key {key:?} has no schema rule"));
            if rule.application() == KeyApplication::FollowOn {
                continue;
            }
            let config = application_contract_config(
                type_name,
                key,
                example_for_key(key, rule.value_kind()),
            );
            let report =
                parse_named("/tmp/rns/application-contract", &config).unwrap_or_else(|errors| {
                    panic!("{type_name} key {key:?} did not parse:\n{errors:?}\n{config}")
                });
            let interface = report
                .value
                .interfaces
                .first()
                .unwrap_or_else(|| panic!("{type_name} key {key:?} produced no interface"));
            assert!(
                !interface.extra.contains_key(key),
                "{type_name} key {key:?} was marked active but remained loose"
            );
            checked += 1;
        }
    }
    assert!(checked > 100);
}

fn application_contract_config(type_name: &'static str, key: &'static str, value: &str) -> String {
    let mut settings = application_contract_baseline(type_name);
    if key == interface_key::REMOTE {
        settings.remove(interface_key::TARGET_HOST);
    }
    let rule = interface_key_rule(type_name, key).expect("contract keys have schema rules");
    if rule.application() == KeyApplication::DiscoveryOnly {
        settings.insert(interface_key::DISCOVERABLE, "Yes");
    }
    let value = if type_name == "PrnsWebSocketClient" && key == interface_key::TARGET {
        "ws://peer.example:4242/prns"
    } else {
        value
    };
    settings.insert(key, value);

    let mut config = String::from("[interfaces]\n[[Application Contract]]\n");
    for (setting, configured) in settings {
        writeln!(&mut config, "{setting} = {configured}").expect("writing to a string succeeds");
    }
    if type_name == "RNodeMultiInterface" {
        config.push_str(
            "[[[Radio]]]\n\
             enabled = Yes\n\
             vport = 0\n\
             frequency = 868000000\n\
             bandwidth = 125000\n\
             txpower = 7\n\
             spreadingfactor = 8\n\
             codingrate = 5\n",
        );
    }
    config
}

fn application_contract_baseline(type_name: &'static str) -> BTreeMap<&'static str, &'static str> {
    let mut settings = BTreeMap::from([
        (interface_key::TYPE, type_name),
        (interface_key::ENABLED, "Yes"),
    ]);
    match type_name {
        "AutoInterface" | "I2PInterface" | "PrnsUsbAuto" | "PrnsBluetoothAuto" => {}
        "TCPClientInterface" => {
            settings.insert(interface_key::TARGET_HOST, "peer.example");
            settings.insert(interface_key::TARGET_PORT, "4242");
        }
        "TCPServerInterface" => {
            settings.insert(interface_key::LISTEN_PORT, "4242");
        }
        "UDPInterface" => {
            settings.insert(interface_key::LISTEN_IP, "0.0.0.0");
            settings.insert(interface_key::LISTEN_PORT, "4242");
            settings.insert(interface_key::FORWARD_IP, "127.0.0.1");
            settings.insert(interface_key::FORWARD_PORT, "4242");
        }
        "SerialInterface" => {
            settings.insert(interface_key::PORT, "/dev/ttyACM0");
        }
        "KISSInterface" => {
            settings.insert(interface_key::PORT, "/dev/ttyACM0");
            settings.insert(interface_key::ID_CALLSIGN, "N0CALL");
            settings.insert(interface_key::ID_INTERVAL, "600");
        }
        "AX25KISSInterface" => {
            settings.insert(interface_key::PORT, "/dev/ttyACM0");
            settings.insert(interface_key::CALLSIGN, "N0CALL");
            settings.insert(interface_key::SSID, "0");
        }
        "RNodeInterface" | "RNodeMultiInterface" => {
            settings.insert(interface_key::PORT, "/dev/ttyACM0");
            settings.insert(interface_key::ID_CALLSIGN, "N0CALL");
            settings.insert(interface_key::ID_INTERVAL, "600");
            if type_name == "RNodeInterface" {
                settings.insert(interface_key::FREQUENCY, "868000000");
                settings.insert(interface_key::BANDWIDTH, "125000");
                settings.insert(interface_key::TXPOWER, "7");
                settings.insert(interface_key::SPREADINGFACTOR, "8");
                settings.insert(interface_key::CODINGRATE, "5");
            }
        }
        "PipeInterface" => {
            settings.insert(interface_key::COMMAND, "/path/to/program");
        }
        "BackboneInterface" | "BackboneClientInterface" => {
            settings.insert(interface_key::TARGET_HOST, "peer.example");
            settings.insert(interface_key::TARGET_PORT, "4242");
        }
        "WeaveInterface" => {
            settings.insert(interface_key::PORT, "/dev/ttyACM0");
        }
        "PrnsWebSocketClient" => {
            settings.insert(interface_key::TARGET, "ws://peer.example:4242/prns");
            settings.insert(interface_key::FRAMING, "raw");
        }
        "PrnsWebSocketServer" => {
            settings.insert(interface_key::PORT, "4242");
            settings.insert(interface_key::FRAMING, "raw");
        }
        _ => panic!("unsupported contract interface {type_name}"),
    }
    settings
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
fn prns_interface_names_accept_explicit_and_short_case_insensitive_aliases() {
    for (configured, canonical) in [
        ("prnsusbauto", "PrnsUsbAuto"),
        ("PRNSUSBAUTOINTERFACE", "PrnsUsbAuto"),
        ("prnsbluetoothauto", "PrnsBluetoothAuto"),
        ("PRNSBLUETOOTHAUTOINTERFACE", "PrnsBluetoothAuto"),
        ("prnsbleauto", "PrnsBluetoothAuto"),
        ("PRNSBLEAUTOINTERFACE", "PrnsBluetoothAuto"),
        ("prnswebsocketclient", "PrnsWebSocketClient"),
        ("PRNSWEBSOCKETCLIENTINTERFACE", "PrnsWebSocketClient"),
        ("prnswebsocketserver", "PrnsWebSocketServer"),
        ("PRNSWEBSOCKETSERVERINTERFACE", "PrnsWebSocketServer"),
    ] {
        let settings = match canonical {
            "PrnsWebSocketClient" => "target = ws://peer.example:4242/prns\nframing = raw\n",
            "PrnsWebSocketServer" => "port = 4242\nframing = raw\n",
            _ => "",
        };
        let config =
            format!("[interfaces]\n[[Prns]]\ntype = {configured}\nenabled = Yes\n{settings}");
        let parsed = parse(&config).unwrap_or_else(|errors| panic!("{configured}: {errors:?}"));
        assert_eq!(parsed.interfaces[0].type_name, canonical, "{configured}");
    }
}

#[test]
fn stock_interface_names_remain_case_sensitive() {
    let errors =
        parse("[interfaces]\n[[Wrong Case]]\ntype = autointerface\nenabled = Yes\n").unwrap_err();
    assert!(has_code(
        &errors,
        ConfigDiagnosticCode::UnsupportedInterface
    ));
}

#[test]
fn websocket_targets_require_a_supported_websocket_url() {
    for target in ["peer.example", "ws://bad target", "wss://bad target"] {
        let errors = parse(&format!(
            "[interfaces]\n[[WebSocket]]\ntype = PrnsWebSocketClient\nenabled = Yes\ntarget = {target}\nframing = raw\n"
        ))
        .unwrap_err();
        assert!(has_code(&errors, ConfigDiagnosticCode::InvalidValue));
    }

    for target in ["ws://peer.example:4284/prns", "wss://peer.example/prns"] {
        parse(&format!(
            "[interfaces]\n[[WebSocket]]\ntype = PrnsWebSocketClient\nenabled = Yes\ntarget = {target}\nframing = raw\n"
        ))
        .expect("supported WebSocket targets parse");
    }
}

#[test]
fn websocket_framing_is_optional_and_closed() {
    for type_and_endpoint in [
        "type = PrnsWebSocketClient\ntarget = ws://peer.example/prns",
        "type = PrnsWebSocketServer\nport = 4242",
    ] {
        parse(&format!(
            "[interfaces]\n[[WebSocket]]\n{type_and_endpoint}\nenabled = Yes\n"
        ))
        .expect("omitted WebSocket framing uses automatic selection");

        for framing in ["auto", "raw", "hdlc", "kiss"] {
            parse(&format!(
                "[interfaces]\n[[WebSocket]]\n{type_and_endpoint}\nenabled = Yes\nframing = {framing}\n"
            ))
            .expect("supported WebSocket framing parses");
        }

        let errors = parse(&format!(
            "[interfaces]\n[[WebSocket]]\n{type_and_endpoint}\nenabled = Yes\nframing = slip\n"
        ))
        .unwrap_err();
        assert!(has_code(&errors, ConfigDiagnosticCode::InvalidValue));
    }
}

#[test]
fn singleton_prns_auto_interfaces_reject_duplicate_enabled_stanzas() {
    for (first, second) in [
        ("PrnsUsbAuto", "PRNSUSBAUTOINTERFACE"),
        ("PrnsBluetoothAuto", "prnsbleauto"),
    ] {
        let errors = parse(&format!(
            "[interfaces]\n[[First]]\ntype = {first}\nenabled = Yes\n[[Second]]\ntype = {second}\nenabled = Yes\n"
        ))
        .unwrap_err();
        assert!(has_code(&errors, ConfigDiagnosticCode::InvalidValue));
    }
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
             target_host = host\n\
             target_port = 4242\n\
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
             target_host = host\n\
             target_port = 4_965\n",
    )
    .unwrap();
    let hub = &config.interfaces[0];
    assert_eq!(hub.bitrate, Some(1_000_000));
    assert!(matches!(
        hub.params,
        ReferenceConfigParams::TcpClient {
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
        "[interfaces]\n[[Hub]]\ntype = TCPClientInterface\ninterface_enabled = Yes\nenabled = true\ninterface_mode = full\nmode = full\nnetwork_name = mesh\nnetworkname = mesh\npass_phrase = secret\npassphrase = secret\ntarget_host = host\ntarget_port = 4242\n",
    )
    .unwrap();
    assert_eq!(report.value.interfaces.len(), 1);
    let interface = &report.value.interfaces[0];
    assert_eq!(interface.mode, Some(ReferenceMode::Full));
    assert_eq!(interface.network_name.as_deref(), Some("mesh"));
    assert_eq!(interface.passphrase.as_deref(), Some("secret"));
    assert!(interface.extra.is_empty());
    assert_eq!(
        report
            .warnings
            .iter()
            .filter(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::RedundantAliases)
            .count(),
        4
    );
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
        "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\nport = 4242\nlisten_ip = 0.0.0.0\nlisten_port = 4242\nforward_ip = 255.255.255.255\nforward_port = 4242\n",
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
        "[reticulum]\ndiscover_interfaces = perhaps\n[logging]\nloglevel = 9\n[interfaces]\n[[Missing]]\nenabled = Yes\n[[Broken]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = host\ntarget_port = many\noutgoing = sideways\n",
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
fn incomplete_host_network_endpoints_fail_with_concrete_repairs() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n\
         [[Client]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = peer\n\
         [[Datagram]]\ntype = UDPInterface\nenabled = Yes\nforward_ip = 255.255.255.255\n",
    )
    .unwrap_err();
    assert_eq!(errors.len(), 2);
    let rendered = errors.to_string();
    assert!(rendered.contains("[[Client]] > target_port"));
    assert!(rendered.contains("target_port = 4242"));
    assert!(rendered.contains("[[Datagram]] > forward_port"));
    assert!(rendered.contains("forward_port = 4242"));
}

#[test]
fn unknown_keys_warn_with_a_nearby_stock_spelling() {
    let report = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = example.com\ntarget_port = 4242\ntarget_hots = example.com\n",
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
fn recognized_follow_ons_warn_at_their_exact_source_locations() {
    let report = parse_named(
        "/tmp/rns/config",
        "[reticulum]\nenable_remote_management = Yes\nrespond_to_probes = No\npublish_blackhole = No\n[interfaces]\n[[LAN]]\ntype = AutoInterface\nenabled = Yes\ndiscovery_port = 29716\nbootstrap_only = Yes\nignore_config_warnings = Yes\n",
    )
    .unwrap();
    let warnings = report
        .warnings
        .iter()
        .filter(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::UnsupportedSetting)
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].source(), "/tmp/rns/config");
    assert!(!warnings
        .iter()
        .any(|warning| warning.path().ends_with("enable_remote_management")));
    assert!(!warnings
        .iter()
        .any(|warning| warning.path().ends_with("respond_to_probes")));
    assert!(!warnings
        .iter()
        .any(|warning| warning.path().ends_with("publish_blackhole")));
    assert!(!warnings.iter().any(|warning| {
        warning.path().ends_with("discovery_port") || warning.path().ends_with("bootstrap_only")
    }));
    assert!(warnings
        .iter()
        .any(|warning| warning.correction().contains("correct each reported")));
}

#[test]
fn blackhole_exchange_globals_are_typed_and_invalid_intervals_are_actionable() {
    let source = "00112233445566778899aabbccddeeff";
    let report = parse_named(
        "/tmp/rns/config",
        &format!(
            "[reticulum]\npublish_blackhole = Yes\nblackhole_sources = {source}, {source}\nblackhole_update_interval = 2.5\n"
        ),
    )
    .unwrap();
    assert_eq!(report.value.blackhole_exchange.publish, Some(true));
    assert_eq!(report.value.blackhole_exchange.sources.len(), 1);
    assert_eq!(
        report.value.blackhole_exchange.update_interval_minutes,
        Some(2.5)
    );
    assert!(!report
        .warnings
        .iter()
        .any(|warning| { matches!(warning.code(), ConfigDiagnosticCode::UnsupportedSetting) }));

    let errors = parse_named(
        "/tmp/rns/config",
        "[reticulum]\nblackhole_update_interval = NaN\n",
    )
    .unwrap_err();
    let diagnostic = errors
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.path().ends_with("blackhole_update_interval"))
        .unwrap();
    assert_eq!(diagnostic.line(), 2);
    assert!(diagnostic.accepted().unwrap().contains("finite"));
    assert!(diagnostic.correction().contains("60.0"));
}

#[test]
fn nomadnet_page_policy_is_not_part_of_the_stock_reticulum_section() {
    let report = parse_named(
        "/tmp/rns/config",
        "[reticulum]\nannounce_node_page = No\nnode_page_announce_interval = 360\n",
    )
    .expect("unknown extension keys remain non-fatal");
    for key in ["announce_node_page", "node_page_announce_interval"] {
        assert!(report.warnings.iter().any(|warning| {
            warning.path().ends_with(key)
                && matches!(warning.code(), ConfigDiagnosticCode::UnknownKey)
        }));
    }
}

#[test]
fn malformed_remote_management_acl_fails_with_a_source_located_correction() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[reticulum]\nenable_remote_management = Yes\nremote_management_allowed = not-an-identity\n",
    )
    .unwrap_err();
    let diagnostic = errors
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.path().ends_with("remote_management_allowed"))
        .expect("remote-management ACL diagnostic");
    assert_eq!(diagnostic.code(), ConfigDiagnosticCode::InvalidValue);
    assert_eq!(diagnostic.source(), "/tmp/rns/config");
    assert_eq!(diagnostic.line(), 3);
    assert_eq!(diagnostic.value(), Some("not-an-identity"));
    assert!(diagnostic
        .accepted()
        .is_some_and(|accepted| accepted.contains("32-character hexadecimal")));
    assert!(diagnostic
        .correction()
        .contains("remote_management_allowed"));
}

#[test]
fn backbone_role_specific_settings_never_disappear() {
    let report = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Listener]]\ntype = BackboneInterface\nenabled = Yes\nlisten_port = 4242\ni2p_tunneled = Yes\nconnect_timeout = 8\nmax_reconnect_tries = 3\n[[Client]]\ntype = BackboneClientInterface\nenabled = Yes\ntarget_host = peer\ntarget_port = 4242\nlisten_ip = 0.0.0.0\ndevice = eth0\n",
    )
    .unwrap();
    let warnings = report
        .warnings
        .iter()
        .filter(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::IneffectiveSetting)
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 5);
    assert!(warnings
        .iter()
        .all(|warning| warning.message().contains("interface role")));
    assert!(warnings
        .iter()
        .any(|warning| warning.path().ends_with("i2p_tunneled")));
    assert!(warnings
        .iter()
        .any(|warning| warning.path().ends_with("device")));
}

#[test]
fn a_zero_announce_rate_target_earns_a_gentle_implicit_off_advisory() {
    let report = parse_named(
        "/tmp/rns/config",
        "[reticulum]\ndefault_ar_target = 0\n[interfaces]\n[[Quiet]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = host\ntarget_port = 4242\nannounce_rate_target = 0\n",
    )
    .unwrap();
    let advisories = report
        .warnings
        .iter()
        .filter(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::ImplicitOff)
        .collect::<Vec<_>>();
    assert_eq!(advisories.len(), 2);
    assert!(advisories
        .iter()
        .any(|advisory| advisory.path().ends_with("default_ar_target")));
    assert!(advisories
        .iter()
        .any(|advisory| advisory.path().ends_with("announce_rate_target")));
    assert!(advisories.iter().all(|advisory| {
        advisory
            .message()
            .contains("turns announce rate limiting off")
            && advisory.correction().contains("= off`")
    }));
}

#[test]
fn explicit_off_announce_rate_target_spellings_are_accepted_without_advisories() {
    for spelling in ["off", "NO", "False"] {
        let source = format!(
            "[reticulum]\ndefault_ar_target = {spelling}\n[interfaces]\n[[Quiet]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = host\ntarget_port = 4242\nannounce_rate_target = {spelling}\n"
        );
        let report = parse_named("/tmp/rns/config", &source).unwrap();
        assert!(report
            .warnings
            .iter()
            .all(|diagnostic| diagnostic.code() != ConfigDiagnosticCode::ImplicitOff));
        assert_eq!(
            report.value.interfaces[0].announce_rate_target,
            Some(ReferenceAnnounceRateTarget::Off)
        );
    }
}

#[test]
fn ifac_size_fails_when_its_floored_byte_count_exceeds_the_wire_limit() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Private]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = host\ntarget_port = 4242\nifac_size = 520\n",
    )
    .unwrap_err();
    let diagnostic = &errors.diagnostics()[0];
    assert_eq!(diagnostic.code(), ConfigDiagnosticCode::InvalidValue);
    assert_eq!(diagnostic.line(), 7);
    assert_eq!(diagnostic.value(), Some("520"));
    assert!(diagnostic
        .accepted()
        .is_some_and(|accepted| accepted.contains("519")));
    assert!(diagnostic.correction().contains("ifac_size = 64"));
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
fn an_rnode_ble_transport_is_supported_and_address_errors_are_focused() {
    let parsed = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = ble://RNode 1234\nfrequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n",
    )
    .expect("named RNode Bluetooth LE target parses");
    assert!(parsed.warnings.is_empty());

    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = ble://GG:BB:CC:DD:EE:FF\nfrequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n",
    )
    .unwrap_err();
    let diagnostic = errors
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.path().ends_with("port"))
        .unwrap();
    assert_eq!(diagnostic.code(), ConfigDiagnosticCode::InvalidValue);
    assert!(diagnostic.to_string().contains("six hexadecimal octets"));
    assert!(diagnostic
        .to_string()
        .contains("port = ble://AA:BB:CC:DD:EE:FF"));
}

#[test]
fn rnode_multi_rejects_uri_transports_as_one_serial_device_requirement() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Dual]]\ntype = RNodeMultiInterface\nenabled = Yes\nport = ble://RNode Multi\n[[[Sub-GHz]]]\ninterface_enabled = Yes\nvport = 0\nfrequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n",
    )
    .unwrap_err();
    let diagnostic = errors
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::UnsupportedTransport)
        .unwrap();
    assert!(diagnostic
        .to_string()
        .contains("RNodeMulti requires one local serial device"));
    let example = if cfg!(windows) {
        "COM3"
    } else {
        "/dev/ttyUSB0"
    };
    assert!(diagnostic
        .to_string()
        .contains(&format!("port = {example}")));
}

#[test]
fn an_rnode_tcp_uri_rejects_a_misleading_explicit_port() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = tcp://radio.example:7633\n",
    )
    .unwrap_err();
    let diagnostic = errors
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.path().ends_with("port"))
        .expect("port diagnostic");
    assert_eq!(diagnostic.code(), ConfigDiagnosticCode::InvalidValue);
    assert!(diagnostic
        .accepted()
        .is_some_and(|accepted| { accepted.contains("host without a port") }));
    assert!(diagnostic
        .correction()
        .contains("port = tcp://radio.example"));
}

#[test]
fn common_control_ranges_fail_with_a_concrete_correction() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[reticulum]\nic_burst_hold = -1\nic_new_time = 1e300\n[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = host\ntarget_port = 4242\nannounce_cap = 101\nec_pr_freq = 1e300\n",
    )
    .unwrap_err();
    assert_eq!(errors.len(), 4);
    let rendered = errors.to_string();
    assert!(rendered.contains("a non-negative duration in seconds"));
    assert!(rendered.contains("rounded milliseconds are below"));
    assert!(rendered.contains("ic_burst_hold = 15.0"));
    assert!(rendered.contains("rounded millihertz are below"));
    assert!(rendered.contains("ec_pr_freq = 5.0"));
    assert!(rendered.contains("a percentage from 0 through 100"));
    assert!(rendered.contains("announce_cap = 2.0"));
}

#[test]
fn serial_line_errors_are_aggregated_with_supported_forms() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Serial]]\ntype = SerialInterface\nenabled = Yes\nport = /dev/ttyUSB0\nspeed = 4\ndatabits = 9\nparity = mark\nstopbits = 3\n",
    )
    .unwrap_err();
    assert_eq!(errors.len(), 4);
    let rendered = errors.to_string();
    assert!(rendered.contains("[[Serial]] > speed"));
    assert!(rendered.contains("speed = 9600"));
    assert!(rendered.contains("one of 5, 6, 7, or 8"));
    assert!(rendered.contains("one of N, None, E, Even, O, or Odd"));
    assert!(rendered.contains("one of 1 or 2"));
}

#[test]
fn station_identification_must_be_complete() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[TNC]]\ntype = KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\nid_callsign = N0CALL\n",
    )
    .unwrap_err();
    assert_eq!(errors.len(), 1);
    let rendered = errors.to_string();
    assert!(rendered.contains("[[TNC]] > id_interval"));
    assert!(rendered.contains("id_interval = 600"));
    assert!(rendered.contains("or remove `id_callsign`"));
}

#[test]
fn rnode_ranges_and_callsign_capacity_fail_before_startup() {
    let errors = parse_named(
        "/tmp/rns/config",
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\nfrequency = 136999999\nbandwidth = 1625001\ntxpower = 38\nspreadingfactor = 13\ncodingrate = 9\nairtime_limit_short = -0.1\nairtime_limit_long = 100.1\nid_callsign = 123456789012345678901234567890123\nid_interval = 600\n",
    )
    .unwrap_err();
    assert_eq!(errors.len(), 8);
    let rendered = errors.to_string();
    assert!(rendered.contains("137000000 through 3000000000 hertz"));
    assert!(rendered.contains("7800 through 1625000 hertz"));
    assert!(rendered.contains("0 through 37 dBm"));
    assert!(rendered.contains("5 through 12"));
    assert!(rendered.contains("5 through 8"));
    assert!(rendered.contains("a percentage from 0 through 100"));
    assert!(rendered.contains("at most 32 UTF-8 bytes"));
}

#[test]
fn pipe_respawn_delay_rejects_negative_and_nonfinite_values() {
    for value in ["-1", "NaN", "inf"] {
        let config = format!(
            "[interfaces]\n[[Pipe]]\ntype = PipeInterface\nenabled = Yes\ncommand = worker\nrespawn_delay = {value}\n"
        );
        let errors = parse_named("/tmp/rns/config", &config).unwrap_err();
        let rendered = errors.to_string();
        assert!(rendered.contains("non-negative finite duration"));
        assert!(rendered.contains("respawn_delay = 5.0"));
    }
}

#[test]
fn pipe_command_rejects_empty_or_incomplete_quoting_before_startup() {
    for value in ["\"\"", "\"program 'unterminated\""] {
        let config = format!(
            "[interfaces]\n[[Pipe]]\ntype = PipeInterface\nenabled = Yes\ncommand = {value}\n"
        );
        let errors = parse_named("/tmp/rns/config", &config).unwrap_err();
        let rendered = errors.to_string();
        assert!(rendered.contains("non-empty command with complete shell-style quoting"));
        assert!(rendered.contains("command = /path/to/program --option value"));
    }
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
