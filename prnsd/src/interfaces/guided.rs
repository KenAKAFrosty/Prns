use std::collections::BTreeMap;
use std::io::IsTerminal;

use prns_config::editing::{
    ConfigEdit, ConfigFile, ConfiguredInterface, InterfaceDefinition, InterfaceName,
    InterfaceSetting, InterfaceSettingChange, InterfaceSettingKey, InterfaceSettingSpec,
    InterfaceSettingValue, RNodeMultiRadioDefinition,
};
use prns_config::InterfaceKind;

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
    let mut draft = SettingDraft::from_interface(kind, interface, show_secrets);
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
    radios: Vec<RNodeMultiRadioDefinition>,
    radios_changed: bool,
    show_secrets: bool,
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
            radios,
            radios_changed: false,
            show_secrets,
        }
    }

    fn from_interface(
        kind: InterfaceKind,
        interface: &ConfiguredInterface,
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
            radios: interface.rnode_multi_radios().to_vec(),
            radios_changed: false,
            show_secrets,
        }
    }

    fn run(&mut self) -> Result<bool, InterfacesError> {
        loop {
            let specs = self.ordered_specs();
            self.print(&specs);
            let selection = prompt(if self.kind == InterfaceKind::RnodeMulti {
                "Setting number, [M] Radio members, [F] Finish, [B] Back"
            } else {
                "Setting number, [F] Finish, [B] Back"
            })?;
            match selection.trim().to_ascii_lowercase().as_str() {
                "f" | "finish" => return Ok(true),
                "b" | "back" | "" => return Ok(false),
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
        let mut specs = self.kind.setting_specs();
        specs.sort_by_key(|spec| {
            (
                spec.category(),
                !self.has_value(spec.key()),
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
            presentation.muted("Configured values appear before unset values in each category.")
        );
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
        if self.kind == InterfaceKind::RnodeMulti {
            println!();
            println!("  Radio members: {}", self.radios.len());
        }
        println!();
    }

    fn edit_setting(&mut self, spec: InterfaceSettingSpec) -> Result<(), InterfacesError> {
        println!();
        println!("{} ({})", spec.label(), spec.key().as_str());
        println!("Accepted: {}", spec.accepted(self.kind));
        if self.has_value(spec.key()) {
            println!("Enter a new value, '-' to remove it, or leave blank to keep it.");
        } else {
            println!("Enter a value or leave blank to keep it unset.");
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
        if spec.is_secret() && !self.show_secrets && self.has_value(spec.key()) {
            return "<redacted>".to_string();
        }
        match self.staged.get(&spec.key()) {
            Some(Some(setting)) => display_setting(setting.value()),
            Some(None) => "—".to_string(),
            None => self
                .current
                .get(&spec.key())
                .cloned()
                .unwrap_or_else(|| "—".to_string()),
        }
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
                "  {}. {} · vport {} · {} Hz · {} Hz · SF{} · CR{} · {} dBm",
                index + 1,
                radio.name(),
                radio.vport(),
                radio.frequency(),
                radio.bandwidth(),
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
        "Frequency Hz",
        current.map(RNodeMultiRadioDefinition::frequency),
    )?;
    let bandwidth = parse_default(
        "Bandwidth Hz",
        current.map(RNodeMultiRadioDefinition::bandwidth),
    )?;
    let txpower = parse_default(
        "TX power dBm",
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
    use prns_config::editing::{InterfaceSetting, InterfaceSettingKey, InterfaceSettingValue};
    use prns_config::InterfaceKind;

    use super::SettingDraft;

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
                spec.category() == prns_config::editing::InterfaceSettingCategory::Network
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

        assert_eq!(draft.display_value(spec), "<redacted>");
    }
}
