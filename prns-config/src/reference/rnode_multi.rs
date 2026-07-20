use std::str::FromStr;

use crate::configobj::{Section, SourceLocations, Value};
use prns_core::interfaces::rnode::multi::{
    HIGH_FREQUENCY_MAX_HZ, HIGH_FREQUENCY_MIN_HZ, LOW_FREQUENCY_MAX_HZ, LOW_FREQUENCY_MIN_HZ,
    MAX_SUBINTERFACES, TX_POWER_MIN_DBM,
};
use prns_core::interfaces::rnode::protocol::TXPOWER_DBM_MAX;

use super::diagnostics::{ErrorCode, ErrorDiagnostic, WarningCode, WarningDiagnostic};
use super::interpret::cleaned_number;
use super::keys::{interface as interface_key, section as section_key};
use super::schema::{rnode_multi_subinterface_key_rule, ValueKind};
use super::validation::{
    location, setting_location, unknown_key, validate_alias_group, validate_value,
    ValidationErrorCollector, ValidationWarnings,
};

const REQUIRED_RADIO_SETTINGS: &[RequiredRadioSetting] = &[
    RequiredRadioSetting {
        key: interface_key::VPORT,
        accepted: "a virtual port from 0 through 10",
        example: "0",
    },
    RequiredRadioSetting {
        key: interface_key::FREQUENCY,
        accepted: "a frequency supported by an RNodeMulti radio",
        example: "868000000",
    },
    RequiredRadioSetting {
        key: interface_key::BANDWIDTH,
        accepted: "a radio bandwidth",
        example: "125000",
    },
    RequiredRadioSetting {
        key: interface_key::TXPOWER,
        accepted: "a transmit power",
        example: "7",
    },
    RequiredRadioSetting {
        key: interface_key::SPREADINGFACTOR,
        accepted: "a LoRa spreading factor",
        example: "8",
    },
    RequiredRadioSetting {
        key: interface_key::CODINGRATE,
        accepted: "a LoRa coding rate",
        example: "5",
    },
];

struct RequiredRadioSetting {
    key: &'static str,
    accepted: &'static str,
    example: &'static str,
}

struct SubinterfaceContext<'a> {
    source: &'a str,
    parent: &'a str,
    name: &'a str,
    section: &'a Section,
    locations: &'a SourceLocations,
}

impl<'a> SubinterfaceContext<'a> {
    fn into_enabled(
        self,
        warnings: &mut ValidationWarnings,
        errors: &mut ValidationErrorCollector,
    ) -> Option<EnabledSubinterface<'a>> {
        let source_path = self.source_path();
        let display_path = self.display_path();
        let enabled = validate_alias_group(
            self.source,
            &source_path,
            &display_path,
            self.section,
            self.locations,
            interface_key::INTERFACE_ENABLED,
            interface_key::ENABLED_ALIASES,
            ValueKind::Bool,
            warnings,
            errors,
        );
        (enabled.as_deref() == Some("true")).then_some(EnabledSubinterface {
            context: self,
            source_path,
            display_path,
        })
    }

    fn source_path(&self) -> [&'a str; 3] {
        [section_key::INTERFACES, self.parent, self.name]
    }

    fn display_path(&self) -> String {
        format!(
            "[interfaces] > [[{parent}]] > [[[{name}]]]",
            parent = self.parent,
            name = self.name
        )
    }
}

struct EnabledSubinterface<'a> {
    context: SubinterfaceContext<'a>,
    source_path: [&'a str; 3],
    display_path: String,
}

impl EnabledSubinterface<'_> {
    fn validate(
        &self,
        assigned_vports: &mut AssignedVports,
        warnings: &mut ValidationWarnings,
        errors: &mut ValidationErrorCollector,
    ) {
        self.validate_values(warnings, errors);
        self.require_radio_settings(errors);
        self.register_vport(assigned_vports, errors);
        self.warn_about_nested_sections(warnings);
    }

    fn validate_values(
        &self,
        warnings: &mut ValidationWarnings,
        errors: &mut ValidationErrorCollector,
    ) {
        for (key, value) in &self.context.section.scalars {
            if interface_key::ENABLED_ALIASES.contains(&key.as_str()) {
                continue;
            }
            match rnode_multi_subinterface_key_rule(key) {
                Some(rule) => validate_value(
                    self.context.source,
                    setting_location(self.context.locations, &self.source_path, key),
                    format!("{} > {key}", self.display_path),
                    key,
                    value,
                    rule.value_kind(),
                    errors,
                ),
                None => warnings.push(unknown_key(
                    self.context.source,
                    setting_location(self.context.locations, &self.source_path, key),
                    format!("{} > {key}", self.display_path),
                    key,
                    value,
                    interface_key::RNODE_MULTI_SUBINTERFACE,
                )),
            }
        }
    }

    fn require_radio_settings(&self, errors: &mut ValidationErrorCollector) {
        for required in REQUIRED_RADIO_SETTINGS {
            if self.context.section.get(required.key).is_some() {
                continue;
            }
            errors.push(ErrorDiagnostic::new(
                ErrorCode::MissingRequiredKey,
                self.context.source,
                location(self.context.locations, &self.source_path),
                format!("{} > {}", self.display_path, required.key),
                None,
                format!(
                    "enabled RNodeMulti subinterface is missing {}",
                    required.accepted
                ),
                Some(required.accepted.to_string()),
                format!(
                    "add `{} = {}` under {}",
                    required.key, required.example, self.display_path
                ),
            ));
        }
    }

    fn register_vport(
        &self,
        assigned_vports: &mut AssignedVports,
        errors: &mut ValidationErrorCollector,
    ) {
        let Some(vport) = self
            .context
            .section
            .get(interface_key::VPORT)
            .and_then(Value::as_scalar)
            .and_then(VirtualPort::parse)
        else {
            return;
        };
        let Err(duplicate) = assigned_vports.register(vport, self.context.name) else {
            return;
        };
        errors.push(ErrorDiagnostic::new(
            ErrorCode::InvalidValue,
            self.context.source,
            setting_location(
                self.context.locations,
                &self.source_path,
                interface_key::VPORT,
            ),
            format!("{} > {}", self.display_path, interface_key::VPORT),
            Some(vport.value().to_string()),
            format!(
                "vport {} is already assigned to subinterface {:?}",
                vport.value(),
                duplicate.first
            ),
            Some("a unique integer from 0 through 10".to_string()),
            format!(
                "set `{} = {}`",
                interface_key::VPORT,
                duplicate.replacement.value()
            ),
        ));
    }

    fn warn_about_nested_sections(&self, warnings: &mut ValidationWarnings) {
        for (nested, _) in &self.context.section.sections {
            warnings.push(WarningDiagnostic::new(
                WarningCode::UnknownSection,
                self.context.source,
                location(
                    self.context.locations,
                    &[
                        section_key::INTERFACES,
                        self.context.parent,
                        self.context.name,
                        nested,
                    ],
                ),
                format!("{} > [[[[{nested}]]]]", self.display_path),
                Some(nested.clone()),
                "sections cannot be nested beneath an RNodeMulti subinterface",
                None,
                format!("remove [[[[{nested}]]]] from [[[{}]]]", self.context.name),
            ));
        }
    }
}

#[derive(Default)]
struct AssignedVports([Option<String>; MAX_SUBINTERFACES]);

impl AssignedVports {
    fn register(&mut self, vport: VirtualPort, subinterface: &str) -> Result<(), DuplicateVport> {
        let first = match &self.0[vport.index()] {
            Some(first) => first.clone(),
            None => {
                self.0[vport.index()] = Some(subinterface.to_string());
                return Ok(());
            }
        };
        let replacement = self
            .0
            .iter()
            .position(Option::is_none)
            .and_then(|index| u8::try_from(index).ok())
            .map(VirtualPort)
            .unwrap_or(vport);
        Err(DuplicateVport { first, replacement })
    }
}

struct DuplicateVport {
    first: String,
    replacement: VirtualPort,
}

#[derive(Clone, Copy)]
struct VirtualPort(u8);

impl VirtualPort {
    fn parse(value: &str) -> Option<Self> {
        parse_number(value)
            .filter(|value| usize::from(*value) < MAX_SUBINTERFACES)
            .map(Self)
    }

    fn index(self) -> usize {
        usize::from(self.0)
    }

    fn value(self) -> u8 {
        self.0
    }
}

pub(super) fn validate_subinterfaces(
    source: &str,
    parent: &str,
    section: &Section,
    locations: &SourceLocations,
    warnings: &mut ValidationWarnings,
    errors: &mut ValidationErrorCollector,
) {
    let mut assigned_vports = AssignedVports::default();
    let mut has_enabled_subinterface = false;
    for (name, child) in &section.sections {
        let context = SubinterfaceContext {
            source,
            parent,
            name,
            section: child,
            locations,
        };
        let Some(subinterface) = context.into_enabled(warnings, errors) else {
            continue;
        };
        has_enabled_subinterface = true;
        subinterface.validate(&mut assigned_vports, warnings, errors);
    }
    if !has_enabled_subinterface {
        require_enabled_subinterface(source, parent, section, locations, errors);
    }
}

fn require_enabled_subinterface(
    source: &str,
    parent: &str,
    section: &Section,
    locations: &SourceLocations,
    errors: &mut ValidationErrorCollector,
) {
    let reason = if section.sections.is_empty() {
        "enabled RNodeMulti interface has no configured subinterfaces"
    } else {
        "enabled RNodeMulti interface has no enabled subinterfaces"
    };
    errors.push(ErrorDiagnostic::new(
        ErrorCode::MissingRequiredKey,
        source,
        location(locations, &[section_key::INTERFACES, parent]),
        format!("[interfaces] > [[{parent}]] > [[[subinterface]]]"),
        None,
        reason,
        Some("at least one enabled [[[subinterface]]] section".to_string()),
        format!(
            "add `[[[Radio 0]]]` under [[{parent}]] with `interface_enabled = Yes`, `vport = 0`, and its radio settings"
        ),
    ));
}

pub(super) fn semantic_value_is_valid(kind: ValueKind, text: &str) -> Option<bool> {
    match kind {
        ValueKind::RnodeMultiVport => Some(VirtualPort::parse(text).is_some()),
        ValueKind::RnodeMultiFrequency => Some(parse_number::<u64>(text).is_some_and(|value| {
            (u64::from(LOW_FREQUENCY_MIN_HZ)..=u64::from(LOW_FREQUENCY_MAX_HZ)).contains(&value)
                || (u64::from(HIGH_FREQUENCY_MIN_HZ)..=u64::from(HIGH_FREQUENCY_MAX_HZ))
                    .contains(&value)
        })),
        ValueKind::RnodeMultiTxPower => Some(
            parse_number::<i16>(text)
                .is_some_and(|value| (TX_POWER_MIN_DBM..=TXPOWER_DBM_MAX).contains(&value)),
        ),
        _ => None,
    }
}

fn parse_number<T>(text: &str) -> Option<T>
where
    T: FromStr,
{
    cleaned_number(text.trim())?.parse().ok()
}
