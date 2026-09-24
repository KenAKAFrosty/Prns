use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Board;

const G4_BOOT_ADAPTER_SHA256: &str =
    "134f5a4d6378c73d9f6670a3dfda4f10edc91f1488c01e2178eb8ca088e70782";
const HELTEC_BOOT_ADAPTER_SHA256: &str =
    "7254d7930320e502c9a4e30a88b99e9202be3b1d1990a563e784df7a93be6805";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RadioProfileError {
    #[error("invalid named UCI section")]
    Section,
    #[error("invalid Linux interface name")]
    Device,
    #[error("invalid mesh ID")]
    MeshId,
    #[error("vendor boot adapter differs from the qualified board contract")]
    BootAdapter,
    #[error("candidate UCI values do not match the qualified plan")]
    Projection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct UciSection(String);

impl TryFrom<String> for UciSection {
    type Error = RadioProfileError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(RadioProfileError::Section);
        }
        Ok(Self(value))
    }
}

impl UciSection {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct RadioDevice(String);

impl RadioDevice {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RadioDevice {
    type Error = RadioProfileError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 15
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(RadioProfileError::Device);
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct MeshId(String);

impl TryFrom<String> for MeshId {
    type Error = RadioProfileError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 32
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(RadioProfileError::MeshId);
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegionalChannel {
    Us924Mhz8Mhz,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RadioPreset {
    Mcs2LongGuard18Dbm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshPathSetup {
    ProactiveRequests,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioBinding {
    pub radio: UciSection,
    pub interface: UciSection,
    pub device: RadioDevice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioProfile {
    pub binding: RadioBinding,
    pub channel: RegionalChannel,
    pub preset: RadioPreset,
    pub mesh_paths: MeshPathSetup,
    pub mesh_id: MeshId,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RadioPlan {
    board: Board,
    boot_adapter_sha256: String,
    uci_batch: String,
    profile: RadioProfile,
    #[serde(skip)]
    settings: Vec<RadioSetting>,
}

#[derive(Debug, PartialEq, Eq)]
enum RadioSetting {
    Set { key: String, value: String },
    EnsureListMember { key: String, value: String },
    Remove { key: String },
}

impl RadioPlan {
    pub fn board(&self) -> &Board {
        &self.board
    }
    pub fn boot_adapter_sha256(&self) -> &str {
        &self.boot_adapter_sha256
    }
    pub fn uci_batch(&self) -> &str {
        &self.uci_batch
    }
    pub fn profile(&self) -> &RadioProfile {
        &self.profile
    }

    pub fn verify_uci_projection(&self, projection: &str) -> Result<(), RadioProfileError> {
        for setting in &self.settings {
            let matches = match setting {
                RadioSetting::Set { key, value } => projection
                    .lines()
                    .any(|line| line == format!("{key}='{value}'")),
                RadioSetting::EnsureListMember { key, value } => projection
                    .lines()
                    .find_map(|line| line.strip_prefix(&format!("{key}=")))
                    .is_some_and(|values| {
                        values
                            .split_whitespace()
                            .filter(|member| *member == format!("'{value}'"))
                            .count()
                            == 1
                    }),
                RadioSetting::Remove { key } => !projection
                    .lines()
                    .any(|line| line.starts_with(&format!("{key}="))),
            };
            if !matches {
                return Err(RadioProfileError::Projection);
            }
        }
        Ok(())
    }
}

impl RadioProfile {
    pub fn plan(&self, board: &Board, boot_adapter: &[u8]) -> Result<RadioPlan, RadioProfileError> {
        let expected = match board {
            Board::ThinkNodeG4 => G4_BOOT_ADAPTER_SHA256,
            Board::HeltecHtHd01V2 => HELTEC_BOOT_ADAPTER_SHA256,
        };
        let actual = hex::encode(Sha256::digest(boot_adapter));
        if actual != expected {
            return Err(RadioProfileError::BootAdapter);
        }
        Ok(self.qualified_plan(board, actual))
    }

    fn qualified_plan(&self, board: &Board, boot_adapter_sha256: String) -> RadioPlan {
        let radio = self.binding.radio.as_str();
        let interface = self.binding.interface.as_str();
        let device = &self.binding.device.0;
        let mesh_id = &self.mesh_id.0;
        let (country, channel) = match self.channel {
            RegionalChannel::Us924Mhz8Mhz => ("US", "44"),
        };
        let (mcs, guard, power) = match self.preset {
            RadioPreset::Mcs2LongGuard18Dbm => ("2", "0", "18"),
        };
        let mut settings = Vec::new();
        for (option, value) in [
            ("country", country),
            ("channel", channel),
            ("disabled", "0"),
            ("enable_fixed_rate", "1"),
            ("fixed_mcs", mcs),
            // These qualified MMRC versions use the bandwidth enum, not MHz.
            ("fixed_bw", "3"),
            ("fixed_ss", "1"),
            ("fixed_guard", guard),
            ("enable_ps", "0"),
            ("txpower", power),
        ] {
            settings.push(RadioSetting::Set {
                key: format!("wireless.{radio}.{option}"),
                value: value.to_owned(),
            });
        }
        settings.push(RadioSetting::EnsureListMember {
            key: format!("wireless.{radio}.s1g_capab"),
            value: "[SHORT-GI-NONE]".to_owned(),
        });
        for (option, value) in [
            ("ifname", device.as_str()),
            ("mode", "mesh"),
            ("mesh_id", mesh_id.as_str()),
            ("encryption", "none"),
            ("powersave", "0"),
            ("wds", "0"),
        ] {
            settings.push(RadioSetting::Set {
                key: format!("wireless.{interface}.{option}"),
                value: value.to_owned(),
            });
        }
        settings.push(RadioSetting::Remove {
            key: format!("wireless.{interface}.key"),
        });
        settings.push(RadioSetting::Remove {
            key: format!("wireless.{interface}.network"),
        });
        let root_mode = match self.mesh_paths {
            MeshPathSetup::ProactiveRequests => "2",
        };
        for (option, value) in [
            ("mesh_fwding", "0"),
            ("mesh_hwmp_rootmode", root_mode),
            ("mesh_gate_announcements", "0"),
        ] {
            settings.push(RadioSetting::Set {
                key: format!("mesh11sd.mesh_params.{option}"),
                value: value.to_owned(),
            });
        }
        let commands = settings
            .iter()
            .map(|setting| match setting {
                RadioSetting::Set { key, value } => format!("set {key}='{value}'\n"),
                RadioSetting::EnsureListMember { key, value } => {
                    format!("del_list {key}='{value}'\nadd_list {key}='{value}'\n")
                }
                RadioSetting::Remove { key } => format!("delete {key}\n"),
            })
            .collect();
        RadioPlan {
            board: board.clone(),
            boot_adapter_sha256,
            uci_batch: commands,
            profile: self.clone(),
            settings,
        }
    }
}

#[cfg(test)]
mod tests;

pub(crate) mod transaction;
