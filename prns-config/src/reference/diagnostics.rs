use crate::diagnostic::{ConfigDiagnostic, ConfigDiagnosticCode};

#[derive(Clone, Copy)]
pub(super) enum WarningCode {
    UnknownKey,
    PersistedRuntimeMetadata,
    UnknownSection,
    RedundantAliases,
    UnsupportedSetting,
    IneffectiveSetting,
    ImplicitOff,
}

impl From<WarningCode> for ConfigDiagnosticCode {
    fn from(code: WarningCode) -> Self {
        match code {
            WarningCode::UnknownKey => Self::UnknownKey,
            WarningCode::PersistedRuntimeMetadata => Self::PersistedRuntimeMetadata,
            WarningCode::UnknownSection => Self::UnknownSection,
            WarningCode::RedundantAliases => Self::RedundantAliases,
            WarningCode::UnsupportedSetting => Self::UnsupportedSetting,
            WarningCode::IneffectiveSetting => Self::IneffectiveSetting,
            WarningCode::ImplicitOff => Self::ImplicitOff,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ErrorCode {
    MisplacedKey,
    MissingRequiredKey,
    InvalidValue,
    ConflictingAliases,
    UnsupportedInterface,
    UnsupportedTransport,
}

impl From<ErrorCode> for ConfigDiagnosticCode {
    fn from(code: ErrorCode) -> Self {
        match code {
            ErrorCode::MisplacedKey => Self::MisplacedKey,
            ErrorCode::MissingRequiredKey => Self::MissingRequiredKey,
            ErrorCode::InvalidValue => Self::InvalidValue,
            ErrorCode::ConflictingAliases => Self::ConflictingAliases,
            ErrorCode::UnsupportedInterface => Self::UnsupportedInterface,
            ErrorCode::UnsupportedTransport => Self::UnsupportedTransport,
        }
    }
}

pub(super) struct WarningDiagnostic(ConfigDiagnostic);

impl WarningDiagnostic {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        code: WarningCode,
        source: impl Into<String>,
        line: usize,
        path: impl Into<String>,
        value: Option<String>,
        message: impl Into<String>,
        accepted: Option<String>,
        correction: impl Into<String>,
    ) -> Self {
        Self(ConfigDiagnostic::new(
            code.into(),
            source,
            line,
            path,
            value,
            message,
            accepted,
            correction,
        ))
    }

    pub(super) fn into_inner(self) -> ConfigDiagnostic {
        self.0
    }

    pub(super) fn with_fixes(self, fixes: Vec<crate::ConfigFix>) -> Self {
        Self(self.0.with_fixes(fixes))
    }
}

pub(super) struct ErrorDiagnostic(ConfigDiagnostic);

impl ErrorDiagnostic {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        code: ErrorCode,
        source: impl Into<String>,
        line: usize,
        path: impl Into<String>,
        value: Option<String>,
        message: impl Into<String>,
        accepted: Option<String>,
        correction: impl Into<String>,
    ) -> Self {
        Self(ConfigDiagnostic::new(
            code.into(),
            source,
            line,
            path,
            value,
            message,
            accepted,
            correction,
        ))
    }

    pub(super) fn into_inner(self) -> ConfigDiagnostic {
        self.0
    }

    pub(super) fn with_fixes(self, fixes: Vec<crate::ConfigFix>) -> Self {
        Self(self.0.with_fixes(fixes))
    }
}
