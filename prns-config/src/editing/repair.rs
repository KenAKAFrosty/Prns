use std::collections::BTreeSet;
use std::fmt;

use crate::configobj::{ConfigDocument, ConfigError};
use crate::{parse_and_plan_named, ConfigDiagnostic, ConfigFix};

use super::{ConfigEdit, InterfaceConfigKey, InterfaceName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRepairReport {
    diagnostics: Vec<ConfigDiagnostic>,
}

impl ConfigRepairReport {
    pub fn analyze(source: &str) -> Result<Self, ConfigRepairError> {
        Self::analyze_named("<config>", source)
    }

    pub fn analyze_named(
        source_name: impl Into<String>,
        source: &str,
    ) -> Result<Self, ConfigRepairError> {
        ConfigDocument::parse(source).map_err(ConfigRepairError::Syntax)?;
        let diagnostics = match parse_and_plan_named(source_name, source) {
            Ok(report) => report.warnings,
            Err(errors) => errors.diagnostics().to_vec(),
        };
        Ok(Self { diagnostics })
    }

    pub fn diagnostics(&self) -> &[ConfigDiagnostic] {
        &self.diagnostics
    }

    pub fn safe_edit(&self) -> Option<ConfigEdit> {
        let mut disabled = BTreeSet::new();
        let mut removed = BTreeSet::new();
        for fix in self
            .diagnostics
            .iter()
            .flat_map(ConfigDiagnostic::fixes)
            .filter(|fix| fix.is_safe())
        {
            match fix {
                ConfigFix::DisableInterface { name } => {
                    if let Ok(name) = InterfaceName::new(name.clone()) {
                        disabled.insert(name);
                    }
                }
                ConfigFix::RemoveValue { path, .. } => {
                    if let Some(target) = interface_value(path) {
                        removed.insert(target);
                    }
                }
                ConfigFix::InsertValue { .. }
                | ConfigFix::ReplaceValue { .. }
                | ConfigFix::ResolveAliases { .. }
                | ConfigFix::ChooseInterfaceType { .. } => {}
            }
        }
        if disabled.is_empty() && removed.is_empty() {
            return None;
        }
        let mut edits = disabled
            .into_iter()
            .map(|name| ConfigEdit::SetEnabled {
                name,
                enabled: false,
            })
            .collect::<Vec<_>>();
        edits.extend(
            removed
                .into_iter()
                .map(|(name, key)| ConfigEdit::RemoveInterfaceValue { name, key }),
        );
        Some(ConfigEdit::Batch(edits))
    }
}

fn interface_value(path: &str) -> Option<(InterfaceName, InterfaceConfigKey)> {
    let start = path.find("[[")? + 2;
    let rest = &path[start..];
    let end = rest.find("]]")?;
    if rest[end + 2..].contains("[[[") {
        return None;
    }
    let name = InterfaceName::new(rest[..end].trim()).ok()?;
    let key = InterfaceConfigKey::new(path.rsplit(" > ").next()?.trim()).ok()?;
    Some((name, key))
}

#[derive(Debug)]
pub enum ConfigRepairError {
    Syntax(ConfigError),
}

impl fmt::Display for ConfigRepairError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => write!(
                formatter,
                "{error}; malformed ConfigObj syntax is preserved for manual correction"
            ),
        }
    }
}

impl std::error::Error for ConfigRepairError {}
