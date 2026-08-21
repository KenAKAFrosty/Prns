use std::fs;

use tempfile::tempdir;

use crate::configobj::ConfigDocument;
use crate::reference::keys::interface as interface_key;
use crate::{parse_and_plan, ConfigDiagnosticCode, InterfaceKind};

use super::{
    ConfigEdit, ConfigEditError, ConfigFile, ConfigFileError, ConfigRepairReport,
    InterfaceDefinition, InterfaceName, InterfaceSetting, InterfaceSettingChange,
    InterfaceSettingInputError, InterfaceSettingInputKind, InterfaceSettingKey,
    InterfaceSettingValue, RNodeMultiRadioDefinition, SecretDisplay,
};

const BASE: &str = "[reticulum]\n    enable_transport = Yes\n[interfaces]\n  [[WiFi]]\n    type = AutoInterface\n    interface_enabled = Yes\n";

fn name(value: &str) -> InterfaceName {
    InterfaceName::new(value).unwrap_or_else(|error| panic!("{error}"))
}

fn key(value: &str) -> InterfaceSettingKey {
    InterfaceSettingKey::parse(value).unwrap_or_else(|| panic!("unknown setting key {value}"))
}

fn usb(name_value: &str) -> InterfaceDefinition {
    InterfaceDefinition::new(
        name(name_value),
        InterfaceKind::PrnsUsbAuto,
        true,
        Vec::new(),
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

fn radio(name_value: &str, vport: u8) -> RNodeMultiRadioDefinition {
    RNodeMultiRadioDefinition::new(name(name_value), vport, 868_000_000, 125_000, 7, 8, 5)
        .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn a_document_round_trips_every_source_byte() {
    let source = "# heading\r\n[interfaces]\r\n  [[\"Third Party\"]] # opaque\r\n    type = VendorInterface\r\n    interface_enabled = No\r\n    value = '''first\nsecond'''\r\n\r\n[plugin]\r\n  key = value\r\n";
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(document.source(), source);
    assert_eq!(document.newline(), "\r\n");
    assert_eq!(document.interfaces().len(), 1);
    assert_eq!(
        document.interfaces()[0].configured_type(),
        Some("VendorInterface")
    );
    assert_eq!(document.interfaces()[0].kind(), None);
    assert_eq!(document.interfaces()[0].enabled(), Some(false));
}

#[test]
fn adding_an_interface_preserves_every_existing_byte() {
    let source = format!("# retained\n{BASE}\n[custom]\nkey = value\n");
    let document = ConfigDocument::parse(&source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&ConfigEdit::Add(usb("USB Auto")))
        .unwrap_or_else(|error| panic!("{error}"));

    let added = "  [[USB Auto]]\n    type = PrnsUsbAuto\n    interface_enabled = Yes\n";
    assert_eq!(
        edited.candidate(),
        source.replace("\n[custom]", &format!("\n{added}[custom]"))
    );
}

#[test]
fn enabling_normalizes_the_stock_alias_without_touching_its_comment() {
    let source = "[interfaces]\n  [[USB]]\n    type = PrnsUsbAuto\n    enabled = no # keep\n";
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&ConfigEdit::SetEnabled {
            name: name("USB"),
            enabled: true,
        })
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(edited.candidate().contains("interface_enabled = Yes"));
    assert!(!edited.candidate().contains("enabled = no"));
}

#[test]
fn changing_a_value_retains_inline_comments_and_other_sections() {
    let source = "[interfaces]\n  [[Server]]\n    type = TCPServerInterface\n    interface_enabled = Yes\n    listen_port = 4242 # public\n[custom]\nvalue = untouched\n";
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let setting = InterfaceSetting::new(
        key(interface_key::LISTEN_PORT),
        InterfaceSettingValue::Unsigned(5252),
    );
    let edited = document
        .edit(&ConfigEdit::ChangeSettings {
            name: name("Server"),
            changes: vec![InterfaceSettingChange::Set(setting)],
        })
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(edited.candidate().contains("listen_port = 5252 # public"));
    assert!(edited
        .candidate()
        .ends_with("[custom]\nvalue = untouched\n"));
}

#[test]
fn a_mutation_cannot_write_an_invalid_candidate() {
    let source = "[interfaces]\n  [[Client]]\n    type = TCPClientInterface\n    interface_enabled = Yes\n    target_host = peer\n    target_port = 4242\n";
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let result = document.edit(&ConfigEdit::ChangeSettings {
        name: name("Client"),
        changes: vec![InterfaceSettingChange::Remove(key(
            interface_key::TARGET_HOST,
        ))],
    });

    assert!(matches!(result, Err(ConfigEditError::Invalid(_))));
}

#[test]
fn diffs_hide_secret_values_by_default() {
    let source = "[interfaces]\n  [[WiFi]]\n    type = AutoInterface\n    interface_enabled = Yes\n    pass_phrase = old-secret\n";
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&ConfigEdit::ChangeSettings {
            name: name("WiFi"),
            changes: vec![InterfaceSettingChange::Set(InterfaceSetting::new(
                key(interface_key::PASS_PHRASE),
                InterfaceSettingValue::Text("new-secret".to_string()),
            ))],
        })
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(!edited.diff(SecretDisplay::Redacted).contains("secret"));
    assert!(edited.diff(SecretDisplay::Revealed).contains("new-secret"));
}

#[test]
fn diffs_hide_multiline_secret_values() {
    let source = "[interfaces]\n  [[WiFi]]\n    type = AutoInterface\n    interface_enabled = Yes\n    pass_phrase = '''old private\ncontinued private'''\n";
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&ConfigEdit::ChangeSettings {
            name: name("WiFi"),
            changes: vec![InterfaceSettingChange::Set(InterfaceSetting::new(
                key(interface_key::PASS_PHRASE),
                InterfaceSettingValue::Text("new private".to_string()),
            ))],
        })
        .unwrap_or_else(|error| panic!("{error}"));
    let diff = edited.diff(SecretDisplay::Redacted);

    assert!(!diff.contains("old private"));
    assert!(!diff.contains("continued private"));
    assert!(!diff.contains("new private"));
}

#[test]
fn safe_repair_disables_an_invalid_interface() {
    let source = "[interfaces]\n  [[Broken]]\n    type = TCPClientInterface\n    interface_enabled = Yes\n    target_port = nope\n";
    let report = ConfigRepairReport::analyze(source).unwrap_or_else(|error| panic!("{error}"));
    assert!(report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::InvalidValue));
    let edit = report
        .safe_edit()
        .unwrap_or_else(|| panic!("missing safe edit"));
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&edit)
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(edited.candidate().contains("interface_enabled = No"));
}

#[test]
fn safe_repair_removes_only_the_redundant_alias() {
    let source = "[interfaces]\n  [[Server]]\n    type = TCPServerInterface\n    interface_enabled = Yes\n    port = 4242\n    listen_port = 4242\n";
    let report = ConfigRepairReport::analyze(source).unwrap_or_else(|error| panic!("{error}"));
    let edit = report
        .safe_edit()
        .unwrap_or_else(|| panic!("missing safe edit"));
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&edit)
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(edited.candidate().contains("    port = 4242\n"));
    assert!(!edited.candidate().contains("listen_port"));
}

#[test]
fn safe_repair_disables_only_the_duplicate_singleton() {
    let source = "[interfaces]\n  [[First]]\n    type = PrnsUsbAuto\n    interface_enabled = Yes\n  [[Second]]\n    type = prnsusbauto\n    interface_enabled = Yes\n";
    let report = ConfigRepairReport::analyze(source).unwrap_or_else(|error| panic!("{error}"));
    let edit = report
        .safe_edit()
        .unwrap_or_else(|| panic!("missing safe edit"));
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&edit)
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(edited
        .candidate()
        .contains("[[First]]\n    type = PrnsUsbAuto\n    interface_enabled = Yes"));
    assert!(edited
        .candidate()
        .contains("[[Second]]\n    type = prnsusbauto\n    interface_enabled = No"));
}

#[test]
fn persisted_rns_runtime_metadata_is_one_safe_cleanup() {
    let source = "[interfaces]\n  [[Default Interface]]\n    type = AutoInterface\n    interface_enabled = Yes\n    name = Default Interface\n    selected_interface_mode = 1\n    configured_bitrate = None\n";
    let report = ConfigRepairReport::analyze_named("/tmp/rns/config", source)
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        report
            .diagnostics()
            .iter()
            .filter(|diagnostic| {
                diagnostic.code() == ConfigDiagnosticCode::PersistedRuntimeMetadata
            })
            .count(),
        3
    );
    assert!(report
        .diagnostics()
        .iter()
        .all(|diagnostic| diagnostic.source() == "/tmp/rns/config"));
    let message_for = |key: &str| {
        report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.path().ends_with(key))
            .map(|diagnostic| diagnostic.message())
            .unwrap_or_else(|| panic!("missing diagnostic for {key}"))
    };
    assert!(message_for("name").contains("section heading"));
    assert!(message_for("selected_interface_mode").contains("\"mode\" setting"));
    assert!(message_for("configured_bitrate").contains("\"bitrate\" setting"));
    let edit = report
        .safe_edit()
        .unwrap_or_else(|| panic!("missing safe metadata cleanup"));
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit_named("/tmp/rns/config", &edit)
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(!edited.candidate().contains("    name ="));
    assert!(!edited.candidate().contains("selected_interface_mode"));
    assert!(!edited.candidate().contains("configured_bitrate"));
    assert!(edited.candidate().contains("type = AutoInterface"));
    assert!(
        crate::parse_and_plan_named("/tmp/rns/config", edited.candidate())
            .unwrap_or_else(|error| panic!("{error}"))
            .warnings
            .is_empty()
    );
}

#[test]
fn arbitrary_unknown_interface_values_remain_guided_only() {
    let source = "[interfaces]\n  [[WiFi]]\n    type = AutoInterface\n    interface_enabled = Yes\n    vendor_extension = retained\n";
    let report = ConfigRepairReport::analyze(source).unwrap_or_else(|error| panic!("{error}"));

    assert!(report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code() == ConfigDiagnosticCode::UnknownKey));
    assert!(report.safe_edit().is_none());
}

#[test]
fn setting_catalog_parses_typed_values_and_canonicalizes_aliases() {
    let mode = InterfaceKind::Auto
        .setting_specs()
        .into_iter()
        .find(|spec| spec.key().as_str() == "interface_mode")
        .unwrap_or_else(|| panic!("missing interface mode setting"));
    let parsed = mode
        .parse(InterfaceKind::Auto, "gateway")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        parsed.value(),
        &InterfaceSettingValue::Text("gateway".to_string())
    );

    let source = "[interfaces]\n  [[WiFi]]\n    type = AutoInterface\n    interface_enabled = Yes\n    mode = full\n";
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&ConfigEdit::ChangeSettings {
            name: name("WiFi"),
            changes: vec![InterfaceSettingChange::Set(parsed)],
        })
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(edited.candidate().contains("interface_mode = gateway"));
    assert!(!edited
        .candidate()
        .lines()
        .any(|line| line.trim_start().starts_with("mode =")));
}

#[test]
fn announce_rate_target_helper_accepts_numeric_and_explicit_off_values() {
    let target = InterfaceKind::Auto
        .setting_specs()
        .into_iter()
        .find(|spec| spec.key().as_str() == interface_key::ANNOUNCE_RATE_TARGET)
        .unwrap_or_else(|| panic!("missing announce rate target setting"));

    assert_eq!(
        target.input_kind(InterfaceKind::Auto),
        InterfaceSettingInputKind::Text
    );
    assert!(target
        .accepted(InterfaceKind::Auto)
        .contains("off, no, false"));
    for spelling in ["off", "NO", "False"] {
        let parsed = target
            .parse(InterfaceKind::Auto, spelling)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            parsed.value(),
            &InterfaceSettingValue::Text("off".to_string())
        );
        assert_eq!(target.format_value(spelling), "off");
    }
    assert_eq!(
        target
            .parse(InterfaceKind::Auto, "3_600")
            .unwrap_or_else(|error| panic!("{error}"))
            .value(),
        &InterfaceSettingValue::Unsigned(3_600)
    );
    assert_eq!(
        target.parse(InterfaceKind::Auto, "disabled"),
        Err(InterfaceSettingInputError::AnnounceRateTarget)
    );

    let setting = target
        .parse(InterfaceKind::Auto, "false")
        .unwrap_or_else(|error| panic!("{error}"));
    let document = ConfigDocument::parse(BASE).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&ConfigEdit::ChangeSettings {
            name: name("WiFi"),
            changes: vec![InterfaceSettingChange::Set(setting)],
        })
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(edited.candidate().contains("announce_rate_target = off"));
    let plan = parse_and_plan(edited.candidate()).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(plan.value.interfaces[0].policy.announce_rate_limit, None);
}

#[test]
fn auto_setting_catalog_exposes_runtime_relevant_values_and_effective_defaults() {
    let specs = InterfaceKind::Auto.supported_setting_specs();
    assert!(specs.iter().any(|spec| spec.key().as_str() == "data_port"));
    assert!(!specs
        .iter()
        .any(|spec| spec.key().as_str() == "discoverable"));
    assert!(!InterfaceKind::Auto.supports_editing_setting(key("ignore_config_warnings")));

    let report = parse_and_plan(BASE).unwrap_or_else(|error| panic!("{error}"));
    let planned = report
        .value
        .interfaces
        .first()
        .unwrap_or_else(|| panic!("missing AutoInterface plan"));
    let data_port = specs
        .iter()
        .find(|spec| spec.key().as_str() == "data_port")
        .unwrap_or_else(|| panic!("missing data port setting"));
    let bitrate = specs
        .iter()
        .find(|spec| spec.key().as_str() == "bitrate")
        .unwrap_or_else(|| panic!("missing bitrate setting"));
    let gravity = specs
        .iter()
        .find(|spec| spec.key().as_str() == "gravity")
        .unwrap_or_else(|| panic!("missing gravity setting"));
    let outgoing = specs
        .iter()
        .find(|spec| spec.key().as_str() == "outgoing")
        .unwrap_or_else(|| panic!("missing outgoing setting"));

    assert_eq!(data_port.effective_value(planned).as_deref(), Some("42671"));
    assert_eq!(
        bitrate.effective_value(planned).as_deref(),
        Some("1000000000")
    );
    assert_eq!(bitrate.format_value("1000000000"), "1 Gbps");
    assert_eq!(gravity.effective_value(planned).as_deref(), Some("0"));
    assert!(matches!(
        gravity.parse(InterfaceKind::Auto, "-42").unwrap().value(),
        InterfaceSettingValue::Signed(-42)
    ));
    assert_eq!(outgoing.label(), "Outgoing traffic allowed");
    assert!(data_port.description().contains("packet traffic"));
}

#[test]
fn websocket_setting_catalog_exposes_automatic_framing_as_the_optional_default() {
    let specs = InterfaceKind::PrnsWebSocketClient.supported_setting_specs();
    let framing = specs
        .iter()
        .find(|spec| spec.key().as_str() == interface_key::FRAMING)
        .unwrap_or_else(|| panic!("missing WebSocket framing setting"));
    let report = parse_and_plan(
        "[interfaces]\n[[WebSocket]]\ntype = PrnsWebSocketClient\nenabled = Yes\ntarget = ws://peer.example/prns\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let planned = report
        .value
        .interfaces
        .first()
        .unwrap_or_else(|| panic!("missing WebSocket plan"));

    assert_eq!(
        framing.required_hint(InterfaceKind::PrnsWebSocketClient),
        None
    );
    assert_eq!(framing.effective_value(planned).as_deref(), Some("auto"));
    assert_eq!(
        framing.accepted(InterfaceKind::PrnsWebSocketClient),
        "auto, raw, hdlc, or kiss"
    );
}

#[test]
fn technical_quantities_use_natural_units_without_changing_stored_values() {
    let formatted = |kind: InterfaceKind, name: &str, value: &str| {
        kind.setting_specs()
            .into_iter()
            .find(|spec| spec.key() == key(name))
            .unwrap_or_else(|| panic!("missing {name} setting for {kind:?}"))
            .format_value(value)
    };

    assert_eq!(
        formatted(InterfaceKind::Rnode, "frequency", "915000000"),
        "915 MHz"
    );
    assert_eq!(
        formatted(InterfaceKind::Kiss, "discovery_frequency", "868100000"),
        "868.1 MHz"
    );
    assert_eq!(
        formatted(InterfaceKind::Rnode, "bandwidth", "125000"),
        "125 kHz"
    );
    assert_eq!(
        formatted(InterfaceKind::Auto, "bitrate", "500000000"),
        "500 Mbps"
    );
    assert_eq!(
        formatted(InterfaceKind::Serial, "speed", "9600"),
        "9.6 kbps"
    );
    assert_eq!(
        formatted(InterfaceKind::TcpClient, "fixed_mtu", "524288"),
        "524,288 bytes"
    );
}

#[test]
fn discovery_editing_support_matches_the_runtime_advertisement_matrix() {
    let discoverable = key("discoverable");
    let reachable_on = key("reachable_on");
    let frequency = key("discovery_frequency");

    for kind in [
        InterfaceKind::TcpClient,
        InterfaceKind::TcpServer,
        InterfaceKind::Kiss,
        InterfaceKind::Rnode,
        InterfaceKind::RnodeMulti,
        InterfaceKind::Backbone,
    ] {
        assert!(kind.supports_editing_setting(discoverable), "{kind:?}");
    }
    for kind in [
        InterfaceKind::Auto,
        InterfaceKind::Udp,
        InterfaceKind::Serial,
        InterfaceKind::Ax25Kiss,
        InterfaceKind::Pipe,
        InterfaceKind::BackboneClient,
        InterfaceKind::I2p,
        InterfaceKind::Weave,
        InterfaceKind::PrnsUsbAuto,
        InterfaceKind::PrnsBluetoothAuto,
        InterfaceKind::PrnsWebSocketClient,
        InterfaceKind::PrnsWebSocketServer,
    ] {
        assert!(!kind.supports_editing_setting(discoverable), "{kind:?}");
    }
    assert!(InterfaceKind::TcpServer.supports_editing_setting(reachable_on));
    assert!(InterfaceKind::Backbone.supports_editing_setting(reachable_on));
    assert!(!InterfaceKind::Rnode.supports_editing_setting(reachable_on));
    assert!(InterfaceKind::TcpClient.supports_editing_setting(frequency));
    assert!(InterfaceKind::Kiss.supports_editing_setting(frequency));
    assert!(!InterfaceKind::Rnode.supports_editing_setting(frequency));
}

#[test]
fn replacing_rnode_multi_radios_preserves_parent_settings_and_siblings() {
    let source = "[interfaces]\n  [[Multi]]\n    type = RNodeMultiInterface\n    interface_enabled = Yes\n    port = /dev/ttyACM0 # retained\n    [[[Old]]]\n      interface_enabled = Yes\n      vport = 0\n      frequency = 868000000\n      bandwidth = 125000\n      txpower = 7\n      spreadingfactor = 8\n      codingrate = 5\n  [[USB]]\n    type = PrnsUsbAuto\n    interface_enabled = Yes\n";
    let document = ConfigDocument::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let edited = document
        .edit(&ConfigEdit::ReplaceRNodeMultiRadios {
            name: name("Multi"),
            radios: vec![radio("Primary", 1)],
        })
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(edited
        .candidate()
        .contains("port = /dev/ttyACM0 # retained"));
    assert!(edited.candidate().contains("[[[Primary]]]"));
    assert!(edited.candidate().contains("vport = 1"));
    assert!(!edited.candidate().contains("[[[Old]]]"));
    assert!(edited.candidate().contains("[[USB]]"));
}

#[test]
fn writes_are_atomic_backed_up_and_permission_preserving() {
    let directory = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("config");
    fs::write(&path, BASE).unwrap_or_else(|error| panic!("{error}"));
    let file = ConfigFile::load(&path, "").unwrap_or_else(|error| panic!("{error}"));
    let edited = file
        .document()
        .edit(&ConfigEdit::Add(usb("USB")))
        .unwrap_or_else(|error| panic!("{error}"));
    let receipt = file
        .write(&edited)
        .unwrap_or_else(|error| panic!("{error}"));

    let backup = receipt.backup().unwrap_or_else(|| panic!("missing backup"));
    assert_eq!(
        fs::read_to_string(backup).unwrap_or_else(|error| panic!("{error}")),
        BASE
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{error}")),
        edited.candidate()
    );
}

#[test]
fn a_write_receipt_can_atomically_restore_the_previous_configuration() {
    let directory = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("config");
    fs::write(&path, BASE).unwrap_or_else(|error| panic!("{error}"));
    let file = ConfigFile::load(&path, "").unwrap_or_else(|error| panic!("{error}"));
    let edited = file
        .document()
        .edit(&ConfigEdit::Add(usb("USB")))
        .unwrap_or_else(|error| panic!("{error}"));
    let receipt = file
        .write(&edited)
        .unwrap_or_else(|error| panic!("{error}"));

    receipt.rollback().unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{error}")),
        BASE
    );
}

#[test]
fn rollback_refuses_to_overwrite_a_concurrent_configuration_change() {
    let directory = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("config");
    fs::write(&path, BASE).unwrap_or_else(|error| panic!("{error}"));
    let file = ConfigFile::load(&path, "").unwrap_or_else(|error| panic!("{error}"));
    let edited = file
        .document()
        .edit(&ConfigEdit::Add(usb("USB")))
        .unwrap_or_else(|error| panic!("{error}"));
    let receipt = file
        .write(&edited)
        .unwrap_or_else(|error| panic!("{error}"));
    let competing = format!("{BASE}# competing edit\n");
    fs::write(&path, &competing).unwrap_or_else(|error| panic!("{error}"));

    assert!(matches!(
        receipt.rollback(),
        Err(ConfigFileError::ConcurrentModification)
    ));
    assert_eq!(
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{error}")),
        competing
    );
}

#[test]
fn stale_sources_are_rejected_without_overwriting_either_version() {
    let directory = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("config");
    fs::write(&path, BASE).unwrap_or_else(|error| panic!("{error}"));
    let file = ConfigFile::load(&path, "").unwrap_or_else(|error| panic!("{error}"));
    let edited = file
        .document()
        .edit(&ConfigEdit::Add(usb("USB")))
        .unwrap_or_else(|error| panic!("{error}"));
    let competing = format!("{BASE}# competing edit\n");
    fs::write(&path, &competing).unwrap_or_else(|error| panic!("{error}"));

    assert!(matches!(
        file.write(&edited),
        Err(ConfigFileError::ConcurrentModification)
    ));
    assert_eq!(
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{error}")),
        competing
    );
}

#[test]
fn editing_a_missing_installation_materializes_the_fallback() {
    let directory = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("config");
    let file = ConfigFile::load(&path, BASE).unwrap_or_else(|error| panic!("{error}"));
    assert!(!file.is_materialized());
    let edited = file
        .document()
        .edit(&ConfigEdit::Add(usb("USB")))
        .unwrap_or_else(|error| panic!("{error}"));
    let receipt = file
        .write(&edited)
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(receipt.created());
    assert_eq!(receipt.backup(), None);
    assert_eq!(
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{error}")),
        edited.candidate()
    );
}

#[test]
fn rollback_removes_a_configuration_created_by_the_write() {
    let directory = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("config");
    let file = ConfigFile::load(&path, BASE).unwrap_or_else(|error| panic!("{error}"));
    let edited = file
        .document()
        .edit(&ConfigEdit::Add(usb("USB")))
        .unwrap_or_else(|error| panic!("{error}"));
    let receipt = file
        .write(&edited)
        .unwrap_or_else(|error| panic!("{error}"));

    receipt.rollback().unwrap_or_else(|error| panic!("{error}"));

    assert!(!path.exists());
}

#[cfg(unix)]
#[test]
fn new_configuration_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("config");
    let file = ConfigFile::load(&path, BASE).unwrap_or_else(|error| panic!("{error}"));
    let edited = file
        .document()
        .edit(&ConfigEdit::Add(usb("USB")))
        .unwrap_or_else(|error| panic!("{error}"));
    file.write(&edited)
        .unwrap_or_else(|error| panic!("{error}"));

    let mode = fs::metadata(path)
        .unwrap_or_else(|error| panic!("{error}"))
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}
