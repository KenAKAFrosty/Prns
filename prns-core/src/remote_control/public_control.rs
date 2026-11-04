//! Public radio construction and request-policy conveniences.
//!
//! These functions delegate the owning models and validators. They neither
//! configure hardware nor infer that a remote interface supports a band.
use super::{RemoteControlLoRaProfile, RemoteControlRequestKind};
use crate::interfaces::lora::{
    Frequency, LoRaProfile, Modulation, PreambleSymbols, RadioProfile, RadioProfileError,
    SubGRegion, TxPower,
};
#[cfg(feature = "lora-2g4")]
use crate::interfaces::lora::{Ghz24Profile, Ghz24ProfileError};

/// Construct a checked Sub-GHz profile using the canonical regional validation.
pub fn remote_control_radio_profile(
    region: SubGRegion,
    frequency: Frequency,
    modulation: Modulation,
    tx_power: TxPower,
    preamble: PreambleSymbols,
) -> Result<RadioProfile, RadioProfileError> {
    RadioProfile::new(region, frequency, modulation, tx_power, preamble)
}

/// Construct a checked 2.4 GHz profile. Vocabulary availability is independent
/// of the attached interface's actual radio and capabilities.
#[cfg(feature = "lora-2g4")]
pub fn remote_control_ghz24_profile(
    frequency: Frequency,
    modulation: Modulation,
    tx_power: TxPower,
    preamble: PreambleSymbols,
) -> Result<Ghz24Profile, Ghz24ProfileError> {
    Ghz24Profile::new(frequency, modulation, tx_power, preamble)
}

/// Convert a checked band profile into the existing bounded wire representation.
#[must_use]
pub fn remote_control_lora_profile_from_band(
    profile: LoRaProfile,
) -> Option<RemoteControlLoRaProfile> {
    RemoteControlLoRaProfile::from_band_profile(profile)
}

/// Decode the canonical band model; callers need not parse its compact grammar.
#[must_use]
pub fn remote_control_lora_profile_band(profile: RemoteControlLoRaProfile) -> Option<LoRaProfile> {
    profile.band_profile()
}

/// Whether this request is reserved to administrators by the owning protocol.
#[must_use]
pub fn remote_control_request_requires_administrator(kind: RemoteControlRequestKind) -> bool {
    kind.requires_administrator()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]
    use super::*;
    use crate::interfaces::lora::{CodingRate, LoraBandwidth, RegulatoryRegion, SpreadingFactor};

    #[test]
    fn checked_region_profile_roundtrips_without_exposing_the_wire_grammar() {
        let modulation = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf7,
            bandwidth: LoraBandwidth::Bw125kHz,
            coding_rate: CodingRate::Cr45,
        };
        let region = SubGRegion::Regulated(RegulatoryRegion::Us915);
        let profile = remote_control_radio_profile(
            region,
            Frequency::new(915_000_000),
            modulation,
            TxPower::new(10),
            PreambleSymbols::new(8),
        );
        let Ok(profile) = profile else {
            panic!("valid regional profile rejected")
        };
        let band = LoRaProfile::SubG(profile);
        assert_eq!(
            remote_control_lora_profile_from_band(band).and_then(remote_control_lora_profile_band),
            Some(band)
        );
        assert!(
            matches!(remote_control_radio_profile(region, Frequency::new(868_000_000),
            modulation, TxPower::new(10), PreambleSymbols::new(8)),
            Err(RadioProfileError::NominalChannelOutsideRegion { region: actual, .. }) if actual == region)
        );
        assert_eq!(
            remote_control_radio_profile(
                region,
                Frequency::new(915_000_000),
                modulation,
                TxPower::new(10),
                PreambleSymbols::new(0)
            ),
            Err(RadioProfileError::EmptyPreamble)
        );
    }

    #[cfg(feature = "lora-2g4")]
    #[test]
    fn ghz24_constructor_preserves_band_specific_failures_and_roundtrip() {
        let modulation = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf7,
            bandwidth: LoraBandwidth::Bw812kHz,
            coding_rate: CodingRate::Cr45,
        };
        let profile = remote_control_ghz24_profile(
            Frequency::new(2_445_000_000),
            modulation,
            TxPower::new(10),
            PreambleSymbols::new(18),
        );
        let Ok(profile) = profile else {
            panic!("valid 2.4 GHz profile rejected")
        };
        let band = LoRaProfile::Ghz24(profile);
        assert_eq!(
            remote_control_lora_profile_from_band(band).and_then(remote_control_lora_profile_band),
            Some(band)
        );
        assert_eq!(
            remote_control_ghz24_profile(
                Frequency::new(2_445_000_000),
                modulation,
                TxPower::new(14),
                PreambleSymbols::new(18)
            ),
            Err(Ghz24ProfileError::TransmitPowerOutsideRange)
        );
    }
}
