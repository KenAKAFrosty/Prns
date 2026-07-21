use std::process::ExitCode;

use serde::Serialize;
use thiserror::Error;

/// Stable machine-readable failure classes for schema-1 CLI events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ErrorCode {
    Usage,
    DevicePreflight,
    ReleaseTrust,
    #[serde(rename = "flash_failed")]
    WriteVerifyReset,
    Cancelled,
    DeveloperWorkflow,
}

impl ErrorCode {
    pub(crate) const fn process_code(self) -> u8 {
        match self {
            Self::Usage | Self::DeveloperWorkflow => 2,
            Self::DevicePreflight => 3,
            Self::ReleaseTrust => 4,
            Self::WriteVerifyReset => 5,
            Self::Cancelled => 130,
        }
    }

    pub(crate) const fn recovery(self) -> &'static str {
        match self {
            Self::Usage => "Run `hopspot-flash --help` and correct the requested options.",
            Self::DevicePreflight => {
                "Check the USB data cable, close other serial tools, enter bootloader mode, and retry."
            }
            Self::ReleaseTrust => {
                "Do not flash these bytes. Retry online or use a previously verified offline cache."
            }
            Self::WriteVerifyReset => {
                "Hold BOOT, tap RESET, release BOOT, then restart the complete flash operation."
            }
            Self::Cancelled => "Run the complete flash operation again when the device is ready.",
            Self::DeveloperWorkflow => {
                "Check the repository toolchains and rerun the explicit developer command."
            }
        }
    }
}

/// Stable process error categories for human and automation callers.
#[derive(Debug, Error)]
pub(crate) enum AppError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Preflight(String),
    #[error("{0}")]
    Trust(String),
    #[error("{0}")]
    Flash(String),
    #[error("operation cancelled; no success was reported")]
    Cancelled,
    #[error("developer workflow failed: {0}")]
    Developer(String),
}

impl AppError {
    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    pub(crate) fn preflight(message: impl Into<String>) -> Self {
        Self::Preflight(message.into())
    }

    pub(crate) fn trust(message: impl Into<String>) -> Self {
        Self::Trust(message.into())
    }

    pub(crate) fn flash(message: impl Into<String>) -> Self {
        Self::Flash(message.into())
    }

    pub(crate) fn developer(message: impl Into<String>) -> Self {
        Self::Developer(message.into())
    }

    pub(crate) fn code(&self) -> u8 {
        self.error_code().process_code()
    }

    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Usage(_) => ErrorCode::Usage,
            Self::Preflight(_) => ErrorCode::DevicePreflight,
            Self::Trust(_) => ErrorCode::ReleaseTrust,
            Self::Flash(_) => ErrorCode::WriteVerifyReset,
            Self::Cancelled => ErrorCode::Cancelled,
            Self::Developer(_) => ErrorCode::DeveloperWorkflow,
        }
    }

    pub(crate) fn exit_code(&self) -> ExitCode {
        ExitCode::from(self.code())
    }

    pub(crate) fn recovery(&self) -> &'static str {
        self.error_code().recovery()
    }
}

#[cfg(test)]
mod tests {
    use super::{AppError, ErrorCode};

    #[test]
    fn public_error_codes_and_exit_codes_are_stable() {
        let cases = [
            (AppError::usage("usage"), ErrorCode::Usage, 2),
            (
                AppError::preflight("preflight"),
                ErrorCode::DevicePreflight,
                3,
            ),
            (AppError::trust("trust"), ErrorCode::ReleaseTrust, 4),
            (AppError::flash("flash"), ErrorCode::WriteVerifyReset, 5),
            (AppError::Cancelled, ErrorCode::Cancelled, 130),
            (
                AppError::developer("developer"),
                ErrorCode::DeveloperWorkflow,
                2,
            ),
        ];
        for (error, error_code, exit_code) in cases {
            assert_eq!(error.error_code(), error_code);
            assert_eq!(error.code(), exit_code);
            assert!(!error.recovery().is_empty());
        }
    }

    #[test]
    fn schema_one_error_code_spelling_is_stable() {
        let cases = [
            (ErrorCode::Usage, r#""usage""#),
            (ErrorCode::DevicePreflight, r#""device_preflight""#),
            (ErrorCode::ReleaseTrust, r#""release_trust""#),
            (ErrorCode::WriteVerifyReset, r#""flash_failed""#),
            (ErrorCode::Cancelled, r#""cancelled""#),
            (ErrorCode::DeveloperWorkflow, r#""developer_workflow""#),
        ];

        for (error_code, expected) in cases {
            assert_eq!(
                serde_json::to_string(&error_code).expect("error code serializes"),
                expected
            );
        }
    }
}
