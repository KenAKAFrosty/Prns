use std::fmt;

use thiserror::Error;

use crate::{ProvisioningDescriptor, TcpClientProvisioningDescriptor};

/// Failure to construct one of the validated release-domain values.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DomainValueError {
    /// A stable board identifier is malformed.
    #[error("board ID {0:?} must use lowercase ASCII, digits, and hyphens")]
    BoardId(String),
    /// A UF2 bootloader identity prefix is not in canonical form.
    #[error("UF2 Board-ID prefix {0:?} is not canonically normalized")]
    Uf2BoardIdPrefix(String),
    /// An immutable release version is malformed.
    #[error("release version {0:?} is not an immutable path-safe identifier")]
    ReleaseVersion(String),
    /// A signing key identifier is malformed.
    #[error("key ID {0:?} must be exactly 16 hexadecimal characters")]
    KeyId(String),
    /// A SHA-256 digest is malformed.
    #[error("SHA-256 digest must be exactly 64 lowercase hexadecimal characters")]
    Sha256Digest,
    /// An artifact path is mutable or can escape its release directory.
    #[error("artifact path {0:?} is not immutable and relative")]
    ImmutableArtifactPath(String),
    /// An expected Espressif chip is unsupported.
    #[error("unsupported chip family {0:?}")]
    ChipFamily(String),
    /// A preparation profile is unsupported.
    #[error("unsupported preparation profile {0:?}")]
    PreparationProfile(String),
    /// A pre-connect reset strategy is unsupported.
    #[error("unsupported pre-connect reset strategy {0:?}")]
    BeforeResetStrategy(String),
    /// A post-flash reset strategy is unsupported.
    #[error("unsupported post-flash reset strategy {0:?}")]
    AfterResetStrategy(String),
    /// A provisioning format is unsupported.
    #[error("unsupported provisioning format {0:?}")]
    ProvisioningFormat(String),
    /// A flash-mode value is unsupported.
    #[error("unsupported flash mode {0:?}")]
    FlashMode(String),
    /// A flash-frequency value is unsupported.
    #[error("unsupported flash frequency {0:?}")]
    FlashFrequency(String),
}

macro_rules! validated_string {
    ($name:ident) => {
        impl $name {
            /// Borrow the canonical wire spelling.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

/// Validated stable board identifier.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct BoardId(String);

impl BoardId {
    /// Validate a public board identifier.
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainValueError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        valid
            .then_some(Self(value.clone()))
            .ok_or(DomainValueError::BoardId(value))
    }
}

validated_string!(BoardId);

/// Validated `Board-ID` prefix a UF2 bootloader publishes in `INFO_UF2.TXT`.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Uf2BoardIdPrefix(String);

impl Uf2BoardIdPrefix {
    /// Fold a bootloader-reported identity into the one spelling the catalog stores.
    pub fn normalize(value: &str) -> String {
        value.trim().to_ascii_lowercase().replace('_', "-")
    }

    /// Require the catalog to store an already-normalized prefix so no comparison has to
    /// normalize the trusted side.
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainValueError> {
        let value = value.into();
        let canonical = !value.is_empty() && value == Self::normalize(&value);
        canonical
            .then_some(Self(value.clone()))
            .ok_or(DomainValueError::Uf2BoardIdPrefix(value))
    }
}

validated_string!(Uf2BoardIdPrefix);

/// Validated immutable release version.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseVersion(String);

impl ReleaseVersion {
    /// Validate a version suitable for immutable release paths.
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainValueError> {
        let value = value.into();
        let valid = !value.is_empty()
            && !value.eq_ignore_ascii_case("next")
            && !matches!(value.as_str(), "." | "..")
            && value.bytes().any(|byte| byte.is_ascii_alphanumeric())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'));
        valid
            .then_some(Self(value.clone()))
            .ok_or(DomainValueError::ReleaseVersion(value))
    }
}

validated_string!(ReleaseVersion);

/// Canonical 16-hex-digit Minisign key identifier.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyId(String);

impl KeyId {
    /// Validate and canonicalize a Minisign key identifier.
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainValueError> {
        let value = value.into();
        if value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Ok(Self(value.to_ascii_uppercase()))
        } else {
            Err(DomainValueError::KeyId(value))
        }
    }
}

validated_string!(KeyId);

/// Validated lowercase SHA-256 digest.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// Validate a lowercase SHA-256 wire value.
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainValueError> {
        let value = value.into();
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            Ok(Self(value))
        } else {
            Err(DomainValueError::Sha256Digest)
        }
    }
}

validated_string!(Sha256Digest);

/// Validated relative path beneath one immutable release directory.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImmutableArtifactPath(String);

impl ImmutableArtifactPath {
    /// Reject absolute, escaping, mutable, and URL-like artifact paths.
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainValueError> {
        let value = value.into();
        let valid = !value.is_empty()
            && !value.starts_with('/')
            && value.split('/').all(|component| {
                !component.is_empty()
                    && !matches!(component, "." | "..")
                    && !component.eq_ignore_ascii_case("latest")
                    && component.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+')
                    })
            });
        valid
            .then_some(Self(value.clone()))
            .ok_or(DomainValueError::ImmutableArtifactPath(value))
    }
}

validated_string!(ImmutableArtifactPath);

/// Espressif silicon families supported by public flash plans.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ChipFamily {
    /// ESP32-S3.
    Esp32S3,
    /// ESP32-C6.
    Esp32C6,
}

impl ChipFamily {
    /// Parse the catalog/manifest wire spelling.
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "esp32s3" => Ok(Self::Esp32S3),
            "esp32c6" => Ok(Self::Esp32C6),
            _ => Err(DomainValueError::ChipFamily(value.to_string())),
        }
    }

    /// Return the canonical wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Esp32S3 => "esp32s3",
            Self::Esp32C6 => "esp32c6",
        }
    }
}

impl fmt::Display for ChipFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable preparation-instruction profile.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum PreparationProfile {
    /// Espressif USB serial bootloader preparation.
    EspUsbBoot,
    /// T-Echo UF2 bootloader preparation.
    TechoUf2,
}

impl PreparationProfile {
    /// Parse the catalog/manifest wire spelling.
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "esp-usb-boot" => Ok(Self::EspUsbBoot),
            "techo-uf2" => Ok(Self::TechoUf2),
            _ => Err(DomainValueError::PreparationProfile(value.to_string())),
        }
    }

    /// Return the canonical wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EspUsbBoot => "esp-usb-boot",
            Self::TechoUf2 => "techo-uf2",
        }
    }
}

/// Reset strategy used before an ESP connection.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum BeforeResetStrategy {
    /// Conventional serial reset.
    DefaultReset,
    /// Native USB reset.
    UsbReset,
}

impl BeforeResetStrategy {
    /// Parse the catalog/manifest wire spelling.
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "default-reset" => Ok(Self::DefaultReset),
            "usb-reset" => Ok(Self::UsbReset),
            _ => Err(DomainValueError::BeforeResetStrategy(value.to_string())),
        }
    }

    /// Return the canonical wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DefaultReset => "default-reset",
            Self::UsbReset => "usb-reset",
        }
    }
}

/// Reset strategy used only after every ESP part verifies.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum AfterResetStrategy {
    /// Hardware reset.
    HardReset,
    /// Watchdog reset.
    WatchdogReset,
}

impl AfterResetStrategy {
    /// Parse the catalog/manifest wire spelling.
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "hard-reset" => Ok(Self::HardReset),
            "watchdog-reset" => Ok(Self::WatchdogReset),
            _ => Err(DomainValueError::AfterResetStrategy(value.to_string())),
        }
    }

    /// Return the canonical wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HardReset => "hard-reset",
            Self::WatchdogReset => "watchdog-reset",
        }
    }
}

/// Supported ESP flash mode.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum FlashMode {
    /// Dual I/O.
    Dio,
}

impl FlashMode {
    pub(crate) fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "dio" => Ok(Self::Dio),
            _ => Err(DomainValueError::FlashMode(value.to_string())),
        }
    }

    /// Return the canonical wire spelling.
    pub const fn as_str(self) -> &'static str {
        "dio"
    }
}

/// Supported ESP flash frequency.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum FlashFrequency {
    /// 40 MHz.
    Mhz40,
}

impl FlashFrequency {
    pub(crate) fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "40m" => Ok(Self::Mhz40),
            _ => Err(DomainValueError::FlashFrequency(value.to_string())),
        }
    }

    /// Return the canonical wire spelling.
    pub const fn as_str(self) -> &'static str {
        "40m"
    }
}

/// Supported local provisioning wire format.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ProvisioningFormat {
    /// HSPCFG1 versioned 4 KiB slot.
    Hspcfg1,
}

impl ProvisioningFormat {
    /// Parse the provisioning format wire spelling.
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "HSPCFG1" => Ok(Self::Hspcfg1),
            _ => Err(DomainValueError::ProvisioningFormat(value.to_string())),
        }
    }

    /// Return the canonical wire spelling.
    pub const fn as_str(self) -> &'static str {
        "HSPCFG1"
    }
}

/// Validated provisioning slot attached only to an ESP target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisioningSlot {
    pub(crate) format: ProvisioningFormat,
    pub(crate) version: u8,
    pub(crate) offset: u32,
    pub(crate) size: u32,
    pub(crate) ssid_max_bytes: usize,
    pub(crate) password_max_bytes: usize,
    pub(crate) tcp_client: Option<TcpClientProvisioningDescriptor>,
}

impl ProvisioningSlot {
    /// Provisioning format.
    pub const fn format(&self) -> ProvisioningFormat {
        self.format
    }

    /// Wire-format version.
    pub const fn version(&self) -> u8 {
        self.version
    }

    /// Absolute flash offset.
    pub const fn offset(&self) -> u32 {
        self.offset
    }

    /// Reserved byte size.
    pub const fn size(&self) -> u32 {
        self.size
    }

    /// Maximum encoded SSID bytes.
    pub const fn ssid_max_bytes(&self) -> usize {
        self.ssid_max_bytes
    }

    /// Maximum encoded password bytes.
    pub const fn password_max_bytes(&self) -> usize {
        self.password_max_bytes
    }

    pub fn tcp_client(&self) -> Option<&TcpClientProvisioningDescriptor> {
        self.tcp_client.as_ref()
    }

    pub(crate) fn to_wire(&self) -> ProvisioningDescriptor {
        ProvisioningDescriptor {
            format: self.format.as_str().to_string(),
            version: self.version,
            offset: self.offset,
            size: self.size,
            ssid_max_bytes: self.ssid_max_bytes,
            password_max_bytes: self.password_max_bytes,
            tcp_client: self.tcp_client.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_identifiers_and_digests_are_strict() {
        assert!(BoardId::parse("heltec-v4").is_ok());
        assert!(BoardId::parse("Heltec V4").is_err());
        assert!(ReleaseVersion::parse("0.2.6-preview.1").is_ok());
        assert!(ReleaseVersion::parse("next").is_err());
        assert!(ReleaseVersion::parse(".").is_err());
        assert!(ReleaseVersion::parse("..").is_err());
        assert!(KeyId::parse("1fb2ca18b2c25e1f").is_ok());
        assert!(KeyId::parse("short").is_err());
        assert!(Sha256Digest::parse("a".repeat(64)).is_ok());
        assert!(Sha256Digest::parse("A".repeat(64)).is_err());
    }

    #[test]
    fn immutable_paths_cannot_escape_or_name_mutable_content() {
        assert!(
            ImmutableArtifactPath::parse("firmware/hopspot/heltec-v4/0.2.6/application.bin")
                .is_ok()
        );
        for invalid in [
            "/absolute.bin",
            "firmware/../secret",
            "firmware/hopspot/latest/application.bin",
            "latest/firmware.bin",
            "firmware/LATEST/application.bin",
            "firmware/%2e%2e/application.bin",
            "firmware/%252e%252e/application.bin",
            "firmware\\artifact.bin",
            "firmware//artifact.bin",
            "firmware/artifact name.bin",
            "firmware/artifact:copy.bin",
        ] {
            assert!(ImmutableArtifactPath::parse(invalid).is_err(), "{invalid}");
        }
    }
}
