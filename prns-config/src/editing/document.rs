use std::fmt;
use std::ops::Range;

use crate::configobj::{ConfigDocument, ConfigError, Value};
use crate::reference::keys::{interface as interface_key, section as section_key};
use crate::{parse_and_plan_named, ConfigDiagnostic, ConfigErrors, InterfaceKind, SecretDisplay};

use super::catalog::ConfiguredInterfaceSetting;
use super::interface::{
    render_bool, render_value, InterfaceConfigKey, InterfaceDefinition, InterfaceName,
    InterfaceSetting, InterfaceSettingKey, RNodeMultiRadioDefinition,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredInterface {
    name: InterfaceName,
    configured_type: Option<String>,
    kind: Option<InterfaceKind>,
    enabled: Option<bool>,
    settings: Vec<ConfiguredInterfaceSetting>,
    rnode_multi_radios: Vec<RNodeMultiRadioDefinition>,
}

impl ConfiguredInterface {
    pub fn name(&self) -> &InterfaceName {
        &self.name
    }

    pub fn configured_type(&self) -> Option<&str> {
        self.configured_type.as_deref()
    }

    pub const fn kind(&self) -> Option<InterfaceKind> {
        self.kind
    }

    pub const fn enabled(&self) -> Option<bool> {
        self.enabled
    }

    pub fn settings(&self) -> &[ConfiguredInterfaceSetting] {
        &self.settings
    }

    pub fn rnode_multi_radios(&self) -> &[RNodeMultiRadioDefinition] {
        &self.rnode_multi_radios
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterfaceSettingChange {
    Set(InterfaceSetting),
    Remove(InterfaceSettingKey),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigEdit {
    Add(InterfaceDefinition),
    Replace {
        current: InterfaceName,
        replacement: InterfaceDefinition,
    },
    Rename {
        current: InterfaceName,
        replacement: InterfaceName,
    },
    SetEnabled {
        name: InterfaceName,
        enabled: bool,
    },
    SetType {
        name: InterfaceName,
        kind: InterfaceKind,
    },
    ChangeSettings {
        name: InterfaceName,
        changes: Vec<InterfaceSettingChange>,
    },
    ReplaceRNodeMultiRadios {
        name: InterfaceName,
        radios: Vec<RNodeMultiRadioDefinition>,
    },
    Remove(InterfaceName),
    RemoveInterfaceValue {
        name: InterfaceName,
        key: InterfaceConfigKey,
    },
    Batch(Vec<ConfigEdit>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditedConfig {
    original: String,
    candidate: String,
    warnings: Vec<ConfigDiagnostic>,
}

impl EditedConfig {
    pub fn original(&self) -> &str {
        &self.original
    }

    pub fn candidate(&self) -> &str {
        &self.candidate
    }

    pub fn warnings(&self) -> &[ConfigDiagnostic] {
        &self.warnings
    }

    pub fn diff(&self, secrets: SecretDisplay) -> String {
        render_diff(&self.original, &self.candidate, secrets)
    }
}

impl ConfigDocument {
    pub fn interfaces(&self) -> Vec<ConfiguredInterface> {
        self.child_section_names(&[section_key::INTERFACES])
            .into_iter()
            .filter_map(|name| {
                let name = InterfaceName::new(name).ok()?;
                let path = [section_key::INTERFACES, name.as_str()];
                let configured_type = self
                    .section_value(&path, interface_key::TYPE)
                    .and_then(Value::as_scalar)
                    .map(str::to_string);
                let kind = configured_type.as_deref().and_then(InterfaceKind::parse);
                let enabled = interface_enabled(self, &path);
                let settings = kind
                    .map(|kind| configured_settings(self, name.as_str(), kind))
                    .unwrap_or_default();
                let rnode_multi_radios = if kind == Some(InterfaceKind::RnodeMulti) {
                    configured_radios(self, name.as_str())
                } else {
                    Vec::new()
                };
                Some(ConfiguredInterface {
                    name,
                    configured_type,
                    kind,
                    enabled,
                    settings,
                    rnode_multi_radios,
                })
            })
            .collect()
    }

    pub fn edit(&self, edit: &ConfigEdit) -> Result<EditedConfig, ConfigEditError> {
        self.edit_named("<edited config>", edit)
    }

    pub fn edit_named(
        &self,
        source_name: impl Into<String>,
        edit: &ConfigEdit,
    ) -> Result<EditedConfig, ConfigEditError> {
        let candidate = mutate(self.source(), edit)?;
        let report =
            parse_and_plan_named(source_name, &candidate).map_err(ConfigEditError::Invalid)?;
        Ok(EditedConfig {
            original: self.source().to_string(),
            candidate,
            warnings: report.warnings,
        })
    }
}

#[derive(Debug)]
pub enum ConfigEditError {
    Syntax(ConfigError),
    InterfaceNotFound(InterfaceName),
    DuplicateInterface(InterfaceName),
    Invalid(ConfigErrors),
}

impl fmt::Display for ConfigEditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => error.fmt(formatter),
            Self::InterfaceNotFound(name) => write!(formatter, "interface {name} was not found"),
            Self::DuplicateInterface(name) => {
                write!(formatter, "interface {name} already exists")
            }
            Self::Invalid(errors) => errors.fmt(formatter),
        }
    }
}

impl std::error::Error for ConfigEditError {}

fn mutate(source: &str, edit: &ConfigEdit) -> Result<String, ConfigEditError> {
    match edit {
        ConfigEdit::Batch(edits) => {
            let mut candidate = source.to_string();
            for edit in edits {
                candidate = mutate(&candidate, edit)?;
            }
            Ok(candidate)
        }
        _ => mutate_one(
            ConfigDocument::parse(source).map_err(ConfigEditError::Syntax)?,
            edit,
        ),
    }
}

fn mutate_one(document: ConfigDocument, edit: &ConfigEdit) -> Result<String, ConfigEditError> {
    match edit {
        ConfigEdit::Add(definition) => add_interface(&document, definition),
        ConfigEdit::Replace {
            current,
            replacement,
        } => replace_interface(&document, current, replacement),
        ConfigEdit::Rename {
            current,
            replacement,
        } => rename_interface(&document, current, replacement),
        ConfigEdit::SetEnabled { name, enabled } => set_enabled(&document, name, *enabled),
        ConfigEdit::SetType { name, kind } => set_type(&document, name, *kind),
        ConfigEdit::ChangeSettings { name, changes } => change_settings(&document, name, changes),
        ConfigEdit::ReplaceRNodeMultiRadios { name, radios } => {
            replace_rnode_multi_radios(&document, name, radios)
        }
        ConfigEdit::Remove(name) => remove_interface(&document, name),
        ConfigEdit::RemoveInterfaceValue { name, key } => {
            remove_interface_value(&document, name, key)
        }
        ConfigEdit::Batch(edits) => {
            let mut candidate = document.source().to_string();
            for edit in edits {
                candidate = mutate(&candidate, edit)?;
            }
            Ok(candidate)
        }
    }
}

fn add_interface(
    document: &ConfigDocument,
    definition: &InterfaceDefinition,
) -> Result<String, ConfigEditError> {
    ensure_interface_absent(document, definition.name())?;
    let newline = document.newline();
    let rendered = definition.render(newline);
    let Some(interfaces) = document.section_range(&[section_key::INTERFACES]) else {
        let mut candidate = document.source().to_string();
        ensure_trailing_newline(&mut candidate, newline);
        candidate.push_str(&format!("[{}]{newline}{rendered}", section_key::INTERFACES));
        return Ok(candidate);
    };
    let insertion = interfaces.end;
    let mut block = String::new();
    if insertion > 0 && !document.source()[..insertion].ends_with(['\n', '\r']) {
        block.push_str(newline);
    }
    block.push_str(&rendered);
    Ok(replace_range(
        document.source(),
        insertion..insertion,
        &block,
    ))
}

fn replace_interface(
    document: &ConfigDocument,
    current: &InterfaceName,
    replacement: &InterfaceDefinition,
) -> Result<String, ConfigEditError> {
    let range = interface_range(document, current)?;
    if current != replacement.name() {
        ensure_interface_absent(document, replacement.name())?;
    }
    Ok(replace_range(
        document.source(),
        range,
        &replacement.render(document.newline()),
    ))
}

fn rename_interface(
    document: &ConfigDocument,
    current: &InterfaceName,
    replacement: &InterfaceName,
) -> Result<String, ConfigEditError> {
    ensure_interface_absent(document, replacement)?;
    let header = document
        .section_header_range(&[section_key::INTERFACES, current.as_str()])
        .ok_or_else(|| ConfigEditError::InterfaceNotFound(current.clone()))?;
    let original = &document.source()[header.clone()];
    let indent = original.len() - original.trim_start().len();
    let suffix = section_header_suffix(original, 2);
    let rendered = format!("{}[[{}]]{}", &original[..indent], replacement, suffix,);
    Ok(replace_range(document.source(), header, &rendered))
}

fn set_enabled(
    document: &ConfigDocument,
    name: &InterfaceName,
    enabled: bool,
) -> Result<String, ConfigEditError> {
    interface_range(document, name)?;
    let explicit_path = [
        section_key::INTERFACES,
        name.as_str(),
        interface_key::INTERFACE_ENABLED,
    ];
    let shorthand_path = [
        section_key::INTERFACES,
        name.as_str(),
        interface_key::ENABLED,
    ];
    let mut replacements = Vec::new();
    match document.key_range(&explicit_path) {
        Some(range) => replacements.push((
            range.clone(),
            render_key_line(
                document.source(),
                range,
                interface_key::INTERFACE_ENABLED,
                render_bool(enabled),
                document.newline(),
            ),
        )),
        None => {
            let insertion = key_insertion(document, name)?;
            replacements.push((
                insertion..insertion,
                format!(
                    "    {} = {}{}",
                    interface_key::INTERFACE_ENABLED,
                    render_bool(enabled),
                    document.newline(),
                ),
            ));
        }
    }
    if let Some(range) = document.key_range(&shorthand_path) {
        replacements.push((range, String::new()));
    }
    Ok(apply_replacements(document.source(), replacements))
}

fn set_type(
    document: &ConfigDocument,
    name: &InterfaceName,
    kind: InterfaceKind,
) -> Result<String, ConfigEditError> {
    interface_range(document, name)?;
    let path = [section_key::INTERFACES, name.as_str(), interface_key::TYPE];
    let replacement = match document.key_range(&path) {
        Some(range) => {
            let line = render_key_line(
                document.source(),
                range.clone(),
                interface_key::TYPE,
                kind.canonical_name(),
                document.newline(),
            );
            replace_range(document.source(), range, &line)
        }
        None => {
            let insertion = key_insertion(document, name)?;
            replace_range(
                document.source(),
                insertion..insertion,
                &format!(
                    "    {} = {}{}",
                    interface_key::TYPE,
                    kind.canonical_name(),
                    document.newline()
                ),
            )
        }
    };
    Ok(replacement)
}

fn change_settings(
    document: &ConfigDocument,
    name: &InterfaceName,
    changes: &[InterfaceSettingChange],
) -> Result<String, ConfigEditError> {
    interface_range(document, name)?;
    let mut candidate = document.source().to_string();
    for change in changes {
        let parsed = ConfigDocument::parse(&candidate).map_err(ConfigEditError::Syntax)?;
        match change {
            InterfaceSettingChange::Set(setting) => {
                let key = setting.key().canonical();
                candidate = remove_aliases(&parsed, name, key, false);
                let parsed = ConfigDocument::parse(&candidate).map_err(ConfigEditError::Syntax)?;
                let path = [section_key::INTERFACES, name.as_str(), key.as_str()];
                let value = render_value(setting.value());
                candidate = match parsed.key_range(&path) {
                    Some(range) => {
                        let line = render_key_line(
                            parsed.source(),
                            range.clone(),
                            key.as_str(),
                            &value,
                            parsed.newline(),
                        );
                        replace_range(parsed.source(), range, &line)
                    }
                    None => {
                        let insertion = key_insertion(&parsed, name)?;
                        replace_range(
                            parsed.source(),
                            insertion..insertion,
                            &format!("    {} = {}{}", key.as_str(), value, parsed.newline()),
                        )
                    }
                };
            }
            InterfaceSettingChange::Remove(key) => {
                candidate = remove_aliases(&parsed, name, key.canonical(), true);
            }
        }
    }
    Ok(candidate)
}

fn remove_aliases(
    document: &ConfigDocument,
    name: &InterfaceName,
    key: InterfaceSettingKey,
    include_canonical: bool,
) -> String {
    let aliases = key.aliases();
    let keys = if aliases.is_empty() {
        vec![key.as_str()]
    } else {
        aliases.to_vec()
    };
    let replacements = keys
        .into_iter()
        .filter(|candidate| include_canonical || *candidate != key.as_str())
        .filter_map(|candidate| {
            document
                .key_range(&[section_key::INTERFACES, name.as_str(), candidate])
                .map(|range| (range, String::new()))
        })
        .collect();
    apply_replacements(document.source(), replacements)
}

fn remove_interface_value(
    document: &ConfigDocument,
    name: &InterfaceName,
    key: &InterfaceConfigKey,
) -> Result<String, ConfigEditError> {
    interface_range(document, name)?;
    let path = [section_key::INTERFACES, name.as_str(), key.as_str()];
    Ok(match document.key_range(&path) {
        Some(range) => replace_range(document.source(), range, ""),
        None => document.source().to_string(),
    })
}

fn configured_settings(
    document: &ConfigDocument,
    name: &str,
    kind: InterfaceKind,
) -> Vec<ConfiguredInterfaceSetting> {
    let Some(section) = document
        .root()
        .section(section_key::INTERFACES)
        .and_then(|interfaces| interfaces.section(name))
    else {
        return Vec::new();
    };
    kind.setting_specs()
        .into_iter()
        .filter_map(|spec| {
            let key = spec.key();
            let aliases = key.aliases();
            let keys = if aliases.is_empty() {
                vec![key.as_str()]
            } else {
                aliases.to_vec()
            };
            keys.into_iter().find_map(|source_key| {
                let value = section.get(source_key)?;
                let value = match value {
                    Value::Scalar(value) => value.clone(),
                    Value::List(values) => values.join(", "),
                };
                Some(ConfiguredInterfaceSetting::new(
                    spec,
                    source_key.to_string(),
                    value,
                ))
            })
        })
        .collect()
}

fn configured_radios(document: &ConfigDocument, name: &str) -> Vec<RNodeMultiRadioDefinition> {
    let Some(section) = document
        .root()
        .section(section_key::INTERFACES)
        .and_then(|interfaces| interfaces.section(name))
    else {
        return Vec::new();
    };
    section
        .sections
        .iter()
        .filter_map(|(name, radio)| {
            let scalar = |key: &str| radio.get(key).and_then(Value::as_scalar);
            let enabled = scalar(interface_key::INTERFACE_ENABLED)
                .or_else(|| scalar(interface_key::ENABLED))
                .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
                    "yes" | "true" | "on" | "1" => Some(true),
                    "no" | "false" | "off" | "0" => Some(false),
                    _ => None,
                })
                .unwrap_or(true);
            if !enabled {
                return None;
            }
            RNodeMultiRadioDefinition::new(
                InterfaceName::new(name.clone()).ok()?,
                scalar(interface_key::VPORT)?.parse().ok()?,
                scalar(interface_key::FREQUENCY)?.parse().ok()?,
                scalar(interface_key::BANDWIDTH)?.parse().ok()?,
                scalar(interface_key::TXPOWER)?.parse().ok()?,
                scalar(interface_key::SPREADINGFACTOR)?.parse().ok()?,
                scalar(interface_key::CODINGRATE)?.parse().ok()?,
            )
            .ok()
        })
        .collect()
}

fn remove_interface(
    document: &ConfigDocument,
    name: &InterfaceName,
) -> Result<String, ConfigEditError> {
    let range = interface_range(document, name)?;
    Ok(replace_range(document.source(), range, ""))
}

fn replace_rnode_multi_radios(
    document: &ConfigDocument,
    name: &InterfaceName,
    radios: &[RNodeMultiRadioDefinition],
) -> Result<String, ConfigEditError> {
    let parent_path = [section_key::INTERFACES, name.as_str()];
    let parent = document
        .section_range(&parent_path)
        .ok_or_else(|| ConfigEditError::InterfaceNotFound(name.clone()))?;
    let child_ranges = document
        .child_section_names(&parent_path)
        .into_iter()
        .filter_map(|child| {
            document.section_range(&[section_key::INTERFACES, name.as_str(), child])
        })
        .collect::<Vec<_>>();
    let replacement = child_ranges
        .first()
        .zip(child_ranges.last())
        .map_or(parent.end..parent.end, |(first, last)| {
            first.start..last.end
        });
    let rendered = radios
        .iter()
        .map(|radio| radio.render(document.newline()))
        .collect::<String>();
    Ok(replace_range(document.source(), replacement, &rendered))
}

fn interface_range(
    document: &ConfigDocument,
    name: &InterfaceName,
) -> Result<Range<usize>, ConfigEditError> {
    document
        .section_range(&[section_key::INTERFACES, name.as_str()])
        .ok_or_else(|| ConfigEditError::InterfaceNotFound(name.clone()))
}

fn ensure_interface_absent(
    document: &ConfigDocument,
    name: &InterfaceName,
) -> Result<(), ConfigEditError> {
    if document
        .section_range(&[section_key::INTERFACES, name.as_str()])
        .is_some()
    {
        return Err(ConfigEditError::DuplicateInterface(name.clone()));
    }
    Ok(())
}

fn key_insertion(
    document: &ConfigDocument,
    name: &InterfaceName,
) -> Result<usize, ConfigEditError> {
    let path = [section_key::INTERFACES, name.as_str()];
    let section = document
        .section_range(&path)
        .ok_or_else(|| ConfigEditError::InterfaceNotFound(name.clone()))?;
    Ok(document
        .first_child_section_start(&path)
        .unwrap_or(section.end))
}

fn replace_range(source: &str, range: Range<usize>, replacement: &str) -> String {
    let mut candidate = String::with_capacity(source.len() + replacement.len());
    candidate.push_str(&source[..range.start]);
    candidate.push_str(replacement);
    candidate.push_str(&source[range.end..]);
    candidate
}

fn apply_replacements(source: &str, mut replacements: Vec<(Range<usize>, String)>) -> String {
    replacements.sort_by_key(|replacement| std::cmp::Reverse(replacement.0.start));
    let mut candidate = source.to_string();
    for (range, replacement) in replacements {
        candidate.replace_range(range, &replacement);
    }
    candidate
}

fn render_key_line(
    source: &str,
    range: Range<usize>,
    key: &str,
    value: &str,
    newline: &str,
) -> String {
    let line = &source[range];
    let indent_end = line.len() - line.trim_start().len();
    let suffix = inline_comment(line).unwrap_or_default();
    format!(
        "{}{} = {}{}{}",
        &line[..indent_end],
        key,
        value,
        suffix,
        newline
    )
}

fn inline_comment(line: &str) -> Option<&str> {
    let mut quote = None;
    for (index, character) in line.char_indices() {
        match (quote, character) {
            (None, '\'' | '"') => quote = Some(character),
            (Some(open), close) if open == close => quote = None,
            (None, '#') => {
                let start = line[..index]
                    .char_indices()
                    .rev()
                    .take_while(|(_, character)| character.is_whitespace())
                    .last()
                    .map_or(index, |(position, _)| position);
                return Some(line[start..].trim_end_matches(['\r', '\n']));
            }
            _ => {}
        }
    }
    None
}

fn section_header_suffix(header: &str, depth: usize) -> &str {
    let close = "]".repeat(depth);
    header
        .find(&close)
        .map(|position| &header[position + depth..])
        .unwrap_or_default()
}

fn interface_enabled(document: &ConfigDocument, path: &[&str]) -> Option<bool> {
    [interface_key::INTERFACE_ENABLED, interface_key::ENABLED]
        .into_iter()
        .filter_map(|key| document.section_value(path, key))
        .filter_map(Value::as_scalar)
        .filter_map(parse_bool)
        .reduce(|left, right| left || right)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Some(true),
        "no" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn ensure_trailing_newline(source: &mut String, newline: &str) {
    if !source.is_empty() && !source.ends_with(['\n', '\r']) {
        source.push_str(newline);
    }
}

fn render_diff(original: &str, candidate: &str, secrets: SecretDisplay) -> String {
    if original == candidate {
        return String::new();
    }
    let before = original.lines().collect::<Vec<_>>();
    let after = candidate.lines().collect::<Vec<_>>();
    let prefix = before
        .iter()
        .zip(&after)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let before_end = before.len().saturating_sub(suffix);
    let after_end = after.len().saturating_sub(suffix);
    let mut rendered = String::from("--- config\n+++ config\n");
    for line in display_lines(&before[prefix..before_end], secrets) {
        rendered.push('-');
        rendered.push_str(&line);
        rendered.push('\n');
    }
    for line in display_lines(&after[prefix..after_end], secrets) {
        rendered.push('+');
        rendered.push_str(&line);
        rendered.push('\n');
    }
    rendered
}

fn display_lines(lines: &[&str], secrets: SecretDisplay) -> Vec<String> {
    if secrets == SecretDisplay::Revealed {
        return lines.iter().map(|line| (*line).to_string()).collect();
    }
    let mut multiline_secret = None;
    let mut displayed = Vec::with_capacity(lines.len());
    for line in lines {
        if let Some(delimiter) = multiline_secret {
            displayed.push("<redacted>".to_string());
            if line.contains(delimiter) {
                multiline_secret = None;
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            displayed.push((*line).to_string());
            continue;
        };
        if !matches!(key.trim(), "pass_phrase" | "passphrase" | "rpc_key") {
            displayed.push((*line).to_string());
            continue;
        }
        let value = value.trim_start();
        multiline_secret = ["\"\"\"", "'''"].into_iter().find(|delimiter| {
            value.starts_with(delimiter) && value.matches(delimiter).count() == 1
        });
        displayed.push(format!("{} = <redacted>", key.trim_end()));
    }
    displayed
}
