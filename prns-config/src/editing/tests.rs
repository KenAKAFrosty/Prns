use std::fs;

use tempfile::tempdir;

use crate::configobj::ConfigDocument;
use crate::reference::keys::interface as interface_key;
use crate::{ConfigDiagnosticCode, InterfaceKind};

use super::{
    ConfigEdit, ConfigEditError, ConfigFile, ConfigFileError, ConfigRepairReport,
    InterfaceDefinition, InterfaceName, InterfaceSetting, InterfaceSettingChange,
    InterfaceSettingKey, InterfaceSettingValue, RNodeMultiRadioDefinition, SecretDisplay,
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
