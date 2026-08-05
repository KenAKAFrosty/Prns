use std::fmt;

use crate::configobj::SourceLocations;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSeverity {
    Warning,
    Error,
}

impl fmt::Display for ConfigSeverity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigSeverity::Warning => formatter.write_str("warning"),
            ConfigSeverity::Error => formatter.write_str("error"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDiagnosticCode {
    Syntax,
    MisplacedKey,
    UnknownKey,
    PersistedRuntimeMetadata,
    UnknownSection,
    MissingRequiredKey,
    InvalidValue,
    ConflictingAliases,
    RedundantAliases,
    UnsupportedInterface,
    UnsupportedTransport,
    UnsupportedSetting,
    IneffectiveSetting,
    ImplicitOff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigFix {
    DisableInterface {
        name: String,
    },
    InsertValue {
        path: String,
        accepted: String,
    },
    ReplaceValue {
        path: String,
        accepted: String,
    },
    RemoveValue {
        path: String,
        safety: ConfigFixSafety,
    },
    ResolveAliases {
        path: String,
        aliases: Vec<String>,
    },
    ChooseInterfaceType {
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFixSafety {
    Safe,
    Guided,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretDisplay {
    Redacted,
    Revealed,
}

impl ConfigFix {
    pub const fn is_safe(&self) -> bool {
        matches!(
            self,
            Self::DisableInterface { .. }
                | Self::RemoveValue {
                    safety: ConfigFixSafety::Safe,
                    ..
                }
        )
    }
}

impl ConfigDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            ConfigDiagnosticCode::Syntax => "syntax",
            ConfigDiagnosticCode::MisplacedKey => "misplaced_key",
            ConfigDiagnosticCode::UnknownKey => "unknown_key",
            ConfigDiagnosticCode::PersistedRuntimeMetadata => "persisted_runtime_metadata",
            ConfigDiagnosticCode::UnknownSection => "unknown_section",
            ConfigDiagnosticCode::MissingRequiredKey => "missing_required_key",
            ConfigDiagnosticCode::InvalidValue => "invalid_value",
            ConfigDiagnosticCode::ConflictingAliases => "conflicting_aliases",
            ConfigDiagnosticCode::RedundantAliases => "redundant_aliases",
            ConfigDiagnosticCode::UnsupportedInterface => "unsupported_interface",
            ConfigDiagnosticCode::UnsupportedTransport => "unsupported_transport",
            ConfigDiagnosticCode::UnsupportedSetting => "unsupported_setting",
            ConfigDiagnosticCode::IneffectiveSetting => "ineffective_setting",
            ConfigDiagnosticCode::ImplicitOff => "implicit_off",
        }
    }

    pub const fn severity(self) -> ConfigSeverity {
        match self {
            ConfigDiagnosticCode::UnknownKey
            | ConfigDiagnosticCode::PersistedRuntimeMetadata
            | ConfigDiagnosticCode::UnknownSection
            | ConfigDiagnosticCode::RedundantAliases
            | ConfigDiagnosticCode::UnsupportedSetting
            | ConfigDiagnosticCode::IneffectiveSetting
            | ConfigDiagnosticCode::ImplicitOff => ConfigSeverity::Warning,
            ConfigDiagnosticCode::Syntax
            | ConfigDiagnosticCode::MisplacedKey
            | ConfigDiagnosticCode::MissingRequiredKey
            | ConfigDiagnosticCode::InvalidValue
            | ConfigDiagnosticCode::ConflictingAliases
            | ConfigDiagnosticCode::UnsupportedInterface
            | ConfigDiagnosticCode::UnsupportedTransport => ConfigSeverity::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiagnostic {
    code: ConfigDiagnosticCode,
    source: String,
    line: usize,
    path: String,
    value: Option<String>,
    message: String,
    accepted: Option<String>,
    correction: String,
    fixes: Vec<ConfigFix>,
}

impl ConfigDiagnostic {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        code: ConfigDiagnosticCode,
        source: impl Into<String>,
        line: usize,
        path: impl Into<String>,
        value: Option<String>,
        message: impl Into<String>,
        accepted: Option<String>,
        correction: impl Into<String>,
    ) -> Self {
        let path = path.into();
        let fixes = default_fixes(code, &path, accepted.as_deref());
        Self {
            code,
            source: source.into(),
            line,
            path,
            value,
            message: message.into(),
            accepted,
            correction: correction.into(),
            fixes,
        }
    }

    pub const fn severity(&self) -> ConfigSeverity {
        self.code.severity()
    }

    pub const fn code(&self) -> ConfigDiagnosticCode {
        self.code
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub const fn line(&self) -> usize {
        self.line
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn accepted(&self) -> Option<&str> {
        self.accepted.as_deref()
    }

    pub fn correction(&self) -> &str {
        &self.correction
    }

    pub fn fixes(&self) -> &[ConfigFix] {
        &self.fixes
    }

    pub(crate) fn with_fixes(mut self, fixes: Vec<ConfigFix>) -> Self {
        self.fixes = fixes;
        self
    }

    pub fn display_with(&self, secrets: SecretDisplay) -> DisplayedConfigDiagnostic<'_> {
        DisplayedConfigDiagnostic {
            diagnostic: self,
            secrets,
        }
    }
}

fn default_fixes(code: ConfigDiagnosticCode, path: &str, accepted: Option<&str>) -> Vec<ConfigFix> {
    let interface = interface_name(path);
    match code {
        ConfigDiagnosticCode::MissingRequiredKey if path.ends_with(" > type") => interface
            .into_iter()
            .flat_map(|name| {
                [
                    ConfigFix::ChooseInterfaceType { name: name.clone() },
                    ConfigFix::DisableInterface { name },
                ]
            })
            .collect(),
        ConfigDiagnosticCode::MissingRequiredKey => accepted
            .map(|accepted| ConfigFix::InsertValue {
                path: path.to_string(),
                accepted: accepted.to_string(),
            })
            .into_iter()
            .chain(interface.map(|name| ConfigFix::DisableInterface { name }))
            .collect(),
        ConfigDiagnosticCode::InvalidValue => accepted
            .map(|accepted| ConfigFix::ReplaceValue {
                path: path.to_string(),
                accepted: accepted.to_string(),
            })
            .into_iter()
            .chain(interface.map(|name| ConfigFix::DisableInterface { name }))
            .collect(),
        ConfigDiagnosticCode::ConflictingAliases => vec![ConfigFix::ResolveAliases {
            path: path.to_string(),
            aliases: Vec::new(),
        }],
        ConfigDiagnosticCode::RedundantAliases => {
            vec![ConfigFix::RemoveValue {
                path: path.to_string(),
                safety: ConfigFixSafety::Safe,
            }]
        }
        ConfigDiagnosticCode::UnknownKey => vec![ConfigFix::RemoveValue {
            path: path.to_string(),
            safety: ConfigFixSafety::Guided,
        }],
        ConfigDiagnosticCode::PersistedRuntimeMetadata => vec![ConfigFix::RemoveValue {
            path: path.to_string(),
            safety: ConfigFixSafety::Safe,
        }],
        ConfigDiagnosticCode::UnsupportedInterface => interface
            .into_iter()
            .flat_map(|name| {
                [
                    ConfigFix::ChooseInterfaceType { name: name.clone() },
                    ConfigFix::DisableInterface { name },
                ]
            })
            .collect(),
        ConfigDiagnosticCode::UnsupportedTransport => interface
            .map(|name| vec![ConfigFix::DisableInterface { name }])
            .unwrap_or_default(),
        ConfigDiagnosticCode::ImplicitOff => vec![ConfigFix::ReplaceValue {
            path: path.to_string(),
            accepted: "off".to_string(),
        }],
        ConfigDiagnosticCode::Syntax
        | ConfigDiagnosticCode::MisplacedKey
        | ConfigDiagnosticCode::UnknownSection
        | ConfigDiagnosticCode::UnsupportedSetting
        | ConfigDiagnosticCode::IneffectiveSetting => Vec::new(),
    }
}

fn interface_name(path: &str) -> Option<String> {
    let start = path.find("[[")? + 2;
    let rest = &path[start..];
    let end = rest.find("]]")?;
    let name = rest[..end].trim();
    (!name.is_empty()).then(|| name.to_string())
}

impl fmt::Display for ConfigDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.display_with(SecretDisplay::Redacted).fmt(formatter)
    }
}

pub struct DisplayedConfigDiagnostic<'a> {
    diagnostic: &'a ConfigDiagnostic,
    secrets: SecretDisplay,
}

impl fmt::Display for DisplayedConfigDiagnostic<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let diagnostic = self.diagnostic;
        write!(
            formatter,
            "{}:{}: {}[{}] {}: {}",
            diagnostic.source,
            diagnostic.line,
            diagnostic.severity(),
            diagnostic.code.as_str(),
            diagnostic.path,
            diagnostic.message,
        )?;
        if let Some(value) = &diagnostic.value {
            if self.secrets == SecretDisplay::Revealed || !secret_path(&diagnostic.path) {
                write!(formatter, "; found {value:?}")?;
            } else {
                formatter.write_str("; found <redacted>")?;
            }
        }
        if let Some(accepted) = &diagnostic.accepted {
            write!(formatter, "; accepted: {accepted}")?;
        }
        if self.secrets == SecretDisplay::Redacted && secret_path(&diagnostic.path) {
            formatter.write_str("; fix: correct the secret-bearing setting")
        } else {
            write!(formatter, "; fix: {}", diagnostic.correction)
        }
    }
}

fn secret_path(path: &str) -> bool {
    matches!(
        path.rsplit(" > ").next().map(str::trim),
        Some("pass_phrase" | "passphrase" | "rpc_key")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigErrors {
    diagnostics: Vec<ConfigDiagnostic>,
}

impl ConfigErrors {
    pub(crate) fn new(diagnostics: Vec<ConfigDiagnostic>) -> Self {
        Self { diagnostics }
    }

    pub fn diagnostics(&self) -> &[ConfigDiagnostic] {
        &self.diagnostics
    }

    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

impl fmt::Display for ConfigErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index != 0 {
                formatter.write_str("\n")?;
            }
            write!(formatter, "{diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigErrors {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigReport<T> {
    pub value: T,
    pub warnings: Vec<ConfigDiagnostic>,
    pub source: String,
    pub locations: SourceLocations,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_redact_secret_values_unless_revealed() {
        let diagnostic = ConfigDiagnostic::new(
            ConfigDiagnosticCode::InvalidValue,
            "config",
            4,
            "[interfaces] > [[WiFi]] > pass_phrase",
            Some("private value".to_string()),
            "invalid passphrase",
            Some("a valid passphrase".to_string()),
            "replace the value",
        );

        assert!(!diagnostic.to_string().contains("private value"));
        assert!(diagnostic
            .display_with(SecretDisplay::Revealed)
            .to_string()
            .contains("private value"));
    }
}
