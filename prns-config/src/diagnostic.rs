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
    UnknownSection,
    MissingRequiredKey,
    InvalidValue,
    ConflictingAliases,
    RedundantAliases,
    UnsupportedInterface,
    UnsupportedTransport,
    UnsupportedSetting,
    IneffectiveSetting,
}

impl ConfigDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            ConfigDiagnosticCode::Syntax => "syntax",
            ConfigDiagnosticCode::MisplacedKey => "misplaced_key",
            ConfigDiagnosticCode::UnknownKey => "unknown_key",
            ConfigDiagnosticCode::UnknownSection => "unknown_section",
            ConfigDiagnosticCode::MissingRequiredKey => "missing_required_key",
            ConfigDiagnosticCode::InvalidValue => "invalid_value",
            ConfigDiagnosticCode::ConflictingAliases => "conflicting_aliases",
            ConfigDiagnosticCode::RedundantAliases => "redundant_aliases",
            ConfigDiagnosticCode::UnsupportedInterface => "unsupported_interface",
            ConfigDiagnosticCode::UnsupportedTransport => "unsupported_transport",
            ConfigDiagnosticCode::UnsupportedSetting => "unsupported_setting",
            ConfigDiagnosticCode::IneffectiveSetting => "ineffective_setting",
        }
    }

    pub const fn severity(self) -> ConfigSeverity {
        match self {
            ConfigDiagnosticCode::UnknownKey
            | ConfigDiagnosticCode::UnknownSection
            | ConfigDiagnosticCode::RedundantAliases
            | ConfigDiagnosticCode::UnsupportedSetting
            | ConfigDiagnosticCode::IneffectiveSetting => ConfigSeverity::Warning,
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
        Self {
            code,
            source: source.into(),
            line,
            path: path.into(),
            value,
            message: message.into(),
            accepted,
            correction: correction.into(),
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
}

impl fmt::Display for ConfigDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}: {}[{}] {}: {}",
            self.source,
            self.line,
            self.severity(),
            self.code.as_str(),
            self.path,
            self.message,
        )?;
        if let Some(value) = &self.value {
            write!(formatter, "; found {value:?}")?;
        }
        if let Some(accepted) = &self.accepted {
            write!(formatter, "; accepted: {accepted}")?;
        }
        write!(formatter, "; fix: {}", self.correction)
    }
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
