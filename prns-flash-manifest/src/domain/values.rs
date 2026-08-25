use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ProvisioningDescriptor, TcpClientProvisioningDescriptor};

const UF2_MOUNT_LABEL_MAX_BYTES: usize = 32;
const UF2_BOARD_ID_MATCH_VALUE_MAX_BYTES: usize = 128;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DomainValueError {
    #[error("board ID {0:?} must use lowercase ASCII, digits, and hyphens")]
    BoardId(String),
    #[error("UF2 mount label {0:?} is not a canonical cross-platform volume label")]
    Uf2MountLabel(String),
    #[error("UF2 Board-ID match {0:?} is not a bounded canonical identity")]
    Uf2BoardIdMatch(String),
    #[error("release version {0:?} is not an immutable path-safe identifier")]
    ReleaseVersion(String),
    #[error("key ID {0:?} must be exactly 16 hexadecimal characters")]
    KeyId(String),
    #[error("SHA-256 digest must be exactly 64 lowercase hexadecimal characters")]
    Sha256Digest,
    #[error("artifact path {0:?} is not immutable and relative")]
    ImmutableArtifactPath(String),
    #[error("unsupported chip family {0:?}")]
    ChipFamily(String),
    #[error("unsupported preparation profile {0:?}")]
    PreparationProfile(String),
    #[error("unsupported pre-connect reset strategy {0:?}")]
    BeforeResetStrategy(String),
    #[error("unsupported post-flash reset strategy {0:?}")]
    AfterResetStrategy(String),
    #[error("unsupported provisioning format {0:?}")]
    ProvisioningFormat(String),
    #[error("unsupported flash mode {0:?}")]
    FlashMode(String),
    #[error("unsupported flash frequency {0:?}")]
    FlashFrequency(String),
    #[error("unsupported SoftDevice family {0:?}")]
    SoftdeviceFamily(String),
    #[error("malformed SoftDevice version {0:?}")]
    SoftdeviceVersion(String),
}

macro_rules! validated_string {
    ($name:ident) => {
        impl $name {
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

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct BoardId(String);

impl BoardId {
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

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Uf2MountLabel(String);

impl Uf2MountLabel {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainValueError> {
        let value = value.into();
        let valid = (1..=UF2_MOUNT_LABEL_MAX_BYTES).contains(&value.len())
            && value
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        valid
            .then_some(Self(value.clone()))
            .ok_or(DomainValueError::Uf2MountLabel(value))
    }
}

validated_string!(Uf2MountLabel);

/// How a catalog entry matches the `Board-ID` published in `INFO_UF2.TXT`.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Uf2BoardIdMatchKind {
    Exact,
    RevisionPrefix,
}

/// Validated, typed match rule for a UF2 bootloader `Board-ID`.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Uf2BoardIdMatch {
    kind: Uf2BoardIdMatchKind,
    value: String,
}

impl Uf2BoardIdMatch {
    pub fn normalize(value: &str) -> String {
        value.trim().to_ascii_lowercase().replace('_', "-")
    }

    pub fn parse(
        kind: Uf2BoardIdMatchKind,
        value: impl Into<String>,
    ) -> Result<Self, DomainValueError> {
        let value = value.into();
        let canonical = (1..=UF2_BOARD_ID_MATCH_VALUE_MAX_BYTES).contains(&value.len())
            && value == Self::normalize(&value)
            && value
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
            });
        let valid = canonical
            && match kind {
                Uf2BoardIdMatchKind::Exact => true,
                Uf2BoardIdMatchKind::RevisionPrefix => value.ends_with("-v"),
            };
        valid
            .then_some(Self {
                kind,
                value: value.clone(),
            })
            .ok_or(DomainValueError::Uf2BoardIdMatch(value))
    }

    pub const fn kind(&self) -> Uf2BoardIdMatchKind {
        self.kind
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn matches(&self, board_id: &str) -> bool {
        match self.kind {
            Uf2BoardIdMatchKind::Exact => board_id == self.value,
            Uf2BoardIdMatchKind::RevisionPrefix => board_id
                .strip_prefix(&self.value)
                .is_some_and(valid_revision_suffix),
        }
    }

    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        match (self.kind, other.kind) {
            (Uf2BoardIdMatchKind::Exact, Uf2BoardIdMatchKind::Exact) => self.value == other.value,
            (Uf2BoardIdMatchKind::Exact, Uf2BoardIdMatchKind::RevisionPrefix) => {
                other.matches(&self.value)
            }
            (Uf2BoardIdMatchKind::RevisionPrefix, Uf2BoardIdMatchKind::Exact) => {
                self.matches(&other.value)
            }
            (Uf2BoardIdMatchKind::RevisionPrefix, Uf2BoardIdMatchKind::RevisionPrefix) => {
                self.value == other.value
            }
        }
    }
}

impl AsRef<str> for Uf2BoardIdMatch {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Uf2BoardIdMatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn valid_revision_suffix(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// Validated immutable release version.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseVersion(String);

impl ReleaseVersion {
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

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ChipFamily {
    Esp32S3,
    Esp32C6,
}

impl ChipFamily {
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "esp32s3" => Ok(Self::Esp32S3),
            "esp32c6" => Ok(Self::Esp32C6),
            _ => Err(DomainValueError::ChipFamily(value.to_string())),
        }
    }

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

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum PreparationProfile {
    EspUsbBoot,
    TechoUf2,
    T096Uf2,
    T114Uf2,
    T1000eNrfDfu,
}

impl PreparationProfile {
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "esp-usb-boot" => Ok(Self::EspUsbBoot),
            "techo-uf2" => Ok(Self::TechoUf2),
            "t096-uf2" => Ok(Self::T096Uf2),
            "t114-uf2" => Ok(Self::T114Uf2),
            "t1000e-nrf-dfu" => Ok(Self::T1000eNrfDfu),
            _ => Err(DomainValueError::PreparationProfile(value.to_string())),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EspUsbBoot => "esp-usb-boot",
            Self::TechoUf2 => "techo-uf2",
            Self::T096Uf2 => "t096-uf2",
            Self::T114Uf2 => "t114-uf2",
            Self::T1000eNrfDfu => "t1000e-nrf-dfu",
        }
    }
}

/// Reset strategy used before an ESP connection.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum BeforeResetStrategy {
    DefaultReset,
    UsbReset,
}

impl BeforeResetStrategy {
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "default-reset" => Ok(Self::DefaultReset),
            "usb-reset" => Ok(Self::UsbReset),
            _ => Err(DomainValueError::BeforeResetStrategy(value.to_string())),
        }
    }

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
    HardReset,
    WatchdogReset,
}

impl AfterResetStrategy {
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "hard-reset" => Ok(Self::HardReset),
            "watchdog-reset" => Ok(Self::WatchdogReset),
            _ => Err(DomainValueError::AfterResetStrategy(value.to_string())),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HardReset => "hard-reset",
            Self::WatchdogReset => "watchdog-reset",
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum FlashMode {
    Dio,
}

impl FlashMode {
    pub(crate) fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "dio" => Ok(Self::Dio),
            _ => Err(DomainValueError::FlashMode(value.to_string())),
        }
    }

    pub const fn as_str(self) -> &'static str {
        "dio"
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum FlashFrequency {
    Mhz40,
}

impl FlashFrequency {
    pub(crate) fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "40m" => Ok(Self::Mhz40),
            _ => Err(DomainValueError::FlashFrequency(value.to_string())),
        }
    }

    pub const fn as_str(self) -> &'static str {
        "40m"
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ProvisioningFormat {
    Hspcfg1,
}

impl ProvisioningFormat {
    pub fn parse(value: &str) -> Result<Self, DomainValueError> {
        match value {
            "HSPCFG1" => Ok(Self::Hspcfg1),
            _ => Err(DomainValueError::ProvisioningFormat(value.to_string())),
        }
    }

    pub const fn as_str(self) -> &'static str {
        "HSPCFG1"
    }
}

/// Validated provisioning slot attached only to an ESP target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisioningSlot {
    pub(crate) wire_format: ProvisioningFormat,
    pub(crate) wire_format_version: u8,
    pub(crate) flash_offset: u32,
    pub(crate) reserved_size_bytes: u32,
    pub(crate) ssid_max_bytes: usize,
    pub(crate) password_max_bytes: usize,
    pub(crate) tcp_client: Option<TcpClientProvisioningDescriptor>,
}

impl ProvisioningSlot {
    pub const fn wire_format(&self) -> ProvisioningFormat {
        self.wire_format
    }

    pub const fn wire_format_version(&self) -> u8 {
        self.wire_format_version
    }

    pub const fn flash_offset(&self) -> u32 {
        self.flash_offset
    }

    pub const fn reserved_size_bytes(&self) -> u32 {
        self.reserved_size_bytes
    }

    pub const fn ssid_max_bytes(&self) -> usize {
        self.ssid_max_bytes
    }

    pub const fn password_max_bytes(&self) -> usize {
        self.password_max_bytes
    }

    pub fn tcp_client(&self) -> Option<&TcpClientProvisioningDescriptor> {
        self.tcp_client.as_ref()
    }

    pub(crate) fn to_wire(&self) -> ProvisioningDescriptor {
        ProvisioningDescriptor {
            format: self.wire_format.as_str().to_string(),
            version: self.wire_format_version,
            offset: self.flash_offset,
            size: self.reserved_size_bytes,
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
    fn uf2_bootloader_identity_values_are_strict() {
        for valid in ["TECHOBOOT", "T114_BOOT", "UF2.1"] {
            assert!(Uf2MountLabel::parse(valid).is_ok(), "{valid}");
        }
        for invalid in ["", ".UF2", "BAD LABEL", "../UF2", "UF2/BOOT"] {
            assert!(Uf2MountLabel::parse(invalid).is_err(), "{invalid}");
        }
        assert!(Uf2MountLabel::parse("A".repeat(UF2_MOUNT_LABEL_MAX_BYTES + 1)).is_err());

        assert_eq!(
            Uf2BoardIdMatch::normalize(" nRF52840_TEcho_v2.1 "),
            "nrf52840-techo-v2.1"
        );
        assert!(
            Uf2BoardIdMatch::parse(Uf2BoardIdMatchKind::RevisionPrefix, "nrf52840-techo-v").is_ok()
        );
        assert!(Uf2BoardIdMatch::parse(Uf2BoardIdMatchKind::Exact, "ht-n5262").is_ok());
        assert!(
            Uf2BoardIdMatch::parse(Uf2BoardIdMatchKind::RevisionPrefix, "nrf52840-techo-v1")
                .is_err()
        );
        for invalid in [
            "",
            "nRF52840_TEcho_v",
            "-nrf52840-techo-v",
            "nrf52840 techo v",
            "nrf52840/techo/v",
            "nrf52840-téchō-v",
        ] {
            assert!(
                Uf2BoardIdMatch::parse(Uf2BoardIdMatchKind::Exact, invalid).is_err(),
                "{invalid}"
            );
        }
        assert!(Uf2BoardIdMatch::parse(
            Uf2BoardIdMatchKind::Exact,
            "a".repeat(UF2_BOARD_ID_MATCH_VALUE_MAX_BYTES + 1)
        )
        .is_err());
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
