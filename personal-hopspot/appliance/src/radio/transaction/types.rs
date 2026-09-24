use std::num::{NonZeroU16, NonZeroU64, NonZeroU8};

use serde::{Deserialize, Serialize};

use super::RadioTransactionError;
use crate::{RadioDevice, RadioPlan, RadioProfile, UciSection};

pub(super) const MAX_FILE_BYTES: u64 = 131072;
const MAX_RECOVERY_SECONDS: u16 = 3600;
pub(super) const MAX_TRIAL_BOOTS: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BootId(String);

impl TryFrom<String> for BootId {
    type Error = RadioTransactionError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 36
            || !value.bytes().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
                }
            })
        {
            return Err(RadioTransactionError::BootIdentity);
        }
        Ok(Self(value))
    }
}

impl From<BootId> for String {
    fn from(value: BootId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RadioDigest(String);

impl RadioDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(super) fn of(bytes: &[u8]) -> Self {
        Self(prns_flash_manifest::sha256_hex(bytes))
    }
}

impl TryFrom<String> for RadioDigest {
    type Error = RadioTransactionError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        prns_flash_manifest::Sha256Digest::parse(&value)
            .map_err(|_| RadioTransactionError::Digest)?;
        Ok(Self(value))
    }
}

impl From<RadioDigest> for String {
    fn from(value: RadioDigest) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct RecoveryWindow(NonZeroU16);

impl RecoveryWindow {
    pub fn seconds(&self) -> u16 {
        self.0.get()
    }
}

impl TryFrom<u16> for RecoveryWindow {
    type Error = RadioTransactionError;
    fn try_from(seconds: u16) -> Result<Self, Self::Error> {
        match NonZeroU16::new(seconds) {
            Some(seconds) if seconds.get() <= MAX_RECOVERY_SECONDS => Ok(Self(seconds)),
            _ => Err(RadioTransactionError::RecoveryWindow),
        }
    }
}

impl From<RecoveryWindow> for u16 {
    fn from(value: RecoveryWindow) -> Self {
        value.seconds()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct TrialBoots(NonZeroU8);

impl TrialBoots {
    pub fn get(&self) -> u8 {
        self.0.get()
    }
}

impl TryFrom<u8> for TrialBoots {
    type Error = RadioTransactionError;
    fn try_from(boots: u8) -> Result<Self, Self::Error> {
        match NonZeroU8::new(boots) {
            Some(boots) if boots.get() <= MAX_TRIAL_BOOTS => Ok(Self(boots)),
            _ => Err(RadioTransactionError::TrialBoots),
        }
    }
}

impl From<TrialBoots> for u8 {
    fn from(value: TrialBoots) -> Self {
        value.get()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioCandidateId {
    pub revision: NonZeroU64,
    pub profile_sha256: RadioDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootTime {
    pub boot: BootId,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioLease {
    pub boot: BootId,
    pub expires_at_uptime_seconds: u64,
    pub trial_boots_remaining: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RadioPhase {
    Prepared {
        lease: RadioLease,
    },
    Applying {
        lease: RadioLease,
    },
    Trial {
        lease: RadioLease,
    },
    Confirmed,
    CanceledBeforeApply,
    Restoring,
    RebootRequired {
        requested_from: BootId,
        expires_at_uptime_seconds: u64,
    },
    RebootUnavailable {
        requested_from: BootId,
    },
    CheckingRestoration {
        lease: RadioLease,
    },
    Restored,
    RestorationUnavailable,
}

impl RadioPhase {
    pub(super) fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Confirmed | Self::CanceledBeforeApply | Self::Restored
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RadioTransactionState {
    Empty,
    Active {
        candidate: RadioCandidateId,
        phase: RadioPhase,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioTransactionStatus {
    pub revision: u64,
    pub state: RadioTransactionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadioFile {
    Wireless,
    Mesh11sd,
    System,
    MorseModule,
}

impl RadioFile {
    pub(super) const ALL: [Self; 4] = [
        Self::Wireless,
        Self::Mesh11sd,
        Self::System,
        Self::MorseModule,
    ];
    pub(super) const CANDIDATE: [Self; 2] = [Self::Wireless, Self::Mesh11sd];
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Wireless => "wireless",
            Self::Mesh11sd => "mesh11sd",
            Self::System => "system",
            Self::MorseModule => "morse",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RadioFileImage {
    pub bytes: Vec<u8>,
    pub mode: FileMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct FileMode(u32);

impl FileMode {
    pub fn unix_bits(&self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for FileMode {
    type Error = RadioTransactionError;
    fn try_from(mode: u32) -> Result<Self, Self::Error> {
        if mode & !0o777 != 0 {
            return Err(RadioTransactionError::FileMode);
        }
        Ok(Self(mode))
    }
}

impl From<FileMode> for u32 {
    fn from(value: FileMode) -> Self {
        value.0
    }
}

pub struct RadioSnapshot {
    pub wireless: RadioFileImage,
    pub mesh11sd: RadioFileImage,
    pub system: RadioFileImage,
    pub morse_module: RadioFileImage,
}

impl RadioSnapshot {
    pub(super) fn image(&self, file: &RadioFile) -> &RadioFileImage {
        match file {
            RadioFile::Wireless => &self.wireless,
            RadioFile::Mesh11sd => &self.mesh11sd,
            RadioFile::System => &self.system,
            RadioFile::MorseModule => &self.morse_module,
        }
    }
}

pub struct RadioPreparation {
    pub plan: RadioPlan,
    pub original: RadioSnapshot,
    pub candidate_wireless: Vec<u8>,
    pub candidate_mesh11sd: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RestoredRadioReadiness {
    Pending,
    Operational,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub enum RadioRecoveryProgress {
    Inactive,
    Wait { seconds: u64 },
    RebootRequested,
    RebootUnavailable,
    Restored,
    RestorationUnavailable,
}

pub trait RadioPlatform {
    fn time(&mut self) -> Result<BootTime, RadioTransactionError>;
    fn prepare(
        &mut self,
        profile: &RadioProfile,
    ) -> Result<RadioPreparation, RadioTransactionError>;
    fn validate(&mut self, plan: &RadioPlan) -> Result<(), RadioTransactionError>;
    fn read(&mut self, file: &RadioFile) -> Result<RadioFileImage, RadioTransactionError>;
    fn replace_synced(
        &mut self,
        file: &RadioFile,
        image: &RadioFileImage,
    ) -> Result<(), RadioTransactionError>;
    fn require_recovery_service(&mut self) -> Result<(), RadioTransactionError>;
    fn reload(&mut self, radio: &UciSection) -> Result<(), RadioTransactionError>;
    fn request_reboot(&mut self) -> Result<(), RadioTransactionError>;
    fn restored_readiness(
        &mut self,
        device: &RadioDevice,
    ) -> Result<RestoredRadioReadiness, RadioTransactionError>;
}
