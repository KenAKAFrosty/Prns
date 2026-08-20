use crate::configobj::{Section, SourceLocations, Value};
use crate::diagnostic::{ConfigDiagnostic, ConfigDiagnosticCode, ConfigFix, ConfigFixSafety};
use prns_core::interfaces::rnode::protocol::{
    BANDWIDTH_HZ_MAX, BANDWIDTH_HZ_MIN, CODING_RATE_MAX, CODING_RATE_MIN, FREQUENCY_HZ_MAX,
    FREQUENCY_HZ_MIN, SPREADING_FACTOR_MAX, SPREADING_FACTOR_MIN, TXPOWER_DBM_MAX, TXPOWER_DBM_MIN,
};
use prns_core::interfaces::IFAC_MAX_SIZE;

use super::diagnostics::{ErrorCode, ErrorDiagnostic, WarningCode, WarningDiagnostic};
use super::i2p::validate_peers;
use super::interface_type::InterfaceType;
use super::interpret::{
    cleaned_number, parse_bool, parse_byte_quantity, parse_identity_hash, ReferenceError,
};
use super::keys::rnode as rnode_key;
use super::keys::{
    common as common_key, global as global_key, interface as interface_key, section as section_key,
};
use super::schema::{
    interface_key_rule, known_interface_keys, KeyApplication, KeyRule, ValueKind, GLOBAL_RULES,
    LOGGING_RULES, PRNS_RULES, SUPPORTED_INTERFACES,
};

#[derive(Default)]
pub(super) struct ValidationWarnings(Vec<ConfigDiagnostic>);

impl ValidationWarnings {
    pub(super) fn push(&mut self, diagnostic: WarningDiagnostic) {
        self.0.push(diagnostic.into_inner());
    }

    pub(super) fn into_inner(self) -> Vec<ConfigDiagnostic> {
        self.0
    }
}

#[derive(Default)]
pub(super) struct ValidationErrorCollector(Vec<ConfigDiagnostic>);

impl ValidationErrorCollector {
    pub(super) fn push(&mut self, diagnostic: ErrorDiagnostic) {
        self.0.push(diagnostic.into_inner());
    }

    fn finish(self) -> Option<ValidationErrors> {
        let mut diagnostics = self.0.into_iter();
        let first = diagnostics.next()?;
        Some(ValidationErrors {
            first,
            remaining: diagnostics.collect(),
        })
    }
}

pub(super) struct ValidationErrors {
    first: ConfigDiagnostic,
    remaining: Vec<ConfigDiagnostic>,
}

impl ValidationErrors {
    pub(super) fn with_warnings(self, warnings: ValidationWarnings) -> Vec<ConfigDiagnostic> {
        let mut diagnostics = Vec::with_capacity(1 + self.remaining.len() + warnings.0.len());
        diagnostics.push(self.first);
        diagnostics.extend(self.remaining);
        diagnostics.extend(warnings.0);
        diagnostics
    }
}

pub(super) enum ValidationResult {
    Valid {
        warnings: ValidationWarnings,
    },
    Invalid {
        errors: Box<ValidationErrors>,
        warnings: ValidationWarnings,
    },
}

pub(super) fn validate(
    source: &str,
    root: &Section,
    locations: &SourceLocations,
) -> ValidationResult {
    let mut warnings = ValidationWarnings::default();
    let mut errors = ValidationErrorCollector::default();
    let global_keys = GLOBAL_RULES.iter().map(|(key, _)| *key).collect::<Vec<_>>();
    let logging_keys = LOGGING_RULES
        .iter()
        .map(|(key, _)| *key)
        .collect::<Vec<_>>();
    let prns_keys = PRNS_RULES.iter().map(|(key, _)| *key).collect::<Vec<_>>();

    for (key, value) in &root.scalars {
        let line = location(locations, &[key]);
        if global_keys.contains(&key.as_str()) {
            errors.push(ErrorDiagnostic::new(
                ErrorCode::MisplacedKey,
                source,
                line,
                format!("<root> > {key}"),
                Some(value_text(value)),
                format!("global setting {key:?} is outside [reticulum] and will not be applied"),
                Some(format!("{key} must be under [reticulum]")),
                format!("move `{key} = {}` into [reticulum]", value_text(value)),
            ));
        } else if logging_keys.contains(&key.as_str()) {
            errors.push(ErrorDiagnostic::new(
                ErrorCode::MisplacedKey,
                source,
                line,
                format!("<root> > {key}"),
                Some(value_text(value)),
                format!("logging setting {key:?} is outside [logging] and will not be applied"),
                Some(format!("{key} must be under [logging]")),
                format!("move `{key} = {}` into [logging]", value_text(value)),
            ));
        } else if prns_keys.contains(&key.as_str()) {
            errors.push(ErrorDiagnostic::new(
                ErrorCode::MisplacedKey,
                source,
                line,
                format!("<root> > {key}"),
                Some(value_text(value)),
                format!("Prns setting {key:?} is outside [prns] and will not be applied"),
                Some(format!("{key} must be under [prns]")),
                format!("move `{key} = {}` into [prns]", value_text(value)),
            ));
        } else {
            warnings.push(unknown_key(
                source,
                line,
                format!("<root> > {key}"),
                key,
                value,
                &[],
            ));
        }
    }

    for (name, section) in &root.sections {
        match name.as_str() {
            section_key::RETICULUM => {
                validate_section(
                    source,
                    "[reticulum]",
                    &[section_key::RETICULUM],
                    section,
                    GLOBAL_RULES,
                    locations,
                    &mut warnings,
                    &mut errors,
                );
                advise_zero_announce_rate_target(
                    source,
                    "[reticulum]",
                    &[section_key::RETICULUM],
                    global_key::DEFAULT_AR_TARGET,
                    section,
                    locations,
                    &mut warnings,
                );
            }
            section_key::LOGGING => validate_section(
                source,
                "[logging]",
                &[section_key::LOGGING],
                section,
                LOGGING_RULES,
                locations,
                &mut warnings,
                &mut errors,
            ),
            section_key::PRNS => validate_section(
                source,
                "[prns]",
                &[section_key::PRNS],
                section,
                PRNS_RULES,
                locations,
                &mut warnings,
                &mut errors,
            ),
            section_key::INTERFACES => {
                validate_interfaces(source, section, locations, &mut warnings, &mut errors)
            }
            _ => {
                let known = [
                    section_key::RETICULUM,
                    section_key::PRNS,
                    section_key::LOGGING,
                    section_key::INTERFACES,
                ];
                let suggestion = closest(name, &known);
                warnings.push(WarningDiagnostic::new(
                    WarningCode::UnknownSection,
                    source,
                    location(locations, &[name]),
                    format!("[{name}]"),
                    Some(name.clone()),
                    "unknown top-level section; its settings will not be applied",
                    Some("[reticulum], [prns], [logging], or [interfaces]".to_string()),
                    suggestion.map_or_else(
                        || {
                            format!(
                                "remove [{name}] or move its settings into a recognized section"
                            )
                        },
                        |expected| format!("rename [{name}] to [{expected}]"),
                    ),
                ));
            }
        }
    }

    match errors.finish() {
        Some(errors) => ValidationResult::Invalid {
            errors: Box::new(errors),
            warnings,
        },
        None => ValidationResult::Valid { warnings },
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_section(
    source: &str,
    display_path: &str,
    source_path: &[&str],
    section: &Section,
    rules: &[(&str, KeyRule)],
    locations: &SourceLocations,
    warnings: &mut ValidationWarnings,
    errors: &mut ValidationErrorCollector,
) {
    let known = rules.iter().map(|(key, _)| *key).collect::<Vec<_>>();
    for (key, value) in &section.scalars {
        let mut key_path = source_path.to_vec();
        key_path.push(key);
        let line = location(locations, &key_path);
        match rules.iter().find(|(known, _)| *known == key) {
            Some((_, rule)) => {
                if let Some(kind) = rule.validation_kind(true) {
                    validate_value(
                        source,
                        line,
                        format!("{display_path} > {key}"),
                        key,
                        value,
                        kind,
                        errors,
                    );
                }
                warn_for_application(
                    source,
                    display_path,
                    source_path,
                    key,
                    value,
                    rule.application(),
                    true,
                    locations,
                    warnings,
                );
            }
            None => warnings.push(unknown_key(
                source,
                line,
                format!("{display_path} > {key}"),
                key,
                value,
                &known,
            )),
        }
    }

    for (name, _) in &section.sections {
        let mut section_path = source_path.to_vec();
        section_path.push(name);
        warnings.push(WarningDiagnostic::new(
            WarningCode::UnknownSection,
            source,
            location(locations, &section_path),
            format!("{display_path} > [[{name}]]"),
            Some(name.clone()),
            "nested sections are not valid here and will not be applied",
            None,
            format!("remove [[{name}]] or move its keys directly under {display_path}"),
        ));
    }
}

fn validate_interfaces(
    source: &str,
    interfaces: &Section,
    locations: &SourceLocations,
    warnings: &mut ValidationWarnings,
    errors: &mut ValidationErrorCollector,
) {
    let mut usb_auto = None;
    let mut bluetooth_auto = None;
    for (key, value) in &interfaces.scalars {
        warnings.push(unknown_key(
            source,
            location(locations, &[section_key::INTERFACES, key]),
            format!("[interfaces] > {key}"),
            key,
            value,
            &[],
        ));
    }
    for (name, section) in &interfaces.sections {
        validate_interface(source, name, section, locations, warnings, errors);
        match enabled_interface_type(section) {
            Some(InterfaceType::PrnsUsbAuto) => reject_duplicate_singleton_interface(
                source,
                name,
                section,
                locations,
                &mut usb_auto,
                errors,
            ),
            Some(InterfaceType::PrnsBluetoothAuto) => reject_duplicate_singleton_interface(
                source,
                name,
                section,
                locations,
                &mut bluetooth_auto,
                errors,
            ),
            _ => {}
        }
    }
}

fn enabled_interface_type(section: &Section) -> Option<InterfaceType> {
    let mut enabled_values = interface_key::ENABLED_ALIASES
        .iter()
        .filter_map(|key| section.get(key).and_then(Value::as_scalar));
    let first = enabled_values.next()?;
    if parse_bool(first) != Some(true)
        || enabled_values.any(|value| parse_bool(value) != Some(true))
    {
        return None;
    }
    section
        .get(interface_key::TYPE)
        .and_then(Value::as_scalar)
        .and_then(InterfaceType::parse)
}

fn reject_duplicate_singleton_interface<'a>(
    source: &str,
    name: &'a str,
    section: &Section,
    locations: &SourceLocations,
    first: &mut Option<&'a str>,
    errors: &mut ValidationErrorCollector,
) {
    let Some(previous) = first.replace(name) else {
        return;
    };
    let configured_type = section
        .get(interface_key::TYPE)
        .and_then(Value::as_scalar)
        .unwrap_or_default();
    errors.push(ErrorDiagnostic::new(
        ErrorCode::InvalidValue,
        source,
        location(
            locations,
            &[section_key::INTERFACES, name, interface_key::TYPE],
        ),
        format!("[interfaces] > [[{name}]] > {}", interface_key::TYPE),
        Some(configured_type.to_string()),
        format!("[[{name}]] duplicates the singleton interface [[{previous}]]"),
        Some("one enabled stanza for this auto-interface family".to_string()),
        format!("set `enabled = No` under [[{name}]], or merge its settings into [[{previous}]]"),
    ));
}

fn advise_zero_announce_rate_target(
    source: &str,
    display_path: &str,
    source_path: &[&str],
    key: &str,
    section: &Section,
    locations: &SourceLocations,
    warnings: &mut ValidationWarnings,
) {
    let Some(value) = section.get(key) else {
        return;
    };
    let is_zero = value
        .as_scalar()
        .and_then(|text| parse_integer::<u64>(text).ok())
        == Some(0);
    if !is_zero {
        return;
    }
    let mut key_path = source_path.to_vec();
    key_path.push(key);
    warnings.push(WarningDiagnostic::new(
        WarningCode::ImplicitOff,
        source,
        location(locations, &key_path),
        format!("{display_path} > {key}"),
        Some(value_text(value)),
        format!("setting {key:?} = 0 turns announce rate limiting off"),
        Some("off, or a positive integer number of seconds".to_string()),
        format!("if disabling is intended, prefer `{key} = off`"),
    ));
}

#[allow(clippy::too_many_arguments)]
fn warn_non_effective_settings(
    source: &str,
    display_path: &str,
    source_path: &[&str],
    section: &Section,
    keys: &[&str],
    locations: &SourceLocations,
    warnings: &mut ValidationWarnings,
    reason: SettingWarningKind,
) {
    for key in keys {
        let Some(value) = section.get(key) else {
            continue;
        };
        warn_non_effective_setting(
            source,
            display_path,
            source_path,
            key,
            value,
            locations,
            warnings,
            reason,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn warn_non_effective_setting(
    source: &str,
    display_path: &str,
    source_path: &[&str],
    key: &str,
    value: &Value,
    locations: &SourceLocations,
    warnings: &mut ValidationWarnings,
    reason: SettingWarningKind,
) {
    let mut key_path = source_path.to_vec();
    key_path.push(key);
    let (code, message, accepted, correction) = match reason {
        SettingWarningKind::FollowOn if key == interface_key::IGNORE_CONFIG_WARNINGS => (
            WarningCode::UnsupportedSetting,
            format!("stock RNS setting {key:?} is not applied by this build"),
            "omit this setting".to_string(),
            format!("remove `{key}` and correct each reported configuration problem"),
        ),
        SettingWarningKind::FollowOn => (
            WarningCode::UnsupportedSetting,
            format!("stock RNS setting {key:?} is not applied by this build"),
            "omit this setting or use a build that implements it".to_string(),
            format!(
                "remove `{key} = {}` until this feature is available",
                value_text(value)
            ),
        ),
        SettingWarningKind::InapplicableInterfaceRole => (
            WarningCode::IneffectiveSetting,
            format!("setting {key:?} is not applied by this interface role"),
            "omit this setting for the selected listener or client role".to_string(),
            format!("remove `{key} = {}` from this stanza", value_text(value)),
        ),
        SettingWarningKind::DiscoveryDisabled => (
            WarningCode::IneffectiveSetting,
            format!("setting {key:?} is not applied while discovery publication is disabled"),
            format!(
                "omit this setting or set {} = Yes",
                interface_key::DISCOVERABLE
            ),
            format!(
                "remove `{key} = {}`, or set `{}` = Yes",
                value_text(value),
                interface_key::DISCOVERABLE
            ),
        ),
    };
    warnings.push(WarningDiagnostic::new(
        code,
        source,
        location(locations, &key_path),
        format!("{display_path} > {key}"),
        Some(value_text(value)),
        message,
        Some(accepted),
        correction,
    ));
}

#[allow(clippy::too_many_arguments)]
fn warn_for_application(
    source: &str,
    display_path: &str,
    source_path: &[&str],
    key: &str,
    value: &Value,
    application: KeyApplication,
    discovery_enabled: bool,
    locations: &SourceLocations,
    warnings: &mut ValidationWarnings,
) {
    let reason = match application {
        KeyApplication::Applied => return,
        KeyApplication::FollowOn => SettingWarningKind::FollowOn,
        KeyApplication::DiscoveryOnly if discovery_enabled => return,
        KeyApplication::DiscoveryOnly => SettingWarningKind::DiscoveryDisabled,
    };
    warn_non_effective_setting(
        source,
        display_path,
        source_path,
        key,
        value,
        locations,
        warnings,
        reason,
    );
}

#[derive(Clone, Copy)]
enum SettingWarningKind {
    FollowOn,
    InapplicableInterfaceRole,
    DiscoveryDisabled,
}

fn validate_interface(
    source: &str,
    name: &str,
    section: &Section,
    locations: &SourceLocations,
    warnings: &mut ValidationWarnings,
    errors: &mut ValidationErrorCollector,
) {
    let interface_path = format!("[interfaces] > [[{name}]]");
    let interface_source_path = [section_key::INTERFACES, name];
    let type_key = interface_key::TYPE;
    let enabled_key = interface_key::ENABLED;
    let enabled = validate_alias_group(
        source,
        &interface_source_path,
        &interface_path,
        section,
        locations,
        interface_key::INTERFACE_ENABLED,
        interface_key::ENABLED_ALIASES,
        ValueKind::Bool,
        warnings,
        errors,
    );
    if enabled.as_deref() != Some("true") {
        return;
    }
    advise_zero_announce_rate_target(
        source,
        &interface_path,
        &interface_source_path,
        interface_key::ANNOUNCE_RATE_TARGET,
        section,
        locations,
        warnings,
    );

    let type_value = section.get(interface_key::TYPE);
    let Some(type_name) = type_value
        .and_then(Value::as_scalar)
        .filter(|name| !name.is_empty())
    else {
        errors.push(ErrorDiagnostic::new(
            ErrorCode::MissingRequiredKey,
            source,
            location(locations, &[section_key::INTERFACES, name]),
            format!("{interface_path} > {type_key}"),
            type_value.map(value_text),
            format!("enabled interface is missing its required {type_key}"),
            Some(SUPPORTED_INTERFACES.join(", ")),
            format!(
                "add `{type_key} = AutoInterface` under [[{name}]], or set `{enabled_key} = No`"
            ),
        ));
        return;
    };

    let Some(interface_type) = InterfaceType::parse(type_name) else {
        errors.push(ErrorDiagnostic::new(
            ErrorCode::UnsupportedInterface,
            source,
            location(locations, &[section_key::INTERFACES, name, type_key]),
            format!("{interface_path} > {type_key}"),
            Some(type_name.to_string()),
            format!("interface type {type_name:?} is not available in this build"),
            Some(SUPPORTED_INTERFACES.join(", ")),
            format!(
                "set `{enabled_key} = No` for [[{name}]] until {type_name} support is installed"
            ),
        ));
        return;
    };
    let type_name = interface_type.canonical_name();

    validate_alias_group(
        source,
        &interface_source_path,
        &interface_path,
        section,
        locations,
        interface_key::INTERFACE_MODE,
        interface_key::MODE_ALIASES,
        ValueKind::Mode,
        warnings,
        errors,
    );
    validate_alias_group(
        source,
        &interface_source_path,
        &interface_path,
        section,
        locations,
        interface_key::NETWORK_NAME,
        interface_key::NETWORK_NAME_ALIASES,
        ValueKind::String,
        warnings,
        errors,
    );
    validate_alias_group(
        source,
        &interface_source_path,
        &interface_path,
        section,
        locations,
        interface_key::PASS_PHRASE,
        interface_key::PASSPHRASE_ALIASES,
        ValueKind::String,
        warnings,
        errors,
    );

    let discoverable = section
        .get(interface_key::DISCOVERABLE)
        .and_then(Value::as_scalar)
        .and_then(parse_bool)
        == Some(true);
    let known = known_interface_keys(type_name);
    for (key, value) in &section.scalars {
        if interface_key::ALIASES.contains(&key.as_str()) {
            continue;
        }
        let line = location(locations, &[section_key::INTERFACES, name, key]);
        match interface_key_rule(type_name, key) {
            Some(rule) => {
                if let Some(kind) = rule.validation_kind(discoverable) {
                    validate_value(
                        source,
                        line,
                        format!("{interface_path} > {key}"),
                        key,
                        value,
                        kind,
                        errors,
                    );
                }
                warn_for_application(
                    source,
                    &interface_path,
                    &interface_source_path,
                    key,
                    value,
                    rule.application(),
                    discoverable,
                    locations,
                    warnings,
                );
            }
            None => warnings.push(unknown_interface_key(
                source,
                line,
                format!("{interface_path} > {key}"),
                key,
                value,
                &known,
            )),
        }
    }

    if matches!(type_name, "BackboneInterface" | "BackboneClientInterface") {
        let client_role = type_name == "BackboneClientInterface"
            || has_one_of(
                section,
                &[interface_key::TARGET_HOST, interface_key::REMOTE],
            );
        let inapplicable = if client_role {
            &[
                interface_key::LISTEN_IP,
                interface_key::LISTEN_PORT,
                interface_key::LISTEN_ON,
                interface_key::DEVICE,
                interface_key::REACHABLE_PORT,
            ][..]
        } else {
            &[
                interface_key::TARGET_PORT,
                interface_key::I2P_TUNNELED,
                interface_key::CONNECT_TIMEOUT,
                interface_key::MAX_RECONNECT_TRIES,
            ][..]
        };
        warn_non_effective_settings(
            source,
            &interface_path,
            &[section_key::INTERFACES, name],
            section,
            inapplicable,
            locations,
            warnings,
            SettingWarningKind::InapplicableInterfaceRole,
        );
    }

    if !matches!(type_name, "BackboneInterface" | "TCPServerInterface") {
        warn_non_effective_settings(
            source,
            &interface_path,
            &[section_key::INTERFACES, name],
            section,
            &[interface_key::REACHABLE_PORT],
            locations,
            warnings,
            SettingWarningKind::InapplicableInterfaceRole,
        );
    }

    match type_name {
        "TCPServerInterface" => compare_alias_pair(
            source,
            name,
            section,
            locations,
            interface_key::PORT,
            interface_key::LISTEN_PORT,
            ValueKind::U16,
            warnings,
            errors,
        ),
        "PrnsWebSocketServer" => compare_alias_pair(
            source,
            name,
            section,
            locations,
            interface_key::PORT,
            interface_key::LISTEN_PORT,
            ValueKind::U16,
            warnings,
            errors,
        ),
        "UDPInterface" => {
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                interface_key::PORT,
                interface_key::LISTEN_PORT,
                ValueKind::U16,
                warnings,
                errors,
            );
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                interface_key::PORT,
                interface_key::FORWARD_PORT,
                ValueKind::U16,
                warnings,
                errors,
            );
        }
        "BackboneInterface" | "BackboneClientInterface" => {
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                interface_key::REMOTE,
                interface_key::TARGET_HOST,
                ValueKind::String,
                warnings,
                errors,
            );
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                interface_key::LISTEN_ON,
                interface_key::LISTEN_IP,
                ValueKind::String,
                warnings,
                errors,
            );
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                interface_key::PORT,
                interface_key::LISTEN_PORT,
                ValueKind::U16,
                warnings,
                errors,
            );
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                interface_key::PORT,
                interface_key::TARGET_PORT,
                ValueKind::U16,
                warnings,
                errors,
            );
        }
        _ => {}
    }

    validate_medium_requirements(source, name, type_name, section, locations, errors);

    if matches!(type_name, "RNodeInterface" | "RNodeMultiInterface") {
        let context = InterfaceRequirementContext {
            source,
            interface: name,
            section,
            locations,
        };
        validate_rnode_uri_transport(&context, type_name, &interface_path, enabled_key, errors);
    }

    if type_name == "RNodeMultiInterface" {
        super::rnode_multi::validate_subinterfaces(
            source, name, section, locations, warnings, errors,
        );
    } else {
        for (child, _) in &section.sections {
            warnings.push(WarningDiagnostic::new(
                WarningCode::UnknownSection,
                source,
                location(locations, &[section_key::INTERFACES, name, child]),
                format!("{interface_path} > [[[{child}]]]"),
                Some(child.clone()),
                "nested interface sections are not supported by this interface type",
                None,
                format!("remove [[[{child}]]] from [[{name}]]"),
            ));
        }
    }
}

#[derive(Clone, Copy)]
enum RNodeInterfaceFamily {
    Single,
    Multi,
}

#[derive(Clone, Copy)]
enum RNodeUriTransport {
    Tcp,
    Ble,
    Serial,
}

fn validate_rnode_uri_transport(
    context: &InterfaceRequirementContext<'_>,
    type_name: &str,
    interface_path: &str,
    enabled_key: &str,
    errors: &mut ValidationErrorCollector,
) {
    let family = match type_name {
        "RNodeInterface" => RNodeInterfaceFamily::Single,
        "RNodeMultiInterface" => RNodeInterfaceFamily::Multi,
        _ => return,
    };
    let Some(port) = rnode_port(context) else {
        return;
    };
    match (family, rnode_uri_transport(port)) {
        (RNodeInterfaceFamily::Single, RNodeUriTransport::Tcp) => {
            validate_rnode_tcp_target(context, interface_path, port, errors);
        }
        (RNodeInterfaceFamily::Single, RNodeUriTransport::Ble) => {
            validate_rnode_ble_target(context, interface_path, port, errors);
        }
        (RNodeInterfaceFamily::Multi, RNodeUriTransport::Tcp | RNodeUriTransport::Ble) => {
            report_unavailable_rnode_multi_transport(
                context,
                interface_path,
                enabled_key,
                port,
                errors,
            );
        }
        (_, RNodeUriTransport::Serial) => {}
    }
}

fn validate_rnode_ble_target(
    context: &InterfaceRequirementContext<'_>,
    interface_path: &str,
    port: &str,
    errors: &mut ValidationErrorCollector,
) {
    let target = &port.trim()[rnode_key::BLE_SCHEME.len()..];
    if crate::plan::RNodeBleTarget::from_uri_suffix(target.to_string()).is_ok() {
        return;
    }
    let port_key = interface_key::PORT;
    errors.push(ErrorDiagnostic::new(
        ErrorCode::InvalidValue,
        context.source,
        location(
            context.locations,
            &[section_key::INTERFACES, context.interface, port_key],
        ),
        format!("{interface_path} > {port_key}"),
        Some(port.to_string()),
        "the RNode Bluetooth LE target looks like an address but contains an invalid octet",
        Some("ble://, ble:// followed by an exact device name, or six hexadecimal octets separated by colons".to_string()),
        format!(
            "set `{port_key} = ble://AA:BB:CC:DD:EE:FF` or `{port_key} = ble://RNode 1234` for [[{}]]",
            context.interface
        ),
    ));
}

fn rnode_port<'a>(context: &'a InterfaceRequirementContext<'_>) -> Option<&'a str> {
    context
        .section
        .scalars
        .iter()
        .find(|(key, _)| key == interface_key::PORT)
        .and_then(|(_, value)| value.as_scalar())
}

fn rnode_uri_transport(port: &str) -> RNodeUriTransport {
    let transport = port.trim().to_ascii_lowercase();
    if transport.starts_with(rnode_key::TCP_SCHEME) {
        RNodeUriTransport::Tcp
    } else if transport.starts_with(rnode_key::BLE_SCHEME) {
        RNodeUriTransport::Ble
    } else {
        RNodeUriTransport::Serial
    }
}

fn validate_rnode_tcp_target(
    context: &InterfaceRequirementContext<'_>,
    interface_path: &str,
    port: &str,
    errors: &mut ValidationErrorCollector,
) {
    let target = &port.trim()[rnode_key::TCP_SCHEME.len()..];
    let Some((host, configured_port)) = target.split_once(':') else {
        return;
    };
    if host.is_empty()
        || configured_port.is_empty()
        || configured_port.contains(':')
        || configured_port.parse::<u16>().is_err()
    {
        return;
    }
    let port_key = interface_key::PORT;
    errors.push(ErrorDiagnostic::new(
        ErrorCode::InvalidValue,
        context.source,
        location(
            context.locations,
            &[section_key::INTERFACES, context.interface, port_key],
        ),
        format!("{interface_path} > {port_key}"),
        Some(port.to_string()),
        "RNode TCP always uses port 7633",
        Some("tcp:// followed by a host without a port".to_string()),
        format!(
            "set `{port_key} = tcp://{host}` for [[{}]]",
            context.interface
        ),
    ));
}

fn report_unavailable_rnode_multi_transport(
    context: &InterfaceRequirementContext<'_>,
    interface_path: &str,
    enabled_key: &str,
    port: &str,
    errors: &mut ValidationErrorCollector,
) {
    let port_key = interface_key::PORT;
    errors.push(ErrorDiagnostic::new(
        ErrorCode::UnsupportedTransport,
        context.source,
        location(
            context.locations,
            &[section_key::INTERFACES, context.interface, port_key],
        ),
        format!("{interface_path} > {port_key}"),
        Some(port.to_string()),
        "RNodeMulti requires one local serial device for all of its radios",
        Some("a local serial device path".to_string()),
        format!(
            "set `{port_key} = {}` for [[{}]], or set `{enabled_key} = No`",
            serial_port_example(),
            context.interface
        ),
    ));
}

fn serial_port_example() -> &'static str {
    if cfg!(windows) {
        "COM3"
    } else {
        "/dev/ttyUSB0"
    }
}

fn validate_medium_requirements(
    source: &str,
    interface: &str,
    type_name: &str,
    section: &Section,
    locations: &SourceLocations,
    errors: &mut ValidationErrorCollector,
) {
    let context = InterfaceRequirementContext {
        source,
        interface,
        section,
        locations,
    };
    match type_name {
        "TCPClientInterface" => {
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::TARGET_HOST,
                    alternatives: &[],
                    accepted: "a TCP target host",
                    correction: format!(
                        "add `{} = example.com` under [[{interface}]]",
                        interface_key::TARGET_HOST
                    ),
                },
                errors,
            );
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::TARGET_PORT,
                    alternatives: &[],
                    accepted: "a TCP target port",
                    correction: format!(
                        "add `{} = 4242` under [[{interface}]]",
                        interface_key::TARGET_PORT
                    ),
                },
                errors,
            );
        }
        "PrnsWebSocketClient" => {
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::TARGET,
                    alternatives: &[],
                    accepted: "a ws:// or wss:// WebSocket target",
                    correction: format!(
                        "add `{} = wss://example.com/prns` under [[{interface}]]",
                        interface_key::TARGET
                    ),
                },
                errors,
            );
            validate_websocket_target(&context, errors);
        }
        "PrnsWebSocketServer" => {
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::PORT,
                    alternatives: &[interface_key::LISTEN_PORT],
                    accepted: "port or listen_port",
                    correction: format!(
                        "add `{} = 4242` under [[{interface}]]",
                        interface_key::PORT
                    ),
                },
                errors,
            );
        }
        "TCPServerInterface" => {
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::PORT,
                    alternatives: &[interface_key::LISTEN_PORT],
                    accepted: "port or listen_port",
                    correction: format!(
                        "add `{} = 4242` under [[{interface}]]",
                        interface_key::PORT
                    ),
                },
                errors,
            );
        }
        "UDPInterface" => {
            validate_udp_requirements(&context, errors);
        }
        "SerialInterface" | "KISSInterface" | "WeaveInterface" => {
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::PORT,
                    alternatives: &[],
                    accepted: "a serial device path",
                    correction: format!(
                        "add `{} = {}` under [[{interface}]]",
                        interface_key::PORT,
                        serial_port_example()
                    ),
                },
                errors,
            );
            if type_name == "KISSInterface" {
                validate_station_identification(&context, errors);
            }
        }
        "AX25KISSInterface" => {
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::PORT,
                    alternatives: &[],
                    accepted: "a serial device path",
                    correction: format!(
                        "add `{} = {}` under [[{interface}]]",
                        interface_key::PORT,
                        serial_port_example()
                    ),
                },
                errors,
            );
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::CALLSIGN,
                    alternatives: &[],
                    accepted: "an AX.25 source callsign",
                    correction: format!(
                        "add `{} = N0CALL` under [[{interface}]]",
                        interface_key::CALLSIGN
                    ),
                },
                errors,
            );
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::SSID,
                    alternatives: &[],
                    accepted: "an AX.25 SSID",
                    correction: format!("add `{} = 0` under [[{interface}]]", interface_key::SSID),
                },
                errors,
            );
        }
        "RNodeInterface" => {
            for required in [
                RequiredSetting {
                    primary: interface_key::PORT,
                    alternatives: &[],
                    accepted: "a local serial device path or tcp:// followed by a host",
                    correction: format!(
                        "add `{} = {}` or `{} = tcp://radio.example` under [[{interface}]]",
                        interface_key::PORT,
                        serial_port_example(),
                        interface_key::PORT
                    ),
                },
                RequiredSetting {
                    primary: interface_key::FREQUENCY,
                    alternatives: &[],
                    accepted: "a radio frequency",
                    correction: format!(
                        "add `{} = 868000000` under [[{interface}]]",
                        interface_key::FREQUENCY
                    ),
                },
                RequiredSetting {
                    primary: interface_key::BANDWIDTH,
                    alternatives: &[],
                    accepted: "a radio bandwidth",
                    correction: format!(
                        "add `{} = 125000` under [[{interface}]]",
                        interface_key::BANDWIDTH
                    ),
                },
                RequiredSetting {
                    primary: interface_key::TXPOWER,
                    alternatives: &[],
                    accepted: "a transmit power",
                    correction: format!(
                        "add `{} = 7` under [[{interface}]]",
                        interface_key::TXPOWER
                    ),
                },
                RequiredSetting {
                    primary: interface_key::SPREADINGFACTOR,
                    alternatives: &[],
                    accepted: "a LoRa spreading factor",
                    correction: format!(
                        "add `{} = 8` under [[{interface}]]",
                        interface_key::SPREADINGFACTOR
                    ),
                },
                RequiredSetting {
                    primary: interface_key::CODINGRATE,
                    alternatives: &[],
                    accepted: "a LoRa coding rate",
                    correction: format!(
                        "add `{} = 5` under [[{interface}]]",
                        interface_key::CODINGRATE
                    ),
                },
            ] {
                require_setting(&context, required, errors);
            }
            validate_station_identification(&context, errors);
            validate_rnode_station_callsign(&context, errors);
        }
        "RNodeMultiInterface" => {
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::PORT,
                    alternatives: &[],
                    accepted: "a local serial device path",
                    correction: format!(
                        "add `{} = {}` under [[{interface}]]",
                        interface_key::PORT,
                        serial_port_example()
                    ),
                },
                errors,
            );
            validate_station_identification(&context, errors);
            validate_rnode_station_callsign(&context, errors);
        }
        "PipeInterface" => {
            require_setting(
                &context,
                RequiredSetting {
                    primary: interface_key::COMMAND,
                    alternatives: &[],
                    accepted: "a subprocess command",
                    correction: format!(
                        "add `{} = /path/to/program` under [[{interface}]]",
                        interface_key::COMMAND
                    ),
                },
                errors,
            );
        }
        "BackboneInterface" | "BackboneClientInterface" => {
            let client = type_name == "BackboneClientInterface"
                || has_one_of(
                    section,
                    &[interface_key::TARGET_HOST, interface_key::REMOTE],
                );
            if client {
                require_setting(
                    &context,
                    RequiredSetting {
                        primary: interface_key::TARGET_HOST,
                        alternatives: &[interface_key::REMOTE],
                        accepted: "target_host or remote",
                        correction: format!(
                            "add `{} = backbone.example.com` under [[{interface}]]",
                            interface_key::REMOTE
                        ),
                    },
                    errors,
                );
                require_setting(
                    &context,
                    RequiredSetting {
                        primary: interface_key::PORT,
                        alternatives: &[interface_key::TARGET_PORT],
                        accepted: "port or target_port",
                        correction: format!(
                            "add `{} = 4242` under [[{interface}]]",
                            interface_key::PORT
                        ),
                    },
                    errors,
                );
            } else {
                require_setting(
                    &context,
                    RequiredSetting {
                        primary: interface_key::PORT,
                        alternatives: &[interface_key::LISTEN_PORT],
                        accepted: "port or listen_port",
                        correction: format!(
                            "add `{} = 4242` under [[{interface}]]",
                            interface_key::PORT
                        ),
                    },
                    errors,
                );
            }
        }
        _ => {}
    }
}

fn validate_websocket_target(
    context: &InterfaceRequirementContext<'_>,
    errors: &mut ValidationErrorCollector,
) {
    let Some(target) = context
        .section
        .get(interface_key::TARGET)
        .and_then(Value::as_scalar)
    else {
        return;
    };
    let target = target.trim();
    if supported_websocket_target(target) {
        return;
    }
    errors.push(ErrorDiagnostic::new(
        ErrorCode::InvalidValue,
        context.source,
        location(
            context.locations,
            &[
                section_key::INTERFACES,
                context.interface,
                interface_key::TARGET,
            ],
        ),
        format!(
            "[interfaces] > [[{}]] > {}",
            context.interface,
            interface_key::TARGET
        ),
        Some(target.to_string()),
        "the WebSocket client target is not a ws:// or wss:// URL",
        Some("a ws:// or wss:// URL with no whitespace".to_string()),
        format!(
            "set `{} = wss://example.com/prns` under [[{}]]",
            interface_key::TARGET,
            context.interface
        ),
    ));
}

pub(crate) fn supported_websocket_target(target: &str) -> bool {
    let address = target
        .strip_prefix("ws://")
        .or_else(|| target.strip_prefix("wss://"));
    address.is_some_and(|address| !address.is_empty()) && !target.chars().any(char::is_whitespace)
}

fn validate_station_identification(
    context: &InterfaceRequirementContext<'_>,
    errors: &mut ValidationErrorCollector,
) {
    let callsign = context.section.get(interface_key::ID_CALLSIGN).is_some();
    let interval = context.section.get(interface_key::ID_INTERVAL).is_some();
    match (callsign, interval) {
        (true, false) => missing_setting(
            context,
            interface_key::ID_INTERVAL,
            "id_interval whenever id_callsign is set",
            format!(
                "add `{} = 600` under [[{}]], or remove `{}`",
                interface_key::ID_INTERVAL,
                context.interface,
                interface_key::ID_CALLSIGN
            ),
            errors,
        ),
        (false, true) => missing_setting(
            context,
            interface_key::ID_CALLSIGN,
            "id_callsign whenever id_interval is set",
            format!(
                "add `{} = N0CALL` under [[{}]], or remove `{}`",
                interface_key::ID_CALLSIGN,
                context.interface,
                interface_key::ID_INTERVAL
            ),
            errors,
        ),
        (false, false) | (true, true) => {}
    }
}

fn validate_rnode_station_callsign(
    context: &InterfaceRequirementContext<'_>,
    errors: &mut ValidationErrorCollector,
) {
    let Some(value) = context.section.get(interface_key::ID_CALLSIGN) else {
        return;
    };
    let Some(callsign) = value.as_scalar() else {
        return;
    };
    if callsign.len() <= 32 {
        return;
    }
    errors.push(ErrorDiagnostic::new(
        ErrorCode::InvalidValue,
        context.source,
        location(
            context.locations,
            &[
                section_key::INTERFACES,
                context.interface,
                interface_key::ID_CALLSIGN,
            ],
        ),
        format!(
            "[interfaces] > [[{}]] > {}",
            context.interface,
            interface_key::ID_CALLSIGN
        ),
        Some(callsign.to_string()),
        "RNode station identification exceeds the device command capacity",
        Some("a non-empty UTF-8 value no longer than 32 encoded bytes".to_string()),
        format!(
            "shorten `{}` to at most 32 UTF-8 bytes",
            interface_key::ID_CALLSIGN
        ),
    ));
}

fn validate_udp_requirements(
    context: &InterfaceRequirementContext<'_>,
    errors: &mut ValidationErrorCollector,
) {
    let section = context.section;
    let device = section.get(interface_key::DEVICE).is_some();
    let shared_port = section.get(interface_key::PORT).is_some();
    let receive_intent = section.get(interface_key::LISTEN_IP).is_some()
        || section.get(interface_key::LISTEN_PORT).is_some()
        || (device && shared_port);
    let send_intent = section.get(interface_key::FORWARD_IP).is_some()
        || section.get(interface_key::FORWARD_PORT).is_some()
        || (device && shared_port);

    if !receive_intent && !send_intent {
        let accepted = if shared_port {
            "listen_ip, forward_ip, or device"
        } else {
            "a complete receive endpoint, send endpoint, or device plus port"
        };
        missing_setting(
            context,
            "udp_endpoint",
            accepted,
            format!(
                "add `{} = 0.0.0.0` and `{} = 4242`, or add `{} = 255.255.255.255` and `{} = 4242` under [[{}]]",
                interface_key::LISTEN_IP,
                interface_key::LISTEN_PORT,
                interface_key::FORWARD_IP,
                interface_key::FORWARD_PORT,
                context.interface,
            ),
            errors,
        );
        return;
    }

    if receive_intent {
        if !device && section.get(interface_key::LISTEN_IP).is_none() {
            missing_setting(
                context,
                interface_key::LISTEN_IP,
                "listen_ip or device",
                format!(
                    "add `{} = 0.0.0.0` under [[{}]]",
                    interface_key::LISTEN_IP,
                    context.interface,
                ),
                errors,
            );
        }
        if !shared_port && section.get(interface_key::LISTEN_PORT).is_none() {
            missing_setting(
                context,
                interface_key::LISTEN_PORT,
                "listen_port or port",
                format!(
                    "add `{} = 4242` under [[{}]]",
                    interface_key::LISTEN_PORT,
                    context.interface,
                ),
                errors,
            );
        }
    }

    if send_intent {
        if !device && section.get(interface_key::FORWARD_IP).is_none() {
            missing_setting(
                context,
                interface_key::FORWARD_IP,
                "forward_ip or device",
                format!(
                    "add `{} = 255.255.255.255` under [[{}]]",
                    interface_key::FORWARD_IP,
                    context.interface,
                ),
                errors,
            );
        }
        if !shared_port && section.get(interface_key::FORWARD_PORT).is_none() {
            missing_setting(
                context,
                interface_key::FORWARD_PORT,
                "forward_port or port",
                format!(
                    "add `{} = 4242` under [[{}]]",
                    interface_key::FORWARD_PORT,
                    context.interface,
                ),
                errors,
            );
        }
    }
}

struct InterfaceRequirementContext<'a> {
    source: &'a str,
    interface: &'a str,
    section: &'a Section,
    locations: &'a SourceLocations,
}

struct RequiredSetting<'a> {
    primary: &'a str,
    alternatives: &'a [&'a str],
    accepted: &'a str,
    correction: String,
}

fn require_setting(
    context: &InterfaceRequirementContext<'_>,
    required: RequiredSetting<'_>,
    errors: &mut ValidationErrorCollector,
) {
    if context.section.get(required.primary).is_some()
        || has_one_of(context.section, required.alternatives)
    {
        return;
    }
    missing_setting(
        context,
        required.primary,
        required.accepted,
        required.correction,
        errors,
    );
}

fn has_one_of(section: &Section, keys: &[&str]) -> bool {
    keys.iter().any(|key| section.get(key).is_some())
}

fn missing_setting(
    context: &InterfaceRequirementContext<'_>,
    key: &str,
    accepted: &str,
    correction: String,
    errors: &mut ValidationErrorCollector,
) {
    let interface = context.interface;
    errors.push(ErrorDiagnostic::new(
        ErrorCode::MissingRequiredKey,
        context.source,
        location(context.locations, &[section_key::INTERFACES, interface]),
        format!("[interfaces] > [[{interface}]] > {key}"),
        None,
        format!("enabled interface is missing {accepted}"),
        Some(accepted.to_string()),
        correction,
    ));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_alias_group(
    source: &str,
    source_path: &[&str],
    display_path: &str,
    section: &Section,
    locations: &SourceLocations,
    canonical: &str,
    keys: &[&str],
    kind: ValueKind,
    warnings: &mut ValidationWarnings,
    errors: &mut ValidationErrorCollector,
) -> Option<String> {
    let mut values = Vec::new();
    for key in keys {
        let Some(value) = section.get(key) else {
            continue;
        };
        match normalized_value(value, kind) {
            Ok(normalized) => values.push((*key, value, normalized)),
            Err(()) => validate_value(
                source,
                setting_location(locations, source_path, key),
                format!("{display_path} > {key}"),
                key,
                value,
                kind,
                errors,
            ),
        }
    }
    let first = values.first()?;
    if let Some(second) = values.get(1) {
        let line = setting_location(locations, source_path, second.0);
        let path = format!("{display_path} > {canonical}");
        let value = Some(format!(
            "{} = {}; {} = {}",
            first.0,
            value_text(first.1),
            second.0,
            value_text(second.1),
        ));
        let accepted = Some(kind.accepted().to_string());
        let correction = format!("keep only `{canonical} = {}`", first.2);
        if first.2 == second.2 {
            warnings.push(
                WarningDiagnostic::new(
                    WarningCode::RedundantAliases,
                    source,
                    line,
                    path,
                    value,
                    format!(
                        "{0:?} and {1:?} specify the same setting",
                        first.0, second.0
                    ),
                    accepted,
                    correction,
                )
                .with_fixes(vec![ConfigFix::RemoveValue {
                    path: format!("{display_path} > {}", second.0),
                    safety: ConfigFixSafety::Safe,
                }]),
            );
        } else {
            errors.push(
                ErrorDiagnostic::new(
                    ErrorCode::ConflictingAliases,
                    source,
                    line,
                    path.clone(),
                    value,
                    format!(
                        "{0:?} and {1:?} specify different values",
                        first.0, second.0
                    ),
                    accepted,
                    correction,
                )
                .with_fixes(vec![ConfigFix::ResolveAliases {
                    path,
                    aliases: vec![second.0.to_string()],
                }]),
            );
        }
    }
    Some(first.2.clone())
}

#[allow(clippy::too_many_arguments)]
fn compare_alias_pair(
    source: &str,
    interface: &str,
    section: &Section,
    locations: &SourceLocations,
    canonical: &str,
    alias: &str,
    kind: ValueKind,
    warnings: &mut ValidationWarnings,
    errors: &mut ValidationErrorCollector,
) {
    let (Some(canonical_value), Some(alias_value)) = (section.get(canonical), section.get(alias))
    else {
        return;
    };
    let (Ok(canonical_normalized), Ok(alias_normalized)) = (
        normalized_value(canonical_value, kind),
        normalized_value(alias_value, kind),
    ) else {
        return;
    };
    let line = location(locations, &[section_key::INTERFACES, interface, alias]);
    let path = format!("[interfaces] > [[{interface}]] > {canonical}");
    let value = Some(format!(
        "{canonical} = {}; {alias} = {}",
        value_text(canonical_value),
        value_text(alias_value),
    ));
    let accepted = Some(kind.accepted().to_string());
    let correction = format!("keep only {canonical} = {canonical_normalized}");
    if canonical_normalized == alias_normalized {
        warnings.push(
            WarningDiagnostic::new(
                WarningCode::RedundantAliases,
                source,
                line,
                path,
                value,
                format!("{canonical:?} and {alias:?} specify the same setting"),
                accepted,
                correction,
            )
            .with_fixes(vec![ConfigFix::RemoveValue {
                path: format!("[interfaces] > [[{interface}]] > {alias}"),
                safety: ConfigFixSafety::Safe,
            }]),
        );
    } else {
        errors.push(
            ErrorDiagnostic::new(
                ErrorCode::ConflictingAliases,
                source,
                line,
                path.clone(),
                value,
                format!("{canonical:?} and {alias:?} specify different values"),
                accepted,
                correction,
            )
            .with_fixes(vec![ConfigFix::ResolveAliases {
                path,
                aliases: vec![alias.to_string()],
            }]),
        );
    }
}

pub(super) fn validate_value(
    source: &str,
    line: usize,
    path: String,
    key: &str,
    value: &Value,
    kind: ValueKind,
    errors: &mut ValidationErrorCollector,
) {
    let i2p_error = match kind {
        ValueKind::I2pPeers => validate_peers(value.as_list())
            .err()
            .map(|error| error.to_string()),
        _ => None,
    };
    if i2p_error.is_none()
        && normalized_value(value, kind).is_ok()
        && semantic_value_is_valid(key, value, kind)
    {
        return;
    }
    errors.push(ErrorDiagnostic::new(
        ErrorCode::InvalidValue,
        source,
        line,
        path,
        Some(value_text(value)),
        i2p_error.map_or_else(
            || format!("invalid value for {key:?}"),
            |reason| format!("invalid value for {key:?}: {reason}"),
        ),
        Some(accepted_for_key(key, kind)),
        format!("set `{key} = {}`", example_for_key(key, kind)),
    ));
}

fn accepted_for_key(key: &str, kind: ValueKind) -> String {
    match kind {
        ValueKind::RnodeMultiVport
        | ValueKind::RnodeMultiFrequency
        | ValueKind::RnodeMultiTxPower
        | ValueKind::BlackholeUpdateInterval
        | ValueKind::WebSocketFramingSelection
        | ValueKind::ByteQuantity => return kind.accepted().to_string(),
        _ => {}
    }
    match key {
        interface_key::IFAC_SIZE => format!(
            "an integer from 0 through {} bits",
            IFAC_MAX_SIZE * 8 + 7
        ),
        interface_key::ANNOUNCE_CAP => "a percentage from 0 through 100".to_string(),
        interface_key::DISCOVERY_SCOPE => {
            "one of link, admin, site, organisation, or global".to_string()
        }
        interface_key::MULTICAST_ADDRESS_TYPE => {
            "one of temporary or permanent".to_string()
        }
        interface_key::DISCOVERY_PORT => {
            "an integer from 1 through 65534; the following port is reserved for reverse discovery"
                .to_string()
        }
        interface_key::DATA_PORT => "an integer from 1 through 65535".to_string(),
        interface_key::SPEED => format!(
            "an integer from {} through {} bits per second",
            prns_core::interfaces::BitrateBps::MINIMUM,
            u32::MAX
        ),
        interface_key::DATABITS => "one of 5, 6, 7, or 8".to_string(),
        interface_key::PARITY => "one of N, None, E, Even, O, or Odd".to_string(),
        interface_key::STOPBITS => "one of 1 or 2".to_string(),
        interface_key::ID_CALLSIGN => "a non-empty station identifier".to_string(),
        interface_key::CALLSIGN => {
            "an ASCII AX.25 callsign containing 3 through 6 characters".to_string()
        }
        interface_key::SSID => "an integer from 0 through 15".to_string(),
        interface_key::FREQUENCY => format!(
            "an integer from {FREQUENCY_HZ_MIN} through {FREQUENCY_HZ_MAX} hertz"
        ),
        interface_key::BANDWIDTH => format!(
            "an integer from {BANDWIDTH_HZ_MIN} through {BANDWIDTH_HZ_MAX} hertz"
        ),
        interface_key::TXPOWER => {
            format!("an integer from {TXPOWER_DBM_MIN} through {TXPOWER_DBM_MAX} dBm")
        }
        interface_key::SPREADINGFACTOR => format!(
            "an integer from {SPREADING_FACTOR_MIN} through {SPREADING_FACTOR_MAX}"
        ),
        interface_key::CODINGRATE => {
            format!("an integer from {CODING_RATE_MIN} through {CODING_RATE_MAX}")
        }
        interface_key::AIRTIME_LIMIT_SHORT | interface_key::AIRTIME_LIMIT_LONG => {
            "a percentage from 0 through 100".to_string()
        }
        interface_key::RESPAWN_DELAY => {
            "a non-negative finite duration in seconds representable by the host".to_string()
        }
        interface_key::COMMAND => {
            "a non-empty command with complete shell-style quoting".to_string()
        }
        common_key::IC_BURST_HOLD | common_key::IC_NEW_TIME | common_key::IC_BURST_PENALTY | common_key::IC_HELD_RELEASE_INTERVAL => {
            "a non-negative duration in seconds whose rounded milliseconds are below 18446744073709551616"
                .to_string()
        }
        common_key::IC_BURST_FREQ_NEW
        | common_key::IC_BURST_FREQ
        | common_key::IC_PR_BURST_FREQ_NEW
        | common_key::IC_PR_BURST_FREQ
        | common_key::EC_PR_FREQ => {
            "a non-negative frequency in hertz whose rounded millihertz are below 18446744073709551616"
                .to_string()
        }
        common_key::IC_MAX_HELD_ANNOUNCES => format!(
            "an integer from 0 through {}",
            (usize::MAX as u128).min(i64::MAX as u128)
        ),
        global_key::DEFAULT_AR_TARGET => {
            format!(
                "off, no, false, or an integer from 0 through {} seconds (0 is treated as off)",
                i64::MAX / 1_000
            )
        }
        global_key::DEFAULT_AR_PENALTY => {
            format!("an integer from 0 through {} seconds", i64::MAX / 1_000)
        }
        interface_key::ANNOUNCE_RATE_TARGET => {
            format!(
                "off, no, false, or an integer from 0 through {} seconds (0 is treated as off)",
                u64::MAX / 1_000
            )
        }
        interface_key::ANNOUNCE_RATE_PENALTY => {
            format!("an integer from 0 through {} seconds", u64::MAX / 1_000)
        }
        global_key::DEFAULT_AR_GRACE | interface_key::ANNOUNCE_RATE_GRACE => "an integer from 0 through 65535".to_string(),
        _ => kind.accepted().to_string(),
    }
}

pub(super) fn example_for_key(key: &str, kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::RnodeMultiVport
        | ValueKind::RnodeMultiFrequency
        | ValueKind::RnodeMultiTxPower
        | ValueKind::BlackholeUpdateInterval
        | ValueKind::WebSocketFramingSelection
        | ValueKind::ByteQuantity => return kind.example(),
        _ => {}
    }
    match key {
        interface_key::IFAC_SIZE => "64",
        interface_key::ANNOUNCE_CAP => "2.0",
        interface_key::DISCOVERY_SCOPE => "link",
        interface_key::MULTICAST_ADDRESS_TYPE => "temporary",
        interface_key::DISCOVERY_PORT => "29716",
        interface_key::DATA_PORT => "42671",
        interface_key::SPEED => "9600",
        interface_key::DATABITS => "8",
        interface_key::PARITY => "N",
        interface_key::STOPBITS => "1",
        interface_key::ID_CALLSIGN | interface_key::CALLSIGN => "N0CALL",
        interface_key::SSID => "0",
        interface_key::FREQUENCY => "868000000",
        interface_key::BANDWIDTH => "125000",
        interface_key::TXPOWER => "7",
        interface_key::SPREADINGFACTOR => "8",
        interface_key::CODINGRATE => "5",
        interface_key::AIRTIME_LIMIT_SHORT => "1.5",
        interface_key::AIRTIME_LIMIT_LONG => "5.0",
        interface_key::RESPAWN_DELAY => "5.0",
        interface_key::COMMAND => "/path/to/program --option value",
        common_key::IC_BURST_HOLD | common_key::IC_BURST_PENALTY => "15.0",
        common_key::IC_NEW_TIME => "7200.0",
        common_key::IC_HELD_RELEASE_INTERVAL => "5.0",
        common_key::IC_BURST_FREQ_NEW | common_key::IC_PR_BURST_FREQ_NEW => "3.0",
        common_key::IC_BURST_FREQ => "10.0",
        common_key::IC_PR_BURST_FREQ => "8.0",
        common_key::EC_PR_FREQ => "5.0",
        common_key::IC_MAX_HELD_ANNOUNCES => "256",
        global_key::DEFAULT_AR_TARGET | interface_key::ANNOUNCE_RATE_TARGET => "3600",
        global_key::DEFAULT_AR_PENALTY | interface_key::ANNOUNCE_RATE_PENALTY => "0",
        global_key::DEFAULT_AR_GRACE | interface_key::ANNOUNCE_RATE_GRACE => "5",
        _ => kind.example(),
    }
}

fn semantic_value_is_valid(key: &str, value: &Value, kind: ValueKind) -> bool {
    let Some(text) = value.as_scalar() else {
        return true;
    };
    if let Some(valid) = super::rnode_multi::semantic_value_is_valid(kind, text) {
        return valid;
    }
    match key {
        interface_key::IFAC_SIZE => {
            parse_integer::<u32>(text).is_ok_and(|value| value <= (IFAC_MAX_SIZE * 8 + 7) as u32)
        }
        interface_key::ANNOUNCE_CAP => {
            parse_float(text).is_ok_and(|value| (0.0..=100.0).contains(&value))
        }
        interface_key::DISCOVERY_SCOPE => {
            prns_core::interfaces::wifi_auto::DiscoveryScope::from_name(text.trim()).is_some()
        }
        interface_key::MULTICAST_ADDRESS_TYPE => {
            prns_core::interfaces::wifi_auto::MulticastAddressType::from_name(text.trim()).is_some()
        }
        interface_key::FRAMING => {
            prns_core::interfaces::websocket::WebSocketFramingSelection::from_name(text.trim())
                .is_ok()
        }
        interface_key::DISCOVERY_PORT => {
            parse_integer::<u16>(text).is_ok_and(|value| (1..=u16::MAX - 1).contains(&value))
        }
        interface_key::DATA_PORT => parse_integer::<u16>(text).is_ok_and(|value| value != 0),
        interface_key::SPEED => parse_integer::<u32>(text)
            .is_ok_and(|value| u64::from(value) >= prns_core::interfaces::BitrateBps::MINIMUM),
        interface_key::DATABITS => {
            parse_integer::<u8>(text).is_ok_and(|value| matches!(value, 5..=8))
        }
        interface_key::PARITY => matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "n" | "none" | "e" | "even" | "o" | "odd"
        ),
        interface_key::STOPBITS => {
            parse_integer::<u8>(text).is_ok_and(|value| matches!(value, 1 | 2))
        }
        interface_key::ID_CALLSIGN => !text.is_empty(),
        interface_key::CALLSIGN => text.is_ascii() && (3..=6).contains(&text.len()),
        interface_key::SSID => parse_integer::<u8>(text).is_ok_and(|value| value <= 15),
        interface_key::FREQUENCY => parse_integer::<u64>(text)
            .is_ok_and(|value| (FREQUENCY_HZ_MIN..=FREQUENCY_HZ_MAX).contains(&value)),
        interface_key::BANDWIDTH => parse_integer::<u32>(text)
            .is_ok_and(|value| (BANDWIDTH_HZ_MIN..=BANDWIDTH_HZ_MAX).contains(&value)),
        interface_key::TXPOWER => parse_integer::<i16>(text)
            .is_ok_and(|value| (TXPOWER_DBM_MIN..=TXPOWER_DBM_MAX).contains(&value)),
        interface_key::SPREADINGFACTOR => parse_integer::<u8>(text)
            .is_ok_and(|value| (SPREADING_FACTOR_MIN..=SPREADING_FACTOR_MAX).contains(&value)),
        interface_key::CODINGRATE => parse_integer::<u8>(text)
            .is_ok_and(|value| (CODING_RATE_MIN..=CODING_RATE_MAX).contains(&value)),
        interface_key::AIRTIME_LIMIT_SHORT | interface_key::AIRTIME_LIMIT_LONG => {
            parse_float(text).is_ok_and(|value| value.is_finite() && (0.0..=100.0).contains(&value))
        }
        interface_key::RESPAWN_DELAY => parse_float(text)
            .is_ok_and(|value| std::time::Duration::try_from_secs_f64(value).is_ok()),
        interface_key::COMMAND => shlex::split(text).is_some_and(|argv| !argv.is_empty()),
        common_key::IC_BURST_HOLD
        | common_key::IC_BURST_FREQ_NEW
        | common_key::IC_BURST_FREQ
        | common_key::IC_PR_BURST_FREQ_NEW
        | common_key::IC_PR_BURST_FREQ
        | common_key::EC_PR_FREQ
        | common_key::IC_NEW_TIME
        | common_key::IC_BURST_PENALTY
        | common_key::IC_HELD_RELEASE_INTERVAL => {
            parse_float(text).is_ok_and(fixed_milli_value_is_valid)
        }
        common_key::IC_MAX_HELD_ANNOUNCES => parse_integer::<i64>(text).is_ok_and(|value| {
            value >= 0 && u128::try_from(value).is_ok_and(|value| value <= usize::MAX as u128)
        }),
        global_key::DEFAULT_AR_TARGET => {
            super::announce_rate_target_is_explicit_off(text)
                || parse_integer::<i64>(text)
                    .is_ok_and(|value| (0..=i64::MAX / 1_000).contains(&value))
        }
        global_key::DEFAULT_AR_PENALTY => {
            parse_integer::<i64>(text).is_ok_and(|value| (0..=i64::MAX / 1_000).contains(&value))
        }
        interface_key::ANNOUNCE_RATE_TARGET => {
            super::announce_rate_target_is_explicit_off(text)
                || parse_integer::<u64>(text).is_ok_and(|value| value <= u64::MAX / 1_000)
        }
        interface_key::ANNOUNCE_RATE_PENALTY => {
            parse_integer::<u64>(text).is_ok_and(|value| value <= u64::MAX / 1_000)
        }
        global_key::DEFAULT_AR_GRACE => {
            parse_integer::<i64>(text).is_ok_and(|value| (0..=i64::from(u16::MAX)).contains(&value))
        }
        interface_key::ANNOUNCE_RATE_GRACE => {
            parse_integer::<u64>(text).is_ok_and(|value| value <= u64::from(u16::MAX))
        }
        global_key::BLACKHOLE_UPDATE_INTERVAL => parse_float(text).is_ok_and(|minutes| {
            minutes.is_finite()
                && std::time::Duration::try_from_secs_f64(minutes.max(2.0) * 60.0).is_ok()
        }),
        _ => true,
    }
}

fn fixed_milli_value_is_valid(value: f64) -> bool {
    value.is_finite() && value >= 0.0 && (value * 1_000.0).round() < u64::MAX as f64
}

fn normalized_value(value: &Value, kind: ValueKind) -> Result<String, ()> {
    if matches!(kind, ValueKind::List | ValueKind::I2pPeers) {
        return Ok(value_text(value));
    }
    if matches!(kind, ValueKind::IdentityHashes) {
        return if value
            .as_list()
            .into_iter()
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .all(|item| parse_identity_hash(item).is_some())
        {
            Ok(value_text(value))
        } else {
            Err(())
        };
    }
    let text = value.as_scalar().ok_or(())?;
    let normalized = match kind {
        ValueKind::Bool => parse_bool(text).map(|value| value.to_string()).ok_or(())?,
        ValueKind::Mode => match text.trim().to_ascii_lowercase().as_str() {
            "full" => "full",
            "access_point" | "accesspoint" | "ap" => "access_point",
            "pointtopoint" | "ptp" => "pointtopoint",
            "roaming" => "roaming",
            "boundary" => "boundary",
            "gateway" | "gw" => "gateway",
            "internal" => "internal",
            _ => return Err(()),
        }
        .to_string(),
        ValueKind::String => text.to_string(),
        ValueKind::List | ValueKind::I2pPeers => {
            unreachable!("list values return before scalar coercion")
        }
        ValueKind::Bitrate => {
            let value = parse_integer::<u64>(text)?;
            if value < prns_core::interfaces::BitrateBps::MINIMUM {
                return Err(());
            }
            value.to_string()
        }
        ValueKind::LinkMtu => {
            let value = parse_integer::<usize>(text)?;
            if !(1..=prns_core::routing::links::MAX_LINK_MTU).contains(&value) {
                return Err(());
            }
            value.to_string()
        }
        ValueKind::U64 => parse_integer::<u64>(text)?.to_string(),
        ValueKind::SecondsOrOff => {
            if super::announce_rate_target_is_explicit_off(text) {
                "off".to_string()
            } else {
                parse_integer::<u64>(text)?.to_string()
            }
        }
        ValueKind::U32 => parse_integer::<u32>(text)?.to_string(),
        ValueKind::U16 => parse_integer::<u16>(text)?.to_string(),
        ValueKind::NonZeroU16 => {
            let value = parse_integer::<u16>(text)?;
            if value == 0 {
                return Err(());
            }
            value.to_string()
        }
        ValueKind::U8 => parse_integer::<u8>(text)?.to_string(),
        ValueKind::I16 => parse_integer::<i16>(text)?.to_string(),
        ValueKind::I64 => parse_integer::<i64>(text)?.to_string(),
        ValueKind::F64 => parse_float(text)?.to_string(),
        ValueKind::StampCost => {
            let value = parse_integer::<i64>(text)?;
            if !(0..=255).contains(&value) {
                return Err(());
            }
            value.to_string()
        }
        ValueKind::IdentityHashes => {
            unreachable!("identity hash lists return before scalar coercion")
        }
        ValueKind::LogLevel => {
            let value = parse_integer::<u8>(text)?;
            if value > 7 {
                return Err(());
            }
            value.to_string()
        }
        ValueKind::SharedInstanceType => match text.trim().to_ascii_lowercase().as_str() {
            "tcp" => "tcp".to_string(),
            "unix" => "unix".to_string(),
            _ => return Err(()),
        },
        ValueKind::HexBytes => {
            let text = text.trim();
            if text.is_empty()
                || text.len() % 2 != 0
                || !text.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(());
            }
            text.to_ascii_lowercase()
        }
        ValueKind::RnodeMultiVport => parse_integer::<u8>(text)?.to_string(),
        ValueKind::RnodeMultiFrequency => parse_integer::<u64>(text)?.to_string(),
        ValueKind::RnodeMultiTxPower => parse_integer::<i16>(text)?.to_string(),
        ValueKind::BlackholeUpdateInterval => {
            let minutes = parse_float(text)?;
            if !minutes.is_finite()
                || std::time::Duration::try_from_secs_f64(minutes.max(2.0) * 60.0).is_err()
            {
                return Err(());
            }
            minutes.to_string()
        }
        ValueKind::WebSocketFramingSelection => {
            prns_core::interfaces::websocket::WebSocketFramingSelection::from_name(text.trim())
                .map_err(|_| ())?
                .name()
                .to_string()
        }
        ValueKind::ByteQuantity => parse_byte_quantity(text).map_err(|_error| ())?.to_string(),
    };
    Ok(normalized)
}

fn parse_integer<T>(text: &str) -> Result<T, ()>
where
    T: TryFrom<i128>,
{
    let cleaned = cleaned_number(text.trim()).ok_or(())?;
    let parsed = cleaned.parse::<i128>().map_err(|_| ())?;
    T::try_from(parsed).map_err(|_| ())
}

fn parse_float(text: &str) -> Result<f64, ()> {
    let cleaned = cleaned_number(text.trim()).ok_or(())?;
    cleaned.parse::<f64>().map_err(|_| ())
}

pub(super) fn unknown_key(
    source: &str,
    line: usize,
    path: String,
    key: &str,
    value: &Value,
    known: &[&str],
) -> WarningDiagnostic {
    let suggestion = closest(key, known);
    WarningDiagnostic::new(
        WarningCode::UnknownKey,
        source,
        line,
        path,
        Some(value_text(value)),
        format!("unknown key {key:?}; it will not be applied"),
        suggestion.map(str::to_string),
        suggestion.map_or_else(
            || format!("remove {key:?} or move it to the section that defines it"),
            |expected| format!("rename {key:?} to {expected:?}"),
        ),
    )
}

fn unknown_interface_key(
    source: &str,
    line: usize,
    path: String,
    key: &str,
    value: &Value,
    known: &[&str],
) -> WarningDiagnostic {
    if matches!(
        key,
        "name" | "selected_interface_mode" | "configured_bitrate"
    ) {
        let (message, correction) = match key {
            "name" => (
                "stock RNS generated a copy of the interface label; the [[section heading]] is authoritative",
                "remove generated field \"name\"; rename the interface section to change its label",
            ),
            "selected_interface_mode" => (
                "stock RNS generated a numeric mode snapshot; the \"mode\" setting is authoritative",
                "remove generated field \"selected_interface_mode\"; set \"mode\" to configure interface behavior",
            ),
            "configured_bitrate" => (
                "stock RNS generated a bitrate snapshot; the \"bitrate\" setting is authoritative",
                "remove generated field \"configured_bitrate\"; set \"bitrate\" to configure an override",
            ),
            _ => unreachable!(),
        };
        return WarningDiagnostic::new(
            WarningCode::PersistedRuntimeMetadata,
            source,
            line,
            path,
            Some(value_text(value)),
            message,
            None,
            correction,
        );
    }
    unknown_key(source, line, path, key, value, known)
}

fn closest<'a>(actual: &str, known: &'a [&str]) -> Option<&'a str> {
    let (candidate, distance) = known
        .iter()
        .map(|candidate| (*candidate, edit_distance(actual, candidate)))
        .min_by_key(|(_, distance)| *distance)?;
    let threshold = 2usize.max(actual.len() / 3);
    (distance <= threshold).then_some(candidate)
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut previous = (0..=right.chars().count()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = Vec::with_capacity(previous.len());
        current.push(left_index + 1);
        for (right_index, right_char) in right.chars().enumerate() {
            let substitution = previous[right_index] + usize::from(left_char != right_char);
            let insertion = current[right_index] + 1;
            let deletion = previous[right_index + 1] + 1;
            current.push(substitution.min(insertion).min(deletion));
        }
        previous = current;
    }
    previous[right.chars().count()]
}

pub(super) fn location(locations: &SourceLocations, path: &[&str]) -> usize {
    locations.line(path.iter().copied()).unwrap_or(1)
}

pub(super) fn setting_location(
    locations: &SourceLocations,
    source_path: &[&str],
    key: &str,
) -> usize {
    let mut path = source_path.to_vec();
    path.push(key);
    location(locations, &path)
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Scalar(text) => text.clone(),
        Value::List(items) => items.join(", "),
    }
}

pub(super) fn legacy_diagnostic(
    source: &str,
    locations: &SourceLocations,
    error: ReferenceError,
) -> ConfigDiagnostic {
    match error {
        ReferenceError::Syntax(error) => ConfigDiagnostic::new(
            ConfigDiagnosticCode::Syntax,
            source,
            error.line(),
            "<document>",
            None,
            error.to_string(),
            None,
            format!("correct the syntax on line {}", error.line()),
        ),
        ReferenceError::MissingType { interface } => ConfigDiagnostic::new(
            ConfigDiagnosticCode::MissingRequiredKey,
            source,
            location(locations, &[section_key::INTERFACES, &interface]),
            format!("[interfaces] > [[{interface}]] > type"),
            None,
            "enabled interface is missing its required type",
            Some(SUPPORTED_INTERFACES.join(", ")),
            format!("add `type = AutoInterface` under [[{interface}]]"),
        ),
        ReferenceError::BadValue {
            interface,
            key,
            reason,
        } => ConfigDiagnostic::new(
            ConfigDiagnosticCode::InvalidValue,
            source,
            location(locations, &[section_key::INTERFACES, &interface, &key]),
            format!("[interfaces] > [[{interface}]] > {key}"),
            None,
            reason,
            None,
            format!("replace {key:?} with a valid value"),
        ),
        ReferenceError::BadGlobalValue { key, reason } => ConfigDiagnostic::new(
            ConfigDiagnosticCode::InvalidValue,
            source,
            location(locations, &[section_key::RETICULUM, &key]),
            format!("[reticulum] > {key}"),
            None,
            reason,
            None,
            format!("replace {key:?} with a valid value"),
        ),
        ReferenceError::BadPrnsValue { key, reason } => ConfigDiagnostic::new(
            ConfigDiagnosticCode::InvalidValue,
            source,
            location(locations, &[section_key::PRNS, &key]),
            format!("[prns] > {key}"),
            None,
            reason,
            Some(ValueKind::ByteQuantity.accepted().to_string()),
            format!("set `{key} = {}`", ValueKind::ByteQuantity.example()),
        ),
    }
}
