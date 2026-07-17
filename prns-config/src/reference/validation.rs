use crate::configobj::{Section, SourceLocations, Value};
use crate::diagnostic::{ConfigDiagnostic, ConfigDiagnosticCode, ConfigSeverity};

use super::interpret::{cleaned_number, parse_bool, parse_identity_hash, ReferenceError};
use super::schema::{
    interface_key_rule, known_interface_keys, KeyRule, ValueKind, GLOBAL_RULES, LOGGING_RULES,
    SUPPORTED_INTERFACES,
};

#[derive(Default)]
pub(super) struct ValidationWarnings(Vec<ConfigDiagnostic>);

impl ValidationWarnings {
    fn push(&mut self, diagnostic: ConfigDiagnostic) {
        assert_eq!(diagnostic.severity(), ConfigSeverity::Warning);
        self.0.push(diagnostic);
    }

    pub(super) fn into_inner(self) -> Vec<ConfigDiagnostic> {
        self.0
    }
}

#[derive(Default)]
struct ValidationErrorCollector(Vec<ConfigDiagnostic>);

impl ValidationErrorCollector {
    fn push(&mut self, diagnostic: ConfigDiagnostic) {
        assert_eq!(diagnostic.severity(), ConfigSeverity::Error);
        self.0.push(diagnostic);
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
        errors: ValidationErrors,
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

    for (key, value) in &root.scalars {
        let line = location(locations, &[key]);
        if global_keys.contains(&key.as_str()) {
            errors.push(ConfigDiagnostic::new(
                ConfigDiagnosticCode::MisplacedKey,
                source,
                line,
                format!("<root> > {key}"),
                Some(value_text(value)),
                format!("global setting {key:?} is outside [reticulum] and will not be applied"),
                Some(format!("{key} must be under [reticulum]")),
                format!("move `{key} = {}` into [reticulum]", value_text(value)),
            ));
        } else if logging_keys.contains(&key.as_str()) {
            errors.push(ConfigDiagnostic::new(
                ConfigDiagnosticCode::MisplacedKey,
                source,
                line,
                format!("<root> > {key}"),
                Some(value_text(value)),
                format!("logging setting {key:?} is outside [logging] and will not be applied"),
                Some(format!("{key} must be under [logging]")),
                format!("move `{key} = {}` into [logging]", value_text(value)),
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
            "reticulum" => validate_section(
                source,
                "[reticulum]",
                &["reticulum"],
                section,
                GLOBAL_RULES,
                locations,
                &mut warnings,
                &mut errors,
            ),
            "logging" => validate_section(
                source,
                "[logging]",
                &["logging"],
                section,
                LOGGING_RULES,
                locations,
                &mut warnings,
                &mut errors,
            ),
            "interfaces" => {
                validate_interfaces(source, section, locations, &mut warnings, &mut errors)
            }
            _ => {
                let known = ["reticulum", "logging", "interfaces"];
                let suggestion = closest(name, &known);
                warnings.push(ConfigDiagnostic::new(
                    ConfigDiagnosticCode::UnknownSection,
                    source,
                    location(locations, &[name]),
                    format!("[{name}]"),
                    Some(name.clone()),
                    "unknown top-level section; its settings will not be applied",
                    Some("[reticulum], [logging], or [interfaces]".to_string()),
                    suggestion.map_or_else(
                        || format!("remove [{name}] or move its settings into a stock section"),
                        |expected| format!("rename [{name}] to [{expected}]"),
                    ),
                ));
            }
        }
    }

    match errors.finish() {
        Some(errors) => ValidationResult::Invalid { errors, warnings },
        None => ValidationResult::Valid { warnings },
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_section(
    source: &str,
    display_path: &str,
    source_path: &[&str],
    section: &Section,
    rules: &[(&str, ValueKind)],
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
            Some((_, kind)) => validate_value(
                source,
                line,
                format!("{display_path} > {key}"),
                key,
                value,
                *kind,
                errors,
            ),
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
        warnings.push(ConfigDiagnostic::new(
            ConfigDiagnosticCode::UnknownSection,
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
    for (key, value) in &interfaces.scalars {
        warnings.push(unknown_key(
            source,
            location(locations, &["interfaces", key]),
            format!("[interfaces] > {key}"),
            key,
            value,
            &[],
        ));
    }
    for (name, section) in &interfaces.sections {
        validate_interface(source, name, section, locations, warnings, errors);
    }
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
    let enabled = validate_alias_group(
        source,
        name,
        section,
        locations,
        "interface_enabled",
        &["interface_enabled", "enabled"],
        ValueKind::Bool,
        warnings,
        errors,
    );
    if enabled.as_deref() != Some("true") {
        return;
    }

    let type_value = section.get("type");
    let Some(type_name) = type_value
        .and_then(Value::as_scalar)
        .filter(|name| !name.is_empty())
    else {
        errors.push(ConfigDiagnostic::new(
            ConfigDiagnosticCode::MissingRequiredKey,
            source,
            location(locations, &["interfaces", name]),
            format!("{interface_path} > type"),
            type_value.map(value_text),
            "enabled interface is missing its required type",
            Some(SUPPORTED_INTERFACES.join(", ")),
            format!("add `type = AutoInterface` under [[{name}]], or set `enabled = No`"),
        ));
        return;
    };

    if !SUPPORTED_INTERFACES.contains(&type_name) {
        errors.push(ConfigDiagnostic::new(
            ConfigDiagnosticCode::UnsupportedInterface,
            source,
            location(locations, &["interfaces", name, "type"]),
            format!("{interface_path} > type"),
            Some(type_name.to_string()),
            format!("interface type {type_name:?} is not available in this build"),
            Some(SUPPORTED_INTERFACES.join(", ")),
            format!("set `enabled = No` for [[{name}]] until {type_name} support is installed"),
        ));
        return;
    }

    validate_alias_group(
        source,
        name,
        section,
        locations,
        "interface_mode",
        &["interface_mode", "mode"],
        ValueKind::Mode,
        warnings,
        errors,
    );
    validate_alias_group(
        source,
        name,
        section,
        locations,
        "network_name",
        &["network_name", "networkname"],
        ValueKind::String,
        warnings,
        errors,
    );
    validate_alias_group(
        source,
        name,
        section,
        locations,
        "pass_phrase",
        &["pass_phrase", "passphrase"],
        ValueKind::String,
        warnings,
        errors,
    );

    let discoverable = section
        .get("discoverable")
        .and_then(Value::as_scalar)
        .and_then(parse_bool)
        == Some(true);
    let alias_keys = [
        "interface_enabled",
        "enabled",
        "interface_mode",
        "mode",
        "network_name",
        "networkname",
        "pass_phrase",
        "passphrase",
    ];
    let known = known_interface_keys(type_name);
    for (key, value) in &section.scalars {
        if alias_keys.contains(&key.as_str()) {
            continue;
        }
        let line = location(locations, &["interfaces", name, key]);
        match interface_key_rule(type_name, key, discoverable) {
            Some(KeyRule::Validate(kind)) => validate_value(
                source,
                line,
                format!("{interface_path} > {key}"),
                key,
                value,
                kind,
                errors,
            ),
            Some(KeyRule::Recognized) => {}
            None => warnings.push(unknown_key(
                source,
                line,
                format!("{interface_path} > {key}"),
                key,
                value,
                &known,
            )),
        }
    }

    match type_name {
        "TCPServerInterface" => compare_alias_pair(
            source,
            name,
            section,
            locations,
            "port",
            "listen_port",
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
                "port",
                "listen_port",
                ValueKind::U16,
                warnings,
                errors,
            );
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                "port",
                "forward_port",
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
                "remote",
                "target_host",
                ValueKind::String,
                warnings,
                errors,
            );
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                "listen_on",
                "listen_ip",
                ValueKind::String,
                warnings,
                errors,
            );
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                "port",
                "listen_port",
                ValueKind::U16,
                warnings,
                errors,
            );
            compare_alias_pair(
                source,
                name,
                section,
                locations,
                "port",
                "target_port",
                ValueKind::U16,
                warnings,
                errors,
            );
        }
        _ => {}
    }

    if type_name == "RNodeInterface" {
        if let Some((_, value)) = section.scalars.iter().find(|(key, _)| key == "port") {
            if let Some(port) = value.as_scalar() {
                let transport = port.trim().to_ascii_lowercase();
                if transport.starts_with("tcp://") || transport.starts_with("ble://") {
                    errors.push(ConfigDiagnostic::new(
                        ConfigDiagnosticCode::UnsupportedTransport,
                        source,
                        location(locations, &["interfaces", name, "port"]),
                        format!("{interface_path} > port"),
                        Some(port.to_string()),
                        "this RNode URI transport is not available in this build",
                        Some("a local serial device path".to_string()),
                        format!("set `port = /dev/ttyUSB0` for [[{name}]], or set `enabled = No`"),
                    ));
                }
            }
        }
    }

    for (child, _) in &section.sections {
        warnings.push(ConfigDiagnostic::new(
            ConfigDiagnosticCode::UnknownSection,
            source,
            location(locations, &["interfaces", name, child]),
            format!("{interface_path} > [[[{child}]]]"),
            Some(child.clone()),
            "nested interface sections are not supported by this interface type",
            None,
            format!("remove [[[{child}]]] from [[{name}]]"),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_alias_group(
    source: &str,
    interface: &str,
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
                location(locations, &["interfaces", interface, key]),
                format!("[interfaces] > [[{interface}]] > {key}"),
                key,
                value,
                kind,
                errors,
            ),
        }
    }
    let first = values.first()?;
    if let Some(second) = values.get(1) {
        let (code, message) = if first.2 == second.2 {
            (
                ConfigDiagnosticCode::RedundantAliases,
                format!(
                    "{0:?} and {1:?} specify the same setting",
                    first.0, second.0
                ),
            )
        } else {
            (
                ConfigDiagnosticCode::ConflictingAliases,
                format!(
                    "{0:?} and {1:?} specify different values",
                    first.0, second.0
                ),
            )
        };
        let diagnostic = ConfigDiagnostic::new(
            code,
            source,
            location(locations, &["interfaces", interface, second.0]),
            format!("[interfaces] > [[{interface}]] > {canonical}"),
            Some(format!(
                "{} = {}; {} = {}",
                first.0,
                value_text(first.1),
                second.0,
                value_text(second.1),
            )),
            message,
            Some(kind.accepted().to_string()),
            format!("keep only `{canonical} = {}`", first.2),
        );
        if code.severity() == ConfigSeverity::Warning {
            warnings.push(diagnostic);
        } else {
            errors.push(diagnostic);
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
    let (code, message) = if canonical_normalized == alias_normalized {
        (
            ConfigDiagnosticCode::RedundantAliases,
            format!("{canonical:?} and {alias:?} specify the same setting"),
        )
    } else {
        (
            ConfigDiagnosticCode::ConflictingAliases,
            format!("{canonical:?} and {alias:?} specify different values"),
        )
    };
    let diagnostic = ConfigDiagnostic::new(
        code,
        source,
        location(locations, &["interfaces", interface, alias]),
        format!("[interfaces] > [[{interface}]] > {canonical}"),
        Some(format!(
            "{canonical} = {}; {alias} = {}",
            value_text(canonical_value),
            value_text(alias_value),
        )),
        message,
        Some(kind.accepted().to_string()),
        format!("keep only {canonical} = {canonical_normalized}"),
    );
    if code.severity() == ConfigSeverity::Warning {
        warnings.push(diagnostic);
    } else {
        errors.push(diagnostic);
    }
}

fn validate_value(
    source: &str,
    line: usize,
    path: String,
    key: &str,
    value: &Value,
    kind: ValueKind,
    errors: &mut ValidationErrorCollector,
) {
    if normalized_value(value, kind).is_ok() {
        return;
    }
    errors.push(ConfigDiagnostic::new(
        ConfigDiagnosticCode::InvalidValue,
        source,
        line,
        path,
        Some(value_text(value)),
        format!("invalid value for {key:?}"),
        Some(kind.accepted().to_string()),
        format!("set `{key} = {}`", kind.example()),
    ));
}

fn normalized_value(value: &Value, kind: ValueKind) -> Result<String, ()> {
    if matches!(kind, ValueKind::List) {
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
            _ => return Err(()),
        }
        .to_string(),
        ValueKind::String => text.to_string(),
        ValueKind::List => unreachable!("list values return before scalar coercion"),
        ValueKind::U64 => parse_integer::<u64>(text)?.to_string(),
        ValueKind::U32 => parse_integer::<u32>(text)?.to_string(),
        ValueKind::U16 => parse_integer::<u16>(text)?.to_string(),
        ValueKind::U8 => parse_integer::<u8>(text)?.to_string(),
        ValueKind::I16 => parse_integer::<i16>(text)?.to_string(),
        ValueKind::I64 => parse_integer::<i64>(text)?.to_string(),
        ValueKind::Usize => parse_integer::<usize>(text)?.to_string(),
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

fn unknown_key(
    source: &str,
    line: usize,
    path: String,
    key: &str,
    value: &Value,
    known: &[&str],
) -> ConfigDiagnostic {
    let suggestion = closest(key, known);
    ConfigDiagnostic::new(
        ConfigDiagnosticCode::UnknownKey,
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

fn location(locations: &SourceLocations, path: &[&str]) -> usize {
    locations.line(path.iter().copied()).unwrap_or(1)
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
            location(locations, &["interfaces", &interface]),
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
            location(locations, &["interfaces", &interface, &key]),
            format!("[interfaces] > [[{interface}]] > {key}"),
            None,
            reason,
            None,
            format!("replace {key:?} with a valid value"),
        ),
        ReferenceError::BadGlobalValue { key, reason } => ConfigDiagnostic::new(
            ConfigDiagnosticCode::InvalidValue,
            source,
            location(locations, &["reticulum", &key]),
            format!("[reticulum] > {key}"),
            None,
            reason,
            None,
            format!("replace {key:?} with a valid value"),
        ),
    }
}
