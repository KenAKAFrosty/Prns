use std::process::ExitCode;

use thiserror::Error;

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
        match self {
            Self::Usage(_) | Self::Developer(_) => 2,
            Self::Preflight(_) => 3,
            Self::Trust(_) => 4,
            Self::Flash(_) => 5,
            Self::Cancelled => 130,
        }
    }

    pub(crate) fn error_code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage",
            Self::Preflight(_) => "device_preflight",
            Self::Trust(_) => "release_trust",
            Self::Flash(_) => "flash_failed",
            Self::Cancelled => "cancelled",
            Self::Developer(_) => "developer_workflow",
        }
    }

    pub(crate) fn exit_code(&self) -> ExitCode {
        ExitCode::from(self.code())
    }

    pub(crate) fn recovery(&self) -> &'static str {
        match self {
            Self::Usage(_) => "Run `hopspot-flash --help` and correct the requested options.",
            Self::Preflight(_) => {
                "Check the USB data cable, close other serial tools, enter bootloader mode, and retry."
            }
            Self::Trust(_) => {
                "Do not flash these bytes. Retry online or use a previously verified offline cache."
            }
            Self::Flash(_) => {
                "Hold BOOT, tap RESET, release BOOT, then restart the complete flash operation."
            }
            Self::Cancelled => "Run the complete flash operation again when the device is ready.",
            Self::Developer(_) => {
                "Check the repository toolchains and rerun the explicit developer command."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppError;

    #[test]
    fn public_error_codes_and_exit_codes_are_stable() {
        let cases = [
            (AppError::usage("usage"), "usage", 2),
            (AppError::preflight("preflight"), "device_preflight", 3),
            (AppError::trust("trust"), "release_trust", 4),
            (AppError::flash("flash"), "flash_failed", 5),
            (AppError::Cancelled, "cancelled", 130),
        ];
        for (error, error_code, exit_code) in cases {
            assert_eq!(error.error_code(), error_code);
            assert_eq!(error.code(), exit_code);
            assert!(!error.recovery().is_empty());
        }
    }
}
