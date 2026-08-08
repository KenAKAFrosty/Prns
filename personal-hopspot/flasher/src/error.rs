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
}

/// Invalid command intent known before a device is modified.
#[derive(Debug, Error)]
pub(crate) enum UsageError {
    #[error("{0}")]
    Arguments(String),
    #[error("{0}")]
    Configuration(String),
    #[error("{0}")]
    Confirmation(String),
    #[error("{0}")]
    UnsupportedOperation(String),
    #[error("{0}")]
    Output(String),
}

impl UsageError {
    const fn recovery(&self) -> &'static str {
        match self {
            Self::Arguments(_) => {
                "Run `hopspot-flash --help` and correct the requested options."
            }
            Self::Configuration(_) => {
                "Correct the release or provisioning configuration, then restart the complete operation."
            }
            Self::Confirmation(_) => {
                "Physically confirm the exact board, then retry interactively or with the documented acknowledgement flag."
            }
            Self::UnsupportedOperation(_) => {
                "Use the transport and options documented for the selected board."
            }
            Self::Output(_) => {
                "Retry the command; if machine output still cannot be encoded, report the failure without credentials."
            }
        }
    }
}

/// Non-writing host, port, mount, or device-identity failure.
#[derive(Debug, Error)]
pub(crate) enum PreflightError {
    #[error("{0}")]
    Host(String),
    #[error("{0}")]
    SerialPort(String),
    #[error("{0}")]
    DeviceIdentity(String),
    #[error("{0}")]
    Uf2Mount(String),
    #[error("{0}")]
    Monitor(String),
}

impl PreflightError {
    const fn recovery(&self) -> &'static str {
        match self {
            Self::Host(_) => "Correct the local host setup or permissions, then retry.",
            Self::SerialPort(_) => {
                "Check the USB data cable, close other serial tools, enter bootloader mode, and retry."
            }
            Self::DeviceIdentity(_) => {
                "Reconnect the device in bootloader mode and verify the selected board before retrying."
            }
            Self::Uf2Mount(_) => {
                "Double-tap RESET, wait for exactly one bootloader drive, then retry."
            }
            Self::Monitor(_) => {
                "Close other serial tools, reconnect the device, and start monitoring again."
            }
        }
    }
}

/// Failure at a signed-release trust boundary.
#[derive(Debug, Error)]
pub(crate) enum TrustError {
    #[error("{0}")]
    Signing(String),
    #[error("{0}")]
    Manifest(String),
    #[error("{0}")]
    Artifact(String),
    #[error("{0}")]
    Cache(String),
    #[error("{0}")]
    ReleaseIdentity(String),
    #[error("{0}")]
    Catalog(String),
    #[error("{0}")]
    Candidate(String),
}

impl TrustError {
    const fn recovery(&self) -> &'static str {
        match self {
            Self::Cache(_) => {
                "Retry online or import the exact signed candidate again; never reuse a corrupt cache entry."
            }
            Self::Artifact(_) => {
                "Do not flash these bytes. Reacquire the exact signed artifact and verify it again."
            }
            Self::Signing(_)
            | Self::Manifest(_)
            | Self::ReleaseIdentity(_)
            | Self::Catalog(_)
            | Self::Candidate(_) => {
                "Do not flash these bytes. Retry from the signed release source or use a previously verified offline candidate."
            }
        }
    }
}

/// Failure after a write-capable operation begins.
#[derive(Debug, Error)]
pub(crate) enum WriteVerifyResetError {
    #[error("{0}")]
    Write(String),
    #[error("{0}")]
    Verify(String),
    #[error("{0}")]
    Reset(String),
    #[error("{0}")]
    DeviceLost(String),
    #[error("{0}")]
    Uf2Delivery(String),
}

impl WriteVerifyResetError {
    const fn recovery(&self) -> &'static str {
        match self {
            Self::Uf2Delivery(_) => {
                "Return the board to its bootloader drive, then copy the complete verified UF2 again."
            }
            Self::Write(_) | Self::Verify(_) | Self::Reset(_) | Self::DeviceLost(_) => {
                "Hold BOOT, tap RESET, release BOOT, then restart the complete sparse flash operation."
            }
        }
    }
}

/// Failure confined to an explicit repository/developer workflow.
#[derive(Debug, Error)]
pub(crate) enum DeveloperBuildError {
    #[error("{0}")]
    Repository(String),
    #[error("{0}")]
    Toolchain(String),
    #[error("{0}")]
    Build(String),
    #[error("{0}")]
    Artifact(String),
    #[error("{0}")]
    Manifest(String),
}

impl DeveloperBuildError {
    const fn recovery(&self) -> &'static str {
        match self {
            Self::Repository(_) => "Run the developer command from a complete PRNS source checkout.",
            Self::Toolchain(_) => {
                "Install the pinned embedded toolchains and retry the explicit developer command."
            }
            Self::Build(_) => "Fix the reported source/build failure and rebuild from the start.",
            Self::Artifact(_) => {
                "Remove only the failed developer output, then rebuild the complete target artifact."
            }
            Self::Manifest(_) => {
                "Rebuild every selected target record, then assemble and validate the manifest again."
            }
        }
    }
}

/// Stable typed process errors for human and automation callers.
#[derive(Debug, Error)]
pub(crate) enum AppError {
    #[error(transparent)]
    Usage(UsageError),
    #[error(transparent)]
    Preflight(PreflightError),
    #[error(transparent)]
    Trust(TrustError),
    #[error(transparent)]
    WriteVerifyReset(WriteVerifyResetError),
    #[error("operation cancelled; no success was reported")]
    Cancelled,
    #[error("developer workflow failed: {0}")]
    DeveloperBuild(DeveloperBuildError),
}

impl AppError {
    pub(crate) fn arguments(message: impl Into<String>) -> Self {
        Self::Usage(UsageError::Arguments(message.into()))
    }

    pub(crate) fn configuration(message: impl Into<String>) -> Self {
        Self::Usage(UsageError::Configuration(message.into()))
    }

    pub(crate) fn confirmation(message: impl Into<String>) -> Self {
        Self::Usage(UsageError::Confirmation(message.into()))
    }

    pub(crate) fn unsupported_operation(message: impl Into<String>) -> Self {
        Self::Usage(UsageError::UnsupportedOperation(message.into()))
    }

    pub(crate) fn output(message: impl Into<String>) -> Self {
        Self::Usage(UsageError::Output(message.into()))
    }

    pub(crate) fn host_preflight(message: impl Into<String>) -> Self {
        Self::Preflight(PreflightError::Host(message.into()))
    }

    pub(crate) fn serial_port(message: impl Into<String>) -> Self {
        Self::Preflight(PreflightError::SerialPort(message.into()))
    }

    pub(crate) fn device_identity(message: impl Into<String>) -> Self {
        Self::Preflight(PreflightError::DeviceIdentity(message.into()))
    }

    pub(crate) fn uf2_mount(message: impl Into<String>) -> Self {
        Self::Preflight(PreflightError::Uf2Mount(message.into()))
    }

    pub(crate) fn monitor(message: impl Into<String>) -> Self {
        Self::Preflight(PreflightError::Monitor(message.into()))
    }

    pub(crate) fn trust_signing(message: impl Into<String>) -> Self {
        Self::Trust(TrustError::Signing(message.into()))
    }

    pub(crate) fn trust_manifest(message: impl Into<String>) -> Self {
        Self::Trust(TrustError::Manifest(message.into()))
    }

    pub(crate) fn trust_artifact(message: impl Into<String>) -> Self {
        Self::Trust(TrustError::Artifact(message.into()))
    }

    pub(crate) fn trust_cache(message: impl Into<String>) -> Self {
        Self::Trust(TrustError::Cache(message.into()))
    }

    pub(crate) fn trust_identity(message: impl Into<String>) -> Self {
        Self::Trust(TrustError::ReleaseIdentity(message.into()))
    }

    pub(crate) fn trust_catalog(message: impl Into<String>) -> Self {
        Self::Trust(TrustError::Catalog(message.into()))
    }

    pub(crate) fn trust_candidate(message: impl Into<String>) -> Self {
        Self::Trust(TrustError::Candidate(message.into()))
    }

    pub(crate) fn write(message: impl Into<String>) -> Self {
        Self::WriteVerifyReset(WriteVerifyResetError::Write(message.into()))
    }

    pub(crate) fn verify(message: impl Into<String>) -> Self {
        Self::WriteVerifyReset(WriteVerifyResetError::Verify(message.into()))
    }

    pub(crate) fn reset(message: impl Into<String>) -> Self {
        Self::WriteVerifyReset(WriteVerifyResetError::Reset(message.into()))
    }

    pub(crate) fn device_lost(message: impl Into<String>) -> Self {
        Self::WriteVerifyReset(WriteVerifyResetError::DeviceLost(message.into()))
    }

    pub(crate) fn uf2_delivery(message: impl Into<String>) -> Self {
        Self::WriteVerifyReset(WriteVerifyResetError::Uf2Delivery(message.into()))
    }

    pub(crate) fn developer_repository(message: impl Into<String>) -> Self {
        Self::DeveloperBuild(DeveloperBuildError::Repository(message.into()))
    }

    pub(crate) fn developer_toolchain(message: impl Into<String>) -> Self {
        Self::DeveloperBuild(DeveloperBuildError::Toolchain(message.into()))
    }

    pub(crate) fn developer_build(message: impl Into<String>) -> Self {
        Self::DeveloperBuild(DeveloperBuildError::Build(message.into()))
    }

    pub(crate) fn developer_artifact(message: impl Into<String>) -> Self {
        Self::DeveloperBuild(DeveloperBuildError::Artifact(message.into()))
    }

    pub(crate) fn developer_manifest(message: impl Into<String>) -> Self {
        Self::DeveloperBuild(DeveloperBuildError::Manifest(message.into()))
    }

    pub(crate) fn code(&self) -> u8 {
        self.error_code().process_code()
    }

    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Usage(_) => ErrorCode::Usage,
            Self::Preflight(_) => ErrorCode::DevicePreflight,
            Self::Trust(_) => ErrorCode::ReleaseTrust,
            Self::WriteVerifyReset(_) => ErrorCode::WriteVerifyReset,
            Self::Cancelled => ErrorCode::Cancelled,
            Self::DeveloperBuild(_) => ErrorCode::DeveloperWorkflow,
        }
    }

    pub(crate) fn exit_code(&self) -> ExitCode {
        ExitCode::from(self.code())
    }

    pub(crate) fn recovery(&self) -> &'static str {
        match self {
            Self::Usage(error) => error.recovery(),
            Self::Preflight(error) => error.recovery(),
            Self::Trust(error) => error.recovery(),
            Self::WriteVerifyReset(error) => error.recovery(),
            Self::Cancelled => "Run the complete flash operation again when the device is ready.",
            Self::DeveloperBuild(error) => error.recovery(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppError, ErrorCode};

    #[test]
    fn public_error_codes_and_exit_codes_are_stable() {
        let cases = vec![
            (AppError::arguments("arguments"), ErrorCode::Usage, 2),
            (
                AppError::configuration("configuration"),
                ErrorCode::Usage,
                2,
            ),
            (AppError::confirmation("confirmation"), ErrorCode::Usage, 2),
            (
                AppError::unsupported_operation("unsupported"),
                ErrorCode::Usage,
                2,
            ),
            (AppError::output("output"), ErrorCode::Usage, 2),
            (
                AppError::host_preflight("host"),
                ErrorCode::DevicePreflight,
                3,
            ),
            (AppError::serial_port("port"), ErrorCode::DevicePreflight, 3),
            (
                AppError::device_identity("identity"),
                ErrorCode::DevicePreflight,
                3,
            ),
            (AppError::uf2_mount("mount"), ErrorCode::DevicePreflight, 3),
            (AppError::monitor("monitor"), ErrorCode::DevicePreflight, 3),
            (
                AppError::trust_signing("signing"),
                ErrorCode::ReleaseTrust,
                4,
            ),
            (
                AppError::trust_manifest("manifest"),
                ErrorCode::ReleaseTrust,
                4,
            ),
            (
                AppError::trust_artifact("artifact"),
                ErrorCode::ReleaseTrust,
                4,
            ),
            (AppError::trust_cache("cache"), ErrorCode::ReleaseTrust, 4),
            (
                AppError::trust_identity("release identity"),
                ErrorCode::ReleaseTrust,
                4,
            ),
            (
                AppError::trust_catalog("catalog"),
                ErrorCode::ReleaseTrust,
                4,
            ),
            (
                AppError::trust_candidate("candidate"),
                ErrorCode::ReleaseTrust,
                4,
            ),
            (AppError::write("write"), ErrorCode::WriteVerifyReset, 5),
            (AppError::verify("verify"), ErrorCode::WriteVerifyReset, 5),
            (AppError::reset("reset"), ErrorCode::WriteVerifyReset, 5),
            (
                AppError::device_lost("device loss"),
                ErrorCode::WriteVerifyReset,
                5,
            ),
            (
                AppError::uf2_delivery("UF2 delivery"),
                ErrorCode::WriteVerifyReset,
                5,
            ),
            (AppError::Cancelled, ErrorCode::Cancelled, 130),
            (
                AppError::developer_repository("repository"),
                ErrorCode::DeveloperWorkflow,
                2,
            ),
            (
                AppError::developer_toolchain("toolchain"),
                ErrorCode::DeveloperWorkflow,
                2,
            ),
            (
                AppError::developer_build("build"),
                ErrorCode::DeveloperWorkflow,
                2,
            ),
            (
                AppError::developer_artifact("developer artifact"),
                ErrorCode::DeveloperWorkflow,
                2,
            ),
            (
                AppError::developer_manifest("developer manifest"),
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
