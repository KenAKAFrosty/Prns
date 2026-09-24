use heapless::{String, Vec};

use super::{
    AirtimePolicy, AirtimePolicyError, Frequency, Modulation, PreambleSymbols, RadioProfile,
    RadioProfileError, TxPower, CHANNEL_TAG_CAP, INVENTORY_CONFIG_CAP,
};
use crate::interfaces::subghz::{ResolvedSubGMode, SubGConfiguration, SubGConfigurationState};
use crate::interfaces::AirtimeDutyCycle;

#[cfg(feature = "lora-2g4")]
mod ghz24;
#[cfg(feature = "lora-2g4")]
pub use ghz24::{Ghz24Profile, Ghz24ProfileError, GHZ24_BALANCED_PROFILE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaProfile {
    SubG(RadioProfile),
    #[cfg(feature = "lora-2g4")]
    Ghz24(Ghz24Profile),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaProfileError {
    SubG(RadioProfileError),
    #[cfg(feature = "lora-2g4")]
    Ghz24(Ghz24ProfileError),
}

impl From<RadioProfile> for LoRaProfile {
    fn from(profile: RadioProfile) -> Self {
        Self::SubG(profile)
    }
}

impl LoRaProfile {
    pub const fn frequency(self) -> Frequency {
        match self {
            Self::SubG(profile) => profile.frequency(),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile.frequency(),
        }
    }
    pub const fn modulation(self) -> Modulation {
        match self {
            Self::SubG(profile) => profile.modulation(),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile.modulation(),
        }
    }
    pub const fn tx_power(self) -> TxPower {
        match self {
            Self::SubG(profile) => profile.tx_power(),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile.tx_power(),
        }
    }
    pub const fn preamble(self) -> PreambleSymbols {
        match self {
            Self::SubG(profile) => profile.preamble(),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile.preamble(),
        }
    }
    pub fn validate(self) -> Result<(), LoRaProfileError> {
        match self {
            Self::SubG(profile) => profile.validate().map_err(LoRaProfileError::SubG),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile.validate().map_err(LoRaProfileError::Ghz24),
        }
    }
    pub fn with_tx_power(self, power: TxPower) -> Result<Self, LoRaProfileError> {
        match self {
            Self::SubG(profile) => profile
                .with_tx_power(power)
                .map(Self::SubG)
                .map_err(LoRaProfileError::SubG),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile
                .with_tx_power(power)
                .map(Self::Ghz24)
                .map_err(LoRaProfileError::Ghz24),
        }
    }
    pub fn with_modulation(self, modulation: Modulation) -> Result<Self, LoRaProfileError> {
        match self {
            Self::SubG(profile) => profile
                .with_modulation(modulation)
                .map(Self::SubG)
                .map_err(LoRaProfileError::SubG),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile
                .with_modulation(modulation)
                .map(Self::Ghz24)
                .map_err(LoRaProfileError::Ghz24),
        }
    }
    pub fn with_preamble(self, preamble: PreambleSymbols) -> Result<Self, LoRaProfileError> {
        match self {
            Self::SubG(profile) => profile
                .with_preamble(preamble)
                .map(Self::SubG)
                .map_err(LoRaProfileError::SubG),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile
                .with_preamble(preamble)
                .map(Self::Ghz24)
                .map_err(LoRaProfileError::Ghz24),
        }
    }
    pub fn airtime_policy(
        self,
        policy: AirtimePolicy,
    ) -> Result<Option<AirtimeDutyCycle>, AirtimePolicyError> {
        match self {
            Self::SubG(profile) => policy.resolve(profile.region()),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(_) => policy.resolve_unregulated(),
        }
    }
    pub const fn nominal_bitrate_bps(self) -> u32 {
        self.modulation().nominal_bitrate_bps()
    }
    #[inline]
    pub const fn time_on_air_us(self, frame_bytes: usize) -> u64 {
        match self {
            Self::SubG(profile) => profile.time_on_air_us(frame_bytes),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile.time_on_air_us(frame_bytes),
        }
    }
    pub fn channel_tag(self) -> Vec<u8, CHANNEL_TAG_CAP> {
        super::profile::channel_parameters_tag(self.frequency(), self.modulation())
    }
    pub fn inventory_config(self) -> String<INVENTORY_CONFIG_CAP> {
        match self {
            Self::SubG(profile) => profile.inventory_config(),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => profile.inventory_config(),
        }
    }
    pub fn parse_inventory_config(text: &str) -> Option<Self> {
        #[cfg(feature = "lora-2g4")]
        if text.starts_with("G,") {
            return Ghz24Profile::parse_inventory_config(text).map(Self::Ghz24);
        }
        RadioProfile::parse_inventory_config(text).map(Self::SubG)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaConfiguration {
    SubG(SubGConfiguration),
    #[cfg(feature = "lora-2g4")]
    Ghz24(Ghz24Profile),
}

impl LoRaConfiguration {
    pub const fn profile(self) -> LoRaProfile {
        match self {
            Self::SubG(configuration) => {
                let ResolvedSubGMode::LoRa(profile) = configuration.resolve();
                LoRaProfile::SubG(profile)
            }
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => LoRaProfile::Ghz24(profile),
        }
    }
    pub const fn manual(profile: LoRaProfile) -> Self {
        match profile {
            LoRaProfile::SubG(profile) => Self::SubG(SubGConfiguration::manual_lora(profile)),
            #[cfg(feature = "lora-2g4")]
            LoRaProfile::Ghz24(profile) => Self::Ghz24(profile),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaConfigurationState {
    Unconfigured,
    Configured(LoRaConfiguration),
}

impl From<SubGConfigurationState> for LoRaConfigurationState {
    fn from(state: SubGConfigurationState) -> Self {
        match state {
            SubGConfigurationState::Unconfigured => Self::Unconfigured,
            SubGConfigurationState::Configured(configuration) => {
                Self::Configured(LoRaConfiguration::SubG(configuration))
            }
        }
    }
}

#[cfg(test)]
mod tests;
