use std::collections::BTreeMap;
use std::io::IsTerminal;

use prns_config::editing::{
    ConfigEdit, ConfigFile, ConfiguredInterface, InterfaceDefinition, InterfaceName,
    InterfaceSetting, InterfaceSettingChange, InterfaceSettingCondition, InterfaceSettingKey,
    InterfaceSettingSpec, InterfaceSettingTier, InterfaceSettingValue, RNodeMultiRadioDefinition,
};
use prns_config::{parse_and_plan_named, InterfaceKind, PlannedInterface};

use super::error::InterfacesError;
use super::presentation::Presentation;
use super::prompt;

pub(super) fn edit_interface(
    file: &ConfigFile,
    interface: &ConfiguredInterface,
    show_secrets: bool,
) -> Result<Option<ConfigEdit>, InterfacesError> {
    let Some(kind) = interface.kind() else {
        println!("This interface type is unknown to Prns. Its settings remain untouched.");
        return Ok(None);
    };
    let planned = parse_and_plan_named(file.path().display().to_string(), file.document().source())
        .ok()
        .and_then(|report| {
            report
                .value
                .interfaces
                .into_iter()
                .find(|planned| planned.name == interface.name().as_str())
        });
    let mut draft = SettingDraft::from_interface(kind, interface, planned, show_secrets);
    loop {
        if !draft.run()? {
            return Ok(None);
        }
        let mut edits = Vec::new();
        let changes = draft.changes();
        if !changes.is_empty() {
            edits.push(ConfigEdit::ChangeSettings {
                name: interface.name().clone(),
                changes,
            });
        }
        if draft.radios_changed {
            edits.push(ConfigEdit::ReplaceRNodeMultiRadios {
                name: interface.name().clone(),
                radios: draft.radios.clone(),
            });
        }
        if edits.is_empty() {
            println!("No setting changes selected.");
            return Ok(None);
        }
        let edit = ConfigEdit::Batch(edits);
        match file
            .document()
            .edit_named(file.path().display().to_string(), &edit)
        {
            Ok(_) => return Ok(Some(edit)),
            Err(error) => {
                println!("The selected settings do not produce a valid configuration:");
                println!("  {error}");
                println!("Adjust the highlighted settings or choose Back to discard the draft.");
            }
        }
    }
}

pub(super) fn add_interface(
    kind: InterfaceKind,
    name: InterfaceName,
    show_secrets: bool,
    settings: Vec<InterfaceSetting>,
    radios: Vec<RNodeMultiRadioDefinition>,
) -> Result<Option<InterfaceDefinition>, InterfacesError> {
    let mut draft = SettingDraft::new(kind, show_secrets, settings, radios);
    loop {
        if !draft.run()? {
            return Ok(None);
        }
        let settings = draft
            .staged
            .values()
            .filter_map(Clone::clone)
            .collect::<Vec<_>>();
        match InterfaceDefinition::new_named_with_rnode_multi_radios(
            format!("new interface {name}"),
            name.clone(),
            kind,
            true,
            settings,
            draft.radios.clone(),
        ) {
            Ok(definition) => return Ok(Some(definition)),
            Err(error) => {
                let presentation =
                    Presentation::new(crate::terminal::enabled(std::io::stdout().is_terminal()));
                println!();
                println!(
                    "{}",
                    presentation
                        .error("More information is required before this interface can be saved.")
                );
                println!("  {error}");
                println!();
            }
        }
    }
}

struct SettingDraft {
    kind: InterfaceKind,
    current: BTreeMap<InterfaceSettingKey, String>,
    staged: BTreeMap<InterfaceSettingKey, Option<InterfaceSetting>>,
    planned: Option<PlannedInterface>,
    radios: Vec<RNodeMultiRadioDefinition>,
    radios_changed: bool,
    show_secrets: bool,
    show_advanced: bool,
}

impl SettingDraft {
    fn new(
        kind: InterfaceKind,
        show_secrets: bool,
        settings: Vec<InterfaceSetting>,
        radios: Vec<RNodeMultiRadioDefinition>,
    ) -> Self {
        Self {
            kind,
            current: BTreeMap::new(),
            staged: settings
                .into_iter()
                .map(|setting| (setting.key(), Some(setting)))
                .collect(),
            planned: None,
            radios,
            radios_changed: false,
            show_secrets,
            show_advanced: false,
        }
    }

    fn from_interface(
        kind: InterfaceKind,
        interface: &ConfiguredInterface,
        planned: Option<PlannedInterface>,
        show_secrets: bool,
    ) -> Self {
        let current = interface
            .settings()
            .iter()
            .map(|setting| (setting.spec().key(), setting.value().to_string()))
            .collect();
        Self {
            kind,
            current,
            staged: BTreeMap::new(),
            planned,
            radios: interface.rnode_multi_radios().to_vec(),
            radios_changed: false,
            show_secrets,
            show_advanced: false,
        }
    }

    fn run(&mut self) -> Result<bool, InterfacesError> {
        loop {
            let specs = self.ordered_specs();
            self.print(&specs);
            let selection = prompt(if self.kind == InterfaceKind::RnodeMulti {
                if self.show_advanced {
                    "Setting number, [A] Everyday settings, [M] Radio members, [F] Finish, [B] Back"
                } else {
                    "Setting number, [A] All settings, [M] Radio members, [F] Finish, [B] Back"
                }
            } else if self.show_advanced {
                "Setting number, [A] Everyday settings, [F] Finish, [B] Back"
            } else {
                "Setting number, [A] All settings, [F] Finish, [B] Back"
            })?;
            match selection.trim().to_ascii_lowercase().as_str() {
                "f" | "finish" => return Ok(true),
                "b" | "back" | "" => return Ok(false),
                "a" | "all" | "advanced" | "common" | "everyday" => {
                    self.show_advanced = !self.show_advanced;
                }
                "m" | "members" | "radios" if self.kind == InterfaceKind::RnodeMulti => {
                    if edit_radios(&mut self.radios)? {
                        self.radios_changed = true;
                    }
                }
                value => {
                    let index = value.parse::<usize>().map_err(|_| {
                        InterfacesError::Usage(super::error::InterfacesUsageError::InvalidSelection)
                    })?;
                    let Some(spec) = specs.get(index.saturating_sub(1)).copied() else {
                        return Err(InterfacesError::Usage(
                            super::error::InterfacesUsageError::MissingSelection,
                        ));
                    };
                    match self.edit_setting(spec) {
                        Ok(()) => {}
                        Err(InterfacesError::InterfaceSettingInput(error)) => {
                            let presentation = Presentation::new(crate::terminal::enabled(
                                std::io::stdout().is_terminal(),
                            ));
                            println!("{}", presentation.error(error.to_string()));
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }

    fn ordered_specs(&self) -> Vec<InterfaceSettingSpec> {
        let mut specs = self
            .kind
            .setting_specs()
            .into_iter()
            .filter(|spec| spec.is_supported(self.kind) || self.has_value(spec.key()))
            .filter(|spec| {
                self.show_advanced
                    || spec.tier() == InterfaceSettingTier::Standard
                    || self.has_value(spec.key())
            })
            .collect::<Vec<_>>();
        specs.sort_by_key(|spec| {
            (
                spec.category(),
                !self.has_value(spec.key()),
                spec.tier() == InterfaceSettingTier::Advanced,
                spec.key().as_str(),
            )
        });
        specs
    }

    fn print(&self, specs: &[InterfaceSettingSpec]) {
        let presentation =
            Presentation::new(crate::terminal::enabled(std::io::stdout().is_terminal()));
        println!();
        println!(
            "{}",
            presentation.muted(
                "Configured values come from this stanza; defaults come from the interface; effective values include inherited node policy."
            )
        );
        if !self.show_advanced {
            println!(
                "{}",
                presentation.muted(
                    "Showing everyday settings plus anything already configured. Choose All settings for advanced controls."
                )
            );
        }
        let mut category = None;
        for (index, spec) in specs.iter().enumerate() {
            if category != Some(spec.category()) {
                category = Some(spec.category());
                println!();
                println!("  {}", spec.category());
            }
            let value = self.display_value(*spec);
            println!("    {:>2}. {:<34} {}", index + 1, spec.label(), value);
        }
        let hidden = self
            .kind
            .setting_specs()
            .into_iter()
            .filter(|spec| spec.is_supported(self.kind))
            .filter(|spec| spec.tier() == InterfaceSettingTier::Advanced)
            .filter(|spec| !self.has_value(spec.key()))
            .count();
        if !self.show_advanced && hidden != 0 {
            println!();
            println!(
                "  {}",
                presentation.muted(format!("{hidden} advanced settings hidden"))
            );
        }
        if self.kind == InterfaceKind::RnodeMulti {
            println!();
            println!("  Radio members: {}", self.radios.len());
        }
        println!();
    }

    fn edit_setting(&mut self, spec: InterfaceSettingSpec) -> Result<(), InterfacesError> {
        let presentation =
            Presentation::new(crate::terminal::enabled(std::io::stdout().is_terminal()));
        println!();
        println!(
            "{}",
            presentation.heading(format!("{} ({})", spec.label(), spec.key().as_str()))
        );
        println!("{}", spec.description());
        println!();
        println!("Current: {}", self.display_value(spec));
        if let Some(default) = self.default_description(spec) {
            println!("Default: {default}");
        }
        if let Some(required) = spec.required_hint(self.kind) {
            println!("Required: {required}");
        }
        if let Some(condition) = spec.condition(self.kind) {
            let state = if self.condition_satisfied(condition) {
                "active"
            } else {
                "inactive"
            };
            println!("Condition: {condition} · {state}");
        }
        if let Some(reason) = spec.unsupported_reason(self.kind) {
            println!("{}", presentation.warning(format!("Not used: {reason}.")));
            if !self.has_value(spec.key()) {
                return Ok(());
            }
            println!("Enter '-' to remove the preserved value, or leave blank to keep it.");
            let value = prompt("Value")?;
            if value == "-" {
                self.staged.insert(spec.key(), None);
            } else if !value.is_empty() {
                println!(
                    "{}",
                    presentation.error("This setting cannot be changed here.")
                );
            }
            return Ok(());
        }
        println!("Accepted: {}", spec.accepted(self.kind));
        if self.has_value(spec.key()) {
            println!("Enter a new value, '-' to restore the default, or leave blank to keep it.");
        } else {
            println!("Enter a value or leave blank to keep the effective default.");
        }
        let value = prompt("Value")?;
        if value.is_empty() {
            return Ok(());
        }
        if value == "-" {
            self.staged.insert(spec.key(), None);
            return Ok(());
        }
        let setting = spec
            .parse(self.kind, &value)
            .map_err(InterfacesError::InterfaceSettingInput)?;
        self.staged.insert(spec.key(), Some(setting));
        Ok(())
    }

    fn has_value(&self, key: InterfaceSettingKey) -> bool {
        match self.staged.get(&key) {
            Some(value) => value.is_some(),
            None => self.current.contains_key(&key),
        }
    }

    fn display_value(&self, spec: InterfaceSettingSpec) -> String {
        let reset = matches!(self.staged.get(&spec.key()), Some(None));
        match self.staged.get(&spec.key()) {
            Some(Some(setting)) => {
                let value = if spec.is_secret() && !self.show_secrets {
                    "<redacted>".to_string()
                } else {
                    spec.format_value(display_setting(setting.value()))
                };
                return self.explicit_display(spec, value, "staged");
            }
            Some(None) => {}
            None => {
                if let Some(value) = self.current.get(&spec.key()) {
                    let value = if spec.is_secret() && !self.show_secrets {
                        "<redacted>".to_string()
                    } else {
                        spec.format_value(value)
                    };
                    return self.explicit_display(spec, value, "configured");
                }
            }
        }
        if let Some(reason) = spec.unsupported_reason(self.kind) {
            return format!("not used · {reason}");
        }
        if let Some(condition) = spec.condition(self.kind) {
            if !self.condition_satisfied(condition) {
                return format!("inactive · {condition}");
            }
        }
        if let Some(value) = (!reset)
            .then_some(self.planned.as_ref())
            .flatten()
            .and_then(|planned| spec.effective_value(planned))
        {
            let source = if spec.inherits_when_unset() {
                "effective"
            } else {
                "default"
            };
            return format!("{} · {source}", spec.format_value(value));
        }
        if let Some(default) = self.default_description(spec) {
            return format!("{default} · default");
        }
        if let Some(required) = spec.required_hint(self.kind) {
            return format!("required · {required}");
        }
        "unset · optional".to_string()
    }

    fn explicit_display(&self, spec: InterfaceSettingSpec, value: String, source: &str) -> String {
        if let Some(reason) = spec.unsupported_reason(self.kind) {
            return format!("{value} · {source}, not used ({reason})");
        }
        if let Some(condition) = spec.condition(self.kind) {
            if !self.condition_satisfied(condition) {
                return format!("{value} · {source}, inactive");
            }
        }
        format!("{value} · {source}")
    }

    fn default_description(&self, spec: InterfaceSettingSpec) -> Option<String> {
        if matches!(spec.key().as_str(), "network_name" | "pass_phrase") {
            return Some(
                if self.condition_satisfied(InterfaceSettingCondition::IfacEnabled) {
                    "not set; IFAC remains active through the other credential".to_string()
                } else {
                    "not set; interface access is open".to_string()
                },
            );
        }
        spec.default_hint(self.kind).map(str::to_string)
    }

    fn condition_satisfied(&self, condition: InterfaceSettingCondition) -> bool {
        match condition {
            InterfaceSettingCondition::IfacEnabled => {
                self.has_named_value("network_name") || self.has_named_value("pass_phrase")
            }
            InterfaceSettingCondition::Discoverable => self.boolean_value("discoverable", false),
            InterfaceSettingCondition::DiscoverableKiss => {
                self.boolean_value("discoverable", false)
                    && self.boolean_value("kiss_framing", false)
            }
            InterfaceSettingCondition::AnnounceRateLimit => {
                self.has_named_value("announce_rate_target")
                    || self
                        .planned
                        .as_ref()
                        .is_some_and(|planned| planned.policy.announce_rate_limit.is_some())
            }
            InterfaceSettingCondition::IngressControl => {
                self.boolean_value("ingress_control", true)
            }
            InterfaceSettingCondition::EgressControl => self.boolean_value("egress_control", false),
            InterfaceSettingCondition::KissFraming => self.boolean_value("kiss_framing", false),
        }
    }

    fn has_named_value(&self, name: &str) -> bool {
        InterfaceSettingKey::parse(name).is_some_and(|key| self.has_value(key))
    }

    fn boolean_value(&self, name: &str, default: bool) -> bool {
        let Some(key) = InterfaceSettingKey::parse(name) else {
            return default;
        };
        let explicit = match self.staged.get(&key) {
            Some(Some(setting)) => Some(display_setting(setting.value())),
            Some(None) => None,
            None => self.current.get(&key).cloned(),
        };
        if let Some(value) = explicit {
            return matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "yes" | "true" | "on" | "1"
            );
        }
        self.planned
            .as_ref()
            .and_then(|planned| {
                self.kind
                    .setting_specs()
                    .into_iter()
                    .find(|spec| spec.key() == key)
                    .and_then(|spec| spec.effective_value(planned))
            })
            .map(|value| value.eq_ignore_ascii_case("yes"))
            .unwrap_or(default)
    }

    fn changes(&self) -> Vec<InterfaceSettingChange> {
        self.staged
            .iter()
            .map(|(key, setting)| match setting {
                Some(setting) => InterfaceSettingChange::Set(setting.clone()),
                None => InterfaceSettingChange::Remove(*key),
            })
            .collect()
    }
}

fn edit_radios(radios: &mut Vec<RNodeMultiRadioDefinition>) -> Result<bool, InterfacesError> {
    let mut changed = false;
    loop {
        println!();
        println!("RNodeMulti radio members");
        for (index, radio) in radios.iter().enumerate() {
            println!(
                "  {}. {} · vport {} · {} · {} · SF{} · 4/{} · {} dBm",
                index + 1,
                radio.name(),
                radio.vport(),
                format_si_frequency(radio.frequency()),
                format_si_frequency(u64::from(radio.bandwidth())),
                radio.spreading_factor(),
                radio.coding_rate(),
                radio.txpower()
            );
        }
        println!("  [A] Add  [B] Back");
        let selection = prompt("Selection")?;
        match selection.trim().to_ascii_lowercase().as_str() {
            "a" | "add" => {
                radios.push(prompt_radio(None)?);
                changed = true;
            }
            "b" | "back" | "" => return Ok(changed),
            value => {
                let index = value.parse::<usize>().map_err(|_| {
                    InterfacesError::Usage(super::error::InterfacesUsageError::InvalidSelection)
                })?;
                let Some(radio) = radios.get(index.saturating_sub(1)).cloned() else {
                    return Err(InterfacesError::Usage(
                        super::error::InterfacesUsageError::MissingSelection,
                    ));
                };
                let action = prompt("[E] Edit  [R] Remove  [B] Back")?;
                match action.trim().to_ascii_lowercase().as_str() {
                    "e" | "edit" => {
                        radios[index - 1] = prompt_radio(Some(&radio))?;
                        changed = true;
                    }
                    "r" | "remove" => {
                        radios.remove(index - 1);
                        changed = true;
                    }
                    "b" | "back" | "" => {}
                    _ => {
                        return Err(InterfacesError::Usage(
                            super::error::InterfacesUsageError::UnknownGuidedAction,
                        ))
                    }
                }
            }
        }
    }
}

fn prompt_radio(
    current: Option<&RNodeMultiRadioDefinition>,
) -> Result<RNodeMultiRadioDefinition, InterfacesError> {
    let name = prompt_default("Name", current.map(|radio| radio.name().as_str()))?;
    let vport = parse_default("Vport", current.map(RNodeMultiRadioDefinition::vport))?;
    let frequency = parse_default(
        "Frequency (Hz)",
        current.map(RNodeMultiRadioDefinition::frequency),
    )?;
    let bandwidth = parse_default(
        "Bandwidth (Hz)",
        current.map(RNodeMultiRadioDefinition::bandwidth),
    )?;
    let txpower = parse_default(
        "Transmit power (dBm)",
        current.map(RNodeMultiRadioDefinition::txpower),
    )?;
    let spreading_factor = parse_default(
        "Spreading factor",
        current.map(RNodeMultiRadioDefinition::spreading_factor),
    )?;
    let coding_rate = parse_default(
        "Coding rate",
        current.map(RNodeMultiRadioDefinition::coding_rate),
    )?;
    RNodeMultiRadioDefinition::new(
        InterfaceName::new(name).map_err(InterfacesError::InterfaceName)?,
        vport,
        frequency,
        bandwidth,
        txpower,
        spreading_factor,
        coding_rate,
    )
    .map_err(InterfacesError::RNodeMultiRadioDefinition)
}

fn prompt_default(label: &str, current: Option<&str>) -> Result<String, InterfacesError> {
    let label = current.map_or_else(|| label.to_string(), |value| format!("{label} [{value}]"));
    let value = prompt(&label)?;
    if value.is_empty() {
        return current.map(str::to_string).ok_or(InterfacesError::Usage(
            super::error::InterfacesUsageError::MissingSelection,
        ));
    }
    Ok(value)
}

fn parse_default<T>(label: &str, current: Option<T>) -> Result<T, InterfacesError>
where
    T: Copy + std::fmt::Display + std::str::FromStr,
{
    let current_text = current.map(|value| value.to_string());
    let value = prompt_default(label, current_text.as_deref())?;
    value
        .parse()
        .map_err(|_| InterfacesError::Usage(super::error::InterfacesUsageError::InvalidSelection))
}

fn format_si_frequency(hertz: u64) -> String {
    if hertz >= 1_000_000 {
        format_scaled_quantity(hertz, 1_000_000, 6, "MHz")
    } else if hertz >= 1_000 {
        format_scaled_quantity(hertz, 1_000, 3, "kHz")
    } else {
        format!("{hertz} Hz")
    }
}

fn format_scaled_quantity(value: u64, scale: u64, decimal_places: usize, unit: &str) -> String {
    let whole = value / scale;
    let remainder = value % scale;
    if remainder == 0 {
        return format!("{whole} {unit}");
    }
    let mut fractional = format!("{remainder:0decimal_places$}");
    while fractional.ends_with('0') {
        fractional.pop();
    }
    format!("{whole}.{fractional} {unit}")
}

fn display_setting(value: &InterfaceSettingValue) -> String {
    match value {
        InterfaceSettingValue::Bool(value) => if *value { "Yes" } else { "No" }.to_string(),
        InterfaceSettingValue::Unsigned(value) => value.to_string(),
        InterfaceSettingValue::Signed(value) => value.to_string(),
        InterfaceSettingValue::Decimal(value) => value.to_string(),
        InterfaceSettingValue::Text(value) => value.clone(),
        InterfaceSettingValue::List(values) => values.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use prns_config::editing::{
        InterfaceSetting, InterfaceSettingKey, InterfaceSettingTier, InterfaceSettingValue,
    };
    use prns_config::{parse_and_plan, InterfaceKind};

    use super::{format_si_frequency, SettingDraft};

    #[test]
    fn radio_frequencies_use_natural_si_units_without_losing_precision() {
        assert_eq!(format_si_frequency(915_000_000), "915 MHz");
        assert_eq!(format_si_frequency(868_100_000), "868.1 MHz");
        assert_eq!(format_si_frequency(125_000), "125 kHz");
        assert_eq!(format_si_frequency(867), "867 Hz");
    }

    #[test]
    fn configured_settings_sort_before_unset_settings_within_their_category() {
        let key = InterfaceSettingKey::parse("network_name")
            .unwrap_or_else(|| panic!("missing network name key"));
        let draft = SettingDraft::new(
            InterfaceKind::Auto,
            false,
            vec![InterfaceSetting::new(
                key,
                InterfaceSettingValue::Text("mesh".to_string()),
            )],
            Vec::new(),
        );
        let network = draft
            .ordered_specs()
            .into_iter()
            .filter(|spec| {
                spec.category() == prns_config::editing::InterfaceSettingCategory::Access
            })
            .collect::<Vec<_>>();

        assert_eq!(network[0].key(), key);
    }

    #[test]
    fn secret_values_are_redacted_in_the_guided_editor() {
        let key = InterfaceSettingKey::parse("pass_phrase")
            .unwrap_or_else(|| panic!("missing passphrase key"));
        let draft = SettingDraft::new(
            InterfaceKind::Auto,
            false,
            vec![InterfaceSetting::new(
                key,
                InterfaceSettingValue::Text("private".to_string()),
            )],
            Vec::new(),
        );
        let spec = InterfaceKind::Auto
            .setting_specs()
            .into_iter()
            .find(|spec| spec.key() == key)
            .unwrap_or_else(|| panic!("missing passphrase specification"));

        assert_eq!(draft.display_value(spec), "<redacted> · staged");
    }

    #[test]
    fn auto_editor_shows_planned_defaults_and_hides_inert_discovery_settings() {
        let source = "[interfaces]\n[[WiFi]]\ntype = AutoInterface\nenabled = Yes\n";
        let report = parse_and_plan(source).unwrap_or_else(|error| panic!("{error}"));
        let mut draft = SettingDraft::new(InterfaceKind::Auto, false, Vec::new(), Vec::new());
        draft.planned = report.value.interfaces.into_iter().next();
        let specs = draft.ordered_specs();
        let data_port = specs
            .iter()
            .find(|spec| spec.key().as_str() == "data_port")
            .copied()
            .unwrap_or_else(|| panic!("missing data port"));

        assert_eq!(draft.display_value(data_port), "42671 · default");
        assert!(!specs
            .iter()
            .any(|spec| spec.key().as_str() == "discoverable"));
        assert!(specs
            .iter()
            .all(|spec| spec.tier() == InterfaceSettingTier::Standard));
    }

    #[test]
    fn conditional_and_advanced_settings_are_distinguished() {
        let mut draft = SettingDraft::new(InterfaceKind::Auto, false, Vec::new(), Vec::new());
        let ifac = InterfaceKind::Auto
            .setting_specs()
            .into_iter()
            .find(|spec| spec.key().as_str() == "ifac_size")
            .unwrap_or_else(|| panic!("missing IFAC size"));
        assert!(draft.display_value(ifac).starts_with("inactive"));
        assert!(!draft
            .ordered_specs()
            .iter()
            .any(|spec| spec.key().as_str() == "bitrate"));

        let network_name = InterfaceSettingKey::parse("network_name")
            .unwrap_or_else(|| panic!("missing network name"));
        draft.staged.insert(
            network_name,
            Some(InterfaceSetting::new(
                network_name,
                InterfaceSettingValue::Text("mesh".to_string()),
            )),
        );
        draft.show_advanced = true;

        assert!(draft.display_value(ifac).contains("default"));
        assert!(draft
            .ordered_specs()
            .iter()
            .any(|spec| spec.key().as_str() == "bitrate"));
    }
}
