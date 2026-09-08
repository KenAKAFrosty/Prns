use heapless::Vec as HeaplessVec;

use crate::interfaces::subghz::{Frequency, SubGRegion, TxPower};
use crate::interfaces::AirtimeDutyCycle;

use super::modulation::{CodingRate, LoraBandwidth, Modulation, SpreadingFactor};

const MODULATION_TAG_LORA: u8 = 0x00;

pub const CHANNEL_TAG_CAP: usize = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreambleSymbols(u16);

impl PreambleSymbols {
    pub const fn new(count: u16) -> Self {
        Self(count)
    }

    pub const fn count(self) -> u16 {
        self.0
    }
}

/// Why a LoRa radio profile cannot be applied safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioProfileError {
    NominalChannelOutsideRegion {
        region: SubGRegion,
        center_hz: u32,
        bandwidth_hz: u32,
        minimum_hz: u32,
        maximum_hz: u32,
    },
    TransmitPowerAboveRegionLimit {
        region: SubGRegion,
        power_dbm: i8,
        maximum_dbm: i8,
    },
    EmptyPreamble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioProfileCompatibilityError {
    TransmitPowerOutsideRadioRange {
        power_dbm: i8,
        minimum_dbm: i8,
        maximum_dbm: i8,
    },
}

/// Where a LoRa interface obtains its airtime limit.
///
/// Regional policy is the normal choice. A fixed override may tighten a
/// region's limit, but cannot weaken it; `None` is accepted only for the
/// explicit custom-band region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirtimePolicy {
    Regional,
    Fixed(Option<AirtimeDutyCycle>),
}

/// Why an explicit airtime policy cannot be applied to a region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirtimePolicyError {
    MissingLimitForRegulatedRegion {
        region: SubGRegion,
    },
    InvalidLimitPerMille {
        limit: u16,
    },
    EmptyQueueBudget,
    WeakerThanRegionalLimit {
        region: SubGRegion,
        regional_limit_per_mille: u16,
        fixed_limit_per_mille: Option<u16>,
    },
}

impl AirtimePolicy {
    pub fn resolve(
        self,
        region: SubGRegion,
    ) -> Result<Option<AirtimeDutyCycle>, AirtimePolicyError> {
        let regional = region.regulatory_duty_cycle();
        let resolved = match self {
            Self::Regional => return Ok(regional),
            Self::Fixed(fixed) => fixed,
        };

        let Some(fixed) = resolved else {
            return if regional.is_some() {
                Err(AirtimePolicyError::MissingLimitForRegulatedRegion { region })
            } else {
                Ok(None)
            };
        };
        for limit in [fixed.limit_short_per_mille, fixed.limit_long_per_mille]
            .into_iter()
            .flatten()
        {
            if limit == 0 || limit > 1_000 {
                return Err(AirtimePolicyError::InvalidLimitPerMille { limit });
            }
        }
        if fixed.max_queued_airtime_ms == 0 {
            return Err(AirtimePolicyError::EmptyQueueBudget);
        }
        if let Some(regional_limit) = regional.and_then(|duty| duty.limit_long_per_mille) {
            if fixed
                .limit_long_per_mille
                .is_none_or(|fixed_limit| fixed_limit > regional_limit)
            {
                return Err(AirtimePolicyError::WeakerThanRegionalLimit {
                    region,
                    regional_limit_per_mille: regional_limit,
                    fixed_limit_per_mille: fixed.limit_long_per_mille,
                });
            }
        }
        Ok(Some(fixed))
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ModemPreset {
        ShortFast,
        MediumFast,
        LongFast,
        LongSlow,
    }
}

impl ModemPreset {
    pub const fn modulation(self) -> Modulation {
        match self {
            Self::ShortFast => Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf7,
                bandwidth: LoraBandwidth::Bw250kHz,
                coding_rate: CodingRate::Cr45,
            },
            Self::MediumFast => Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf9,
                bandwidth: LoraBandwidth::Bw250kHz,
                coding_rate: CodingRate::Cr45,
            },
            Self::LongFast => Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf11,
                bandwidth: LoraBandwidth::Bw250kHz,
                coding_rate: CodingRate::Cr45,
            },
            Self::LongSlow => Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf12,
                bandwidth: LoraBandwidth::Bw125kHz,
                coding_rate: CodingRate::Cr48,
            },
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ShortFast => "ShortFast",
            Self::MediumFast => "MediumFast",
            Self::LongFast => "LongFast",
            Self::LongSlow => "LongSlow",
        }
    }

    pub fn matching(modulation: Modulation) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.modulation() == modulation)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioProfile {
    frequency: Frequency,
    modulation: Modulation,
    tx_power: TxPower,
    preamble: PreambleSymbols,
    region: SubGRegion,
}

impl RadioProfile {
    pub const fn new(
        region: SubGRegion,
        frequency: Frequency,
        modulation: Modulation,
        tx_power: TxPower,
        preamble: PreambleSymbols,
    ) -> Result<Self, RadioProfileError> {
        let profile = Self {
            frequency,
            modulation,
            tx_power,
            preamble,
            region,
        };
        match profile.validate() {
            Ok(()) => Ok(profile),
            Err(error) => Err(error),
        }
    }

    pub const fn frequency(self) -> Frequency {
        self.frequency
    }

    pub const fn modulation(self) -> Modulation {
        self.modulation
    }

    pub const fn tx_power(self) -> TxPower {
        self.tx_power
    }

    pub const fn preamble(self) -> PreambleSymbols {
        self.preamble
    }

    pub const fn region(self) -> SubGRegion {
        self.region
    }

    pub const fn with_frequency(self, frequency: Frequency) -> Result<Self, RadioProfileError> {
        Self::new(
            self.region,
            frequency,
            self.modulation,
            self.tx_power,
            self.preamble,
        )
    }

    pub const fn with_modulation(self, modulation: Modulation) -> Result<Self, RadioProfileError> {
        Self::new(
            self.region,
            self.frequency,
            modulation,
            self.tx_power,
            self.preamble,
        )
    }

    pub const fn with_tx_power(self, tx_power: TxPower) -> Result<Self, RadioProfileError> {
        Self::new(
            self.region,
            self.frequency,
            self.modulation,
            tx_power,
            self.preamble,
        )
    }

    pub const fn with_preamble(self, preamble: PreambleSymbols) -> Result<Self, RadioProfileError> {
        Self::new(
            self.region,
            self.frequency,
            self.modulation,
            self.tx_power,
            preamble,
        )
    }

    pub const fn validate(self) -> Result<(), RadioProfileError> {
        let range = self.region.frequency_range();
        let Modulation::Lora { bandwidth, .. } = self.modulation;
        if !range.contains_nominal_channel(self.frequency, bandwidth.hz()) {
            return Err(RadioProfileError::NominalChannelOutsideRegion {
                region: self.region,
                center_hz: self.frequency.hz(),
                bandwidth_hz: bandwidth.hz(),
                minimum_hz: range.minimum().hz(),
                maximum_hz: range.maximum().hz(),
            });
        }
        let power_dbm = self.tx_power.dbm();
        let maximum_dbm = self.region.max_tx_power().dbm();
        if power_dbm > maximum_dbm {
            return Err(RadioProfileError::TransmitPowerAboveRegionLimit {
                region: self.region,
                power_dbm,
                maximum_dbm,
            });
        }
        if self.preamble.count() == 0 {
            return Err(RadioProfileError::EmptyPreamble);
        }
        Ok(())
    }

    pub const fn nominal_bitrate_bps(self) -> u32 {
        self.modulation.nominal_bitrate_bps()
    }

    /// RNode firmware `add_airtime` (RNode_Firmware.ino, SX126x arm): the real on-air time of one `frame_bytes` transmission at this profile, counting what the nominal bitrate ignores (preamble, PHY header symbols, CRC bits, sync overhead, low-data-rate widening). Integer throughout; agrees with the firmware's float arithmetic to under a microsecond.
    pub const fn time_on_air_us(self, frame_bytes: usize) -> u64 {
        let Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        } = self.modulation;
        let sf = spreading_factor as u128;
        let coding = coding_rate as u128;
        let bandwidth_hz = bandwidth.hz() as u128;
        let preamble = self.preamble.count() as u128;
        let bytes = frame_bytes as u128;
        let (coded_bits, quarter_denominator, tail_quarter_symbols) = if sf >= 7 {
            let ldro = if self.modulation.is_low_data_rate() {
                2
            } else {
                0
            };
            (
                (8 * bytes + 44).saturating_sub(4 * sf),
                sf - ldro,
                4 * preamble + 33,
            )
        } else {
            (
                (8 * bytes + 36).saturating_sub(4 * sf),
                sf,
                4 * preamble + 41,
            )
        };
        let payload_us =
            coded_bits * coding * (1 << sf) * 1_000_000 / (4 * quarter_denominator * bandwidth_hz);
        let tail_us = tail_quarter_symbols * (1 << sf) * 250_000 / bandwidth_hz;
        (payload_us + tail_us) as u64
    }
}

pub fn channel_tag(profile: &RadioProfile) -> HeaplessVec<u8, CHANNEL_TAG_CAP> {
    let mut tag = HeaplessVec::new();
    let _ = tag.extend_from_slice(&profile.frequency.hz().to_be_bytes());
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation;
    let _ = tag.push(MODULATION_TAG_LORA);
    let _ = tag.push(spreading_factor as u8);
    let _ = tag.extend_from_slice(&bandwidth.hz().to_be_bytes());
    let _ = tag.push(coding_rate as u8);
    tag
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE;
    use crate::interfaces::subghz::{RegulatoryRegion, SubGRegion};
    use crate::interfaces::{InterfaceId, InterfaceKind};

    #[test]
    fn time_on_air_matches_the_rnode_firmware_formula() {
        assert_eq!(US915_AUTO_LORA_PROFILE.time_on_air_us(167), 68_525);
        let long_slow = US915_AUTO_LORA_PROFILE
            .with_modulation(ModemPreset::LongSlow.modulation())
            .unwrap();
        assert_eq!(long_slow.time_on_air_us(255), 14_203_289);
        let sub_sf7 = US915_AUTO_LORA_PROFILE
            .with_modulation(Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf6,
                bandwidth: LoraBandwidth::Bw500kHz,
                coding_rate: CodingRate::Cr45,
            })
            .unwrap()
            .with_preamble(PreambleSymbols::new(12))
            .unwrap();
        assert_eq!(sub_sf7.time_on_air_us(50), 13_834);
    }

    #[test]
    fn auto_lora_profile_uses_the_fastest_supported_lora_shape() {
        assert_eq!(
            US915_AUTO_LORA_PROFILE.modulation(),
            Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf7,
                bandwidth: LoraBandwidth::Bw500kHz,
                coding_rate: CodingRate::Cr45,
            }
        );
        assert_eq!(
            US915_AUTO_LORA_PROFILE.frequency(),
            Frequency::new(921_500_000)
        );
    }

    #[test]
    fn time_on_air_exceeds_the_nominal_serialization_time() {
        let nominal_us =
            167u64 * 8 * 1_000_000 / u64::from(US915_AUTO_LORA_PROFILE.nominal_bitrate_bps());
        assert!(US915_AUTO_LORA_PROFILE.time_on_air_us(167) > nominal_us);
    }

    #[test]
    fn regions_cycle_through_all_values() {
        let mut region = RegulatoryRegion::Us915;
        for _ in 0..RegulatoryRegion::ALL.len() {
            region = region.next();
        }
        assert_eq!(region, RegulatoryRegion::Us915);
    }

    #[test]
    fn every_region_default_channel_sits_inside_its_frequency_range() {
        for region in RegulatoryRegion::ALL {
            let defaults = SubGRegion::Regulated(region).manual_lora_defaults();
            let Modulation::Lora { bandwidth, .. } = defaults.modulation();
            assert!(
                region
                    .frequency_range()
                    .contains_nominal_channel(defaults.frequency(), bandwidth.hz()),
                "{} default channel is outside its frequency range",
                region.label()
            );
        }
    }

    #[test]
    fn modem_presets_round_trip_through_their_modulation() {
        for preset in ModemPreset::ALL {
            assert_eq!(ModemPreset::matching(preset.modulation()), Some(preset));
        }
        assert_eq!(ModemPreset::LongFast.modulation().nominal_bitrate_bps(), {
            Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf11,
                bandwidth: LoraBandwidth::Bw250kHz,
                coding_rate: CodingRate::Cr45,
            }
            .nominal_bitrate_bps()
        });
    }

    #[test]
    fn changing_the_channel_settings_re_keys_the_interface_id() {
        let a = US915_AUTO_LORA_PROFILE;
        let b = US915_AUTO_LORA_PROFILE
            .with_modulation(Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf10,
                bandwidth: LoraBandwidth::Bw125kHz,
                coding_rate: CodingRate::Cr45,
            })
            .unwrap();
        let id_a = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&a));
        let id_b = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&b));
        assert_ne!(id_a, id_b);
        let id_a_again = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&a));
        assert_eq!(id_a, id_a_again);
    }

    #[test]
    fn local_knobs_do_not_re_key_identity() {
        let low = US915_AUTO_LORA_PROFILE
            .with_tx_power(TxPower::new(2))
            .unwrap();
        let high = US915_AUTO_LORA_PROFILE
            .with_preamble(PreambleSymbols::new(24))
            .unwrap();
        assert_eq!(channel_tag(&low), channel_tag(&high));
    }

    #[test]
    fn region_duty_cycles_follow_the_eu_subband_rules() {
        let eu868 = RegulatoryRegion::Eu868
            .regulatory_duty_cycle()
            .expect("EU 868 is duty-limited");
        assert_eq!(eu868.limit_long_per_mille, Some(10));
        assert_eq!(eu868.limit_short_per_mille, None);
        assert_eq!(
            RegulatoryRegion::Eu433
                .regulatory_duty_cycle()
                .expect("EU 433 is duty-limited")
                .limit_long_per_mille,
            Some(100)
        );
        assert_eq!(
            RegulatoryRegion::Eu869
                .regulatory_duty_cycle()
                .unwrap()
                .limit_long_per_mille,
            Some(100)
        );
        assert!(RegulatoryRegion::Us915.regulatory_duty_cycle().is_none());
        assert!(RegulatoryRegion::As923.regulatory_duty_cycle().is_none());
        assert!(SubGRegion::Custom.regulatory_duty_cycle().is_none());
    }

    #[test]
    fn profiles_reject_out_of_band_frequency_power_and_empty_preambles() {
        assert_eq!(US915_AUTO_LORA_PROFILE.validate(), Ok(()));

        assert!(matches!(
            RadioProfile::new(
                SubGRegion::Regulated(RegulatoryRegion::Us915),
                Frequency::new(868_300_000),
                US915_AUTO_LORA_PROFILE.modulation(),
                US915_AUTO_LORA_PROFILE.tx_power(),
                US915_AUTO_LORA_PROFILE.preamble(),
            ),
            Err(RadioProfileError::NominalChannelOutsideRegion {
                region: SubGRegion::Regulated(RegulatoryRegion::Us915),
                ..
            })
        ));

        assert_eq!(
            RadioProfile::new(
                SubGRegion::Regulated(RegulatoryRegion::Eu868),
                RegulatoryRegion::Eu868.default_frequency(),
                US915_AUTO_LORA_PROFILE.modulation(),
                TxPower::new(22),
                US915_AUTO_LORA_PROFILE.preamble(),
            ),
            Err(RadioProfileError::TransmitPowerAboveRegionLimit {
                region: SubGRegion::Regulated(RegulatoryRegion::Eu868),
                power_dbm: 22,
                maximum_dbm: 14,
            })
        );

        assert_eq!(
            US915_AUTO_LORA_PROFILE.with_preamble(PreambleSymbols::new(0)),
            Err(RadioProfileError::EmptyPreamble)
        );
    }

    #[test]
    fn fixed_airtime_policy_can_only_preserve_or_tighten_regional_limits() {
        let tighter = AirtimeDutyCycle {
            limit_short_per_mille: None,
            limit_long_per_mille: Some(5),
            max_queued_airtime_ms: 2_000,
        };
        assert_eq!(
            AirtimePolicy::Fixed(Some(tighter))
                .resolve(SubGRegion::Regulated(RegulatoryRegion::Eu868)),
            Ok(Some(tighter))
        );
        assert_eq!(
            AirtimePolicy::Fixed(None).resolve(SubGRegion::Regulated(RegulatoryRegion::Eu868)),
            Err(AirtimePolicyError::MissingLimitForRegulatedRegion {
                region: SubGRegion::Regulated(RegulatoryRegion::Eu868),
            })
        );
        assert!(matches!(
            AirtimePolicy::Fixed(Some(AirtimeDutyCycle {
                limit_short_per_mille: None,
                limit_long_per_mille: Some(20),
                max_queued_airtime_ms: 2_000,
            }))
            .resolve(SubGRegion::Regulated(RegulatoryRegion::Eu868)),
            Err(AirtimePolicyError::WeakerThanRegionalLimit { .. })
        ));
        assert_eq!(
            AirtimePolicy::Fixed(None).resolve(SubGRegion::Custom),
            Ok(None)
        );
    }
}
