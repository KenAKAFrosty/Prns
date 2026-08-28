use heapless::Vec as HeaplessVec;

use crate::interfaces::AirtimeDutyCycle;

use super::modulation::{CodingRate, LoraBandwidth, Modulation, SpreadingFactor};

const DUTY_ONE_PERCENT_PER_MILLE: u16 = 10;
const DUTY_QUEUE_BUDGET_MS: u32 = 4_000;
const DUTY_TEN_PERCENT_PER_MILLE: u16 = 100;
const MODULATION_TAG_LORA: u8 = 0x00;

pub const CHANNEL_TAG_CAP: usize = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frequency(u32);

impl Frequency {
    pub const fn new(hz: u32) -> Self {
        Self(hz)
    }

    pub const fn hz(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxPower(i8);

impl TxPower {
    pub const fn new(dbm: i8) -> Self {
        Self(dbm)
    }

    pub const fn dbm(self) -> i8 {
        self.0
    }
}

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

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Region {
        Us915,
        Au915,
        Eu433,
        Eu865,
        Eu868,
        Eu869,
        As923,
        In865,
        Cn470,
        Kr920,
        Jp920,
        Unlimited,
    }
}

impl Region {
    pub const fn band(self) -> (u32, u32) {
        match self {
            Self::Us915 => (902_000_000, 928_000_000),
            Self::Au915 => (915_000_000, 928_000_000),
            Self::Eu433 => (433_050_000, 434_790_000),
            Self::Eu865 => (865_000_000, 868_000_000),
            Self::Eu868 => (868_000_000, 868_600_000),
            Self::Eu869 => (869_400_000, 869_650_000),
            Self::As923 => (920_000_000, 925_000_000),
            Self::In865 => (865_000_000, 867_000_000),
            Self::Cn470 => (470_000_000, 510_000_000),
            Self::Kr920 => (920_000_000, 923_000_000),
            Self::Jp920 => (920_800_000, 927_800_000),
            Self::Unlimited => (150_000_000, 960_000_000),
        }
    }

    pub const fn default_frequency(self) -> Frequency {
        let hz = match self {
            Self::Us915 => 915_000_000,
            Self::Au915 => 921_500_000,
            Self::Eu433 => 433_900_000,
            Self::Eu865 => 866_500_000,
            Self::Eu868 => 868_300_000,
            Self::Eu869 => 869_500_000,
            Self::As923 => 922_500_000,
            Self::In865 => 866_000_000,
            Self::Cn470 => 490_000_000,
            Self::Kr920 => 921_500_000,
            Self::Jp920 => 922_000_000,
            Self::Unlimited => 915_000_000,
        };
        Frequency::new(hz)
    }

    pub const fn max_tx_power(self) -> TxPower {
        let dbm = match self {
            Self::Us915 | Self::Au915 | Self::In865 | Self::Eu869 => 30,
            Self::Unlimited => 22,
            Self::Cn470 => 19,
            Self::As923 | Self::Jp920 => 16,
            Self::Eu865 | Self::Eu868 | Self::Kr920 => 14,
            Self::Eu433 => 12,
        };
        TxPower::new(dbm)
    }

    pub const fn regulatory_duty_cycle(self) -> Option<AirtimeDutyCycle> {
        let limit_long_per_mille = match self {
            Self::Eu865 | Self::Eu868 => DUTY_ONE_PERCENT_PER_MILLE,
            Self::Eu433 | Self::Eu869 => DUTY_TEN_PERCENT_PER_MILLE,
            _ => return None,
        };
        Some(AirtimeDutyCycle {
            limit_short_per_mille: None,
            limit_long_per_mille: Some(limit_long_per_mille),
            max_queued_airtime_ms: DUTY_QUEUE_BUDGET_MS,
        })
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Us915 => "US915",
            Self::Au915 => "AU915",
            Self::Eu433 => "EU433",
            Self::Eu865 => "EU865",
            Self::Eu868 => "EU868",
            Self::Eu869 => "EU869",
            Self::As923 => "AS923",
            Self::In865 => "IN865",
            Self::Cn470 => "CN470",
            Self::Kr920 => "KR920",
            Self::Jp920 => "JP920",
            Self::Unlimited => "Custom",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Us915 => Self::Au915,
            Self::Au915 => Self::Eu433,
            Self::Eu433 => Self::Eu865,
            Self::Eu865 => Self::Eu868,
            Self::Eu868 => Self::Eu869,
            Self::Eu869 => Self::As923,
            Self::As923 => Self::In865,
            Self::In865 => Self::Cn470,
            Self::Cn470 => Self::Kr920,
            Self::Kr920 => Self::Jp920,
            Self::Jp920 => Self::Unlimited,
            Self::Unlimited => Self::Us915,
        }
    }
}

/// Why a LoRa radio profile cannot be applied safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioProfileError {
    FrequencyOutsideRegion {
        region: Region,
        frequency_hz: u32,
        minimum_hz: u32,
        maximum_hz: u32,
    },
    TransmitPowerAboveRegionLimit {
        region: Region,
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
        region: Region,
    },
    InvalidLimitPerMille {
        limit: u16,
    },
    EmptyQueueBudget,
    WeakerThanRegionalLimit {
        region: Region,
        regional_limit_per_mille: u16,
        fixed_limit_per_mille: Option<u16>,
    },
}

impl AirtimePolicy {
    pub fn resolve(self, region: Region) -> Result<Option<AirtimeDutyCycle>, AirtimePolicyError> {
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
        Montreal,
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
            Self::Montreal => Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf9,
                bandwidth: LoraBandwidth::Bw125kHz,
                coding_rate: CodingRate::Cr47,
            },
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ShortFast => "ShortFast",
            Self::MediumFast => "MediumFast",
            Self::LongFast => "LongFast",
            Self::LongSlow => "LongSlow",
            Self::Montreal => "Montreal",
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
    pub frequency: Frequency,
    pub modulation: Modulation,
    pub tx_power: TxPower,
    pub preamble: PreambleSymbols,
    pub region: Region,
}

impl RadioProfile {
    /// Cap TX power to a board/radio ceiling. Channel identity is unchanged (power is local).
    pub const fn with_tx_power_at_most(mut self, max_dbm: i8) -> Self {
        if self.tx_power.dbm() > max_dbm {
            self.tx_power = TxPower::new(max_dbm);
        }
        self
    }

    pub const fn validate(self) -> Result<(), RadioProfileError> {
        let frequency_hz = self.frequency.hz();
        let (minimum_hz, maximum_hz) = self.region.band();
        if frequency_hz < minimum_hz || frequency_hz > maximum_hz {
            return Err(RadioProfileError::FrequencyOutsideRegion {
                region: self.region,
                frequency_hz,
                minimum_hz,
                maximum_hz,
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

pub const DEFAULT_915_PROFILE: RadioProfile = RadioProfile {
    frequency: Frequency::new(915_000_000),
    modulation: ModemPreset::MediumFast.modulation(),
    tx_power: TxPower::new(22),
    preamble: PreambleSymbols::new(18),
    region: Region::Us915,
};

/// Lab / Montreal mesh: 914.875 MHz, SF9, 125 kHz, CR 4/7, US915, preamble 18.
///
/// TX power is requested at the US915 regional ceiling (30 dBm). Callers must pass the
/// board's antenna-referred maximum into [`boot_lora_profile`] so weaker PAs clamp down.
/// Power is not part of the LoRa channel tag, so mixed TX levels still peer.
pub const MONTREAL_PROFILE: RadioProfile = RadioProfile {
    frequency: Frequency::new(914_875_000),
    modulation: ModemPreset::Montreal.modulation(),
    tx_power: TxPower::new(30),
    preamble: PreambleSymbols::new(18),
    region: Region::Us915,
};

/// Boot-time LoRa default, clamped to `max_tx_power_dbm` for this board.
/// Set `PRNS_LORA_PROFILE=montreal` at compile time for the Montreal mesh.
pub fn boot_lora_profile(max_tx_power_dbm: i8) -> RadioProfile {
    let profile = match option_env!("PRNS_LORA_PROFILE") {
        Some("montreal") => MONTREAL_PROFILE,
        _ => DEFAULT_915_PROFILE,
    };
    profile.with_tx_power_at_most(max_tx_power_dbm)
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
    use crate::interfaces::{InterfaceId, InterfaceKind};

    #[test]
    fn time_on_air_matches_the_rnode_firmware_formula() {
        assert_eq!(DEFAULT_915_PROFILE.time_on_air_us(167), 436_053);
        let long_slow = RadioProfile {
            modulation: ModemPreset::LongSlow.modulation(),
            ..DEFAULT_915_PROFILE
        };
        assert_eq!(long_slow.time_on_air_us(255), 14_203_289);
        let sub_sf7 = RadioProfile {
            modulation: Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf6,
                bandwidth: LoraBandwidth::Bw500kHz,
                coding_rate: CodingRate::Cr45,
            },
            preamble: PreambleSymbols::new(12),
            ..DEFAULT_915_PROFILE
        };
        assert_eq!(sub_sf7.time_on_air_us(50), 13_834);
    }

    #[test]
    fn default_profile_balances_capacity_and_reach_at_medium_fast() {
        assert_eq!(
            DEFAULT_915_PROFILE.modulation,
            ModemPreset::MediumFast.modulation()
        );
    }

    #[test]
    fn time_on_air_exceeds_the_nominal_serialization_time() {
        let nominal_us =
            167u64 * 8 * 1_000_000 / u64::from(DEFAULT_915_PROFILE.nominal_bitrate_bps());
        assert!(DEFAULT_915_PROFILE.time_on_air_us(167) > nominal_us);
    }

    #[test]
    fn regions_cycle_through_all_values() {
        let mut region = Region::Us915;
        for _ in 0..Region::ALL.len() {
            region = region.next();
        }
        assert_eq!(region, Region::Us915);
    }

    #[test]
    fn every_region_default_frequency_sits_inside_its_band() {
        for region in Region::ALL {
            let (lo, hi) = region.band();
            let default = region.default_frequency().hz();
            assert!(
                (lo..=hi).contains(&default),
                "{}: default {default} outside band {lo}..={hi}",
                region.label()
            );
        }
    }

    #[test]
    fn montreal_profile_validates_on_us915() {
        assert_eq!(MONTREAL_PROFILE.validate(), Ok(()));
        assert_eq!(
            MONTREAL_PROFILE.modulation,
            ModemPreset::Montreal.modulation()
        );
        assert_eq!(MONTREAL_PROFILE.nominal_bitrate_bps(), 1_255);
    }

    #[test]
    fn boot_profile_clamps_tx_power_to_the_board_ceiling() {
        let mt2 = MONTREAL_PROFILE.with_tx_power_at_most(30);
        let hv4 = MONTREAL_PROFILE.with_tx_power_at_most(28);
        let t114 = MONTREAL_PROFILE.with_tx_power_at_most(21);
        assert_eq!(mt2.tx_power.dbm(), 30);
        assert_eq!(hv4.tx_power.dbm(), 28);
        assert_eq!(t114.tx_power.dbm(), 21);
        assert_eq!(channel_tag(&mt2), channel_tag(&t114));
        assert_eq!(boot_lora_profile(14).tx_power.dbm(), 14);
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
        let a = DEFAULT_915_PROFILE;
        let mut b = DEFAULT_915_PROFILE;
        b.modulation = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf10,
            bandwidth: LoraBandwidth::Bw125kHz,
            coding_rate: CodingRate::Cr45,
        };
        let id_a = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&a));
        let id_b = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&b));
        assert_ne!(id_a, id_b);
        let id_a_again = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&a));
        assert_eq!(id_a, id_a_again);
    }

    #[test]
    fn local_knobs_do_not_re_key_identity() {
        let mut low = DEFAULT_915_PROFILE;
        let mut high = DEFAULT_915_PROFILE;
        low.tx_power = TxPower::new(2);
        high.tx_power = TxPower::new(22);
        high.preamble = PreambleSymbols::new(24);
        assert_eq!(channel_tag(&low), channel_tag(&high));
    }

    #[test]
    fn region_duty_cycles_follow_the_eu_subband_rules() {
        let eu868 = Region::Eu868
            .regulatory_duty_cycle()
            .expect("EU 868 is duty-limited");
        assert_eq!(eu868.limit_long_per_mille, Some(10));
        assert_eq!(eu868.limit_short_per_mille, None);
        assert_eq!(
            Region::Eu433
                .regulatory_duty_cycle()
                .expect("EU 433 is duty-limited")
                .limit_long_per_mille,
            Some(100)
        );
        assert_eq!(
            Region::Eu869
                .regulatory_duty_cycle()
                .unwrap()
                .limit_long_per_mille,
            Some(100)
        );
        assert!(Region::Us915.regulatory_duty_cycle().is_none());
        assert!(Region::As923.regulatory_duty_cycle().is_none());
        assert!(Region::Unlimited.regulatory_duty_cycle().is_none());
    }

    #[test]
    fn region_is_a_local_knob_outside_the_channel_tag() {
        let mut a = DEFAULT_915_PROFILE;
        let mut b = DEFAULT_915_PROFILE;
        a.region = Region::Eu868;
        b.region = Region::Unlimited;
        assert_eq!(channel_tag(&a), channel_tag(&b));
    }

    #[test]
    fn profiles_reject_out_of_band_frequency_power_and_empty_preambles() {
        assert_eq!(DEFAULT_915_PROFILE.validate(), Ok(()));

        let mut outside_band = DEFAULT_915_PROFILE;
        outside_band.frequency = Frequency::new(868_300_000);
        assert!(matches!(
            outside_band.validate(),
            Err(RadioProfileError::FrequencyOutsideRegion {
                region: Region::Us915,
                ..
            })
        ));

        let mut excessive_power = DEFAULT_915_PROFILE;
        excessive_power.region = Region::Eu868;
        excessive_power.frequency = Region::Eu868.default_frequency();
        assert_eq!(
            excessive_power.validate(),
            Err(RadioProfileError::TransmitPowerAboveRegionLimit {
                region: Region::Eu868,
                power_dbm: 22,
                maximum_dbm: 14,
            })
        );

        let mut empty_preamble = DEFAULT_915_PROFILE;
        empty_preamble.preamble = PreambleSymbols::new(0);
        assert_eq!(
            empty_preamble.validate(),
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
            AirtimePolicy::Fixed(Some(tighter)).resolve(Region::Eu868),
            Ok(Some(tighter))
        );
        assert_eq!(
            AirtimePolicy::Fixed(None).resolve(Region::Eu868),
            Err(AirtimePolicyError::MissingLimitForRegulatedRegion {
                region: Region::Eu868,
            })
        );
        assert!(matches!(
            AirtimePolicy::Fixed(Some(AirtimeDutyCycle {
                limit_short_per_mille: None,
                limit_long_per_mille: Some(20),
                max_queued_airtime_ms: 2_000,
            }))
            .resolve(Region::Eu868),
            Err(AirtimePolicyError::WeakerThanRegionalLimit { .. })
        ));
        assert_eq!(
            AirtimePolicy::Fixed(None).resolve(Region::Unlimited),
            Ok(None)
        );
    }
}
