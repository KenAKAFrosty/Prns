use super::*;
#[cfg(feature = "lora-2g4")]
use crate::interfaces::lora::{CodingRate, LoraBandwidth, SpreadingFactor};
use crate::interfaces::subghz::{regions::us915::Us915, regions::us915::US915_AUTO_LORA_PROFILE};

#[test]
fn legacy_profile_and_configuration_adapters_preserve_all_semantics() {
    let old = US915_AUTO_LORA_PROFILE;
    let profile = LoRaProfile::from(old);
    assert_eq!(profile.validate(), Ok(()));
    assert_eq!(profile.frequency(), old.frequency());
    assert_eq!(profile.modulation(), old.modulation());
    assert_eq!(profile.tx_power(), old.tx_power());
    assert_eq!(profile.preamble(), old.preamble());
    assert_eq!(profile.nominal_bitrate_bps(), old.nominal_bitrate_bps());
    assert_eq!(
        profile.channel_tag(),
        crate::interfaces::lora::channel_tag(&old)
    );
    assert_eq!(profile.inventory_config(), old.inventory_config());
    assert_eq!(
        LoRaProfile::parse_inventory_config(old.inventory_config().as_str()),
        Some(profile)
    );
    assert_eq!(LoRaProfile::parse_inventory_config("garbage"), None);
    assert_eq!(
        profile.airtime_policy(AirtimePolicy::Regional),
        AirtimePolicy::Regional.resolve(old.region())
    );
    assert_eq!(
        profile.with_tx_power(TxPower::new(12)),
        old.with_tx_power(TxPower::new(12))
            .map(LoRaProfile::SubG)
            .map_err(LoRaProfileError::SubG)
    );
    assert_eq!(profile.with_modulation(old.modulation()), Ok(profile));
    assert_eq!(profile.with_preamble(old.preamble()), Ok(profile));
    assert_eq!(LoRaConfiguration::manual(profile).profile(), profile);
    assert_eq!(
        LoRaConfigurationState::from(SubGConfigurationState::Unconfigured),
        LoRaConfigurationState::Unconfigured
    );
    assert_eq!(
        LoRaConfigurationState::from(SubGConfigurationState::Configured(Us915::auto_lora())),
        LoRaConfigurationState::Configured(LoRaConfiguration::SubG(Us915::auto_lora()))
    );
    for length in 0..=255 {
        assert_eq!(profile.time_on_air_us(length), old.time_on_air_us(length));
    }
}

#[cfg(not(feature = "lora-2g4"))]
#[test]
fn disabled_band_is_not_accepted_as_a_subg_profile() {
    assert_eq!(
        LoRaProfile::parse_inventory_config("G,2445000000,7,8,5,10,18"),
        None
    );
}

#[cfg(feature = "lora-2g4")]
#[test]
fn balanced_profile_has_explicit_parameters_and_distinct_channel_identity() {
    let ghz = GHZ24_BALANCED_PROFILE;
    let profile = LoRaProfile::Ghz24(ghz);
    assert_eq!(ghz.frequency().hz(), 2_445_000_000);
    assert_eq!(ghz.tx_power().dbm(), 10);
    assert_eq!(ghz.preamble().count(), 18);
    assert_eq!(
        ghz.modulation(),
        Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf7,
            bandwidth: LoraBandwidth::Bw812kHz,
            coding_rate: CodingRate::Cr45
        }
    );
    assert_eq!(profile.validate(), Ok(()));
    assert_eq!(
        profile.inventory_config().as_str(),
        "G,2445000000,7,8,5,10,18"
    );
    assert_eq!(
        LoRaProfile::parse_inventory_config(profile.inventory_config().as_str()),
        Some(profile)
    );
    assert_eq!(profile.frequency(), ghz.frequency());
    assert_eq!(profile.modulation(), ghz.modulation());
    assert_eq!(profile.preamble(), ghz.preamble());
    assert_eq!(profile.tx_power(), ghz.tx_power());
    assert_eq!(profile.with_modulation(ghz.modulation()), Ok(profile));
    assert_eq!(LoRaConfiguration::manual(profile).profile(), profile);
    assert_ne!(
        profile.channel_tag(),
        crate::interfaces::lora::channel_tag(&US915_AUTO_LORA_PROFILE)
    );
    for changed in [
        profile.with_tx_power(TxPower::new(11)).unwrap(),
        profile.with_preamble(PreambleSymbols::new(20)).unwrap(),
    ] {
        assert_eq!(profile.channel_tag(), changed.channel_tag());
    }
    assert_eq!(profile.airtime_policy(AirtimePolicy::Regional), Ok(None));
    assert_eq!(profile.airtime_policy(AirtimePolicy::Fixed(None)), Ok(None));
    for length in 0..255 {
        assert!(profile.time_on_air_us(length) <= profile.time_on_air_us(length + 1));
    }
}

#[cfg(feature = "lora-2g4")]
#[test]
fn band_validation_checks_occupied_edges_power_preamble_and_bandwidth() {
    use Ghz24ProfileError::*;
    let profile = GHZ24_BALANCED_PROFILE;
    for (bandwidth, hz, next) in [
        (LoraBandwidth::Bw203kHz, 203_000, LoraBandwidth::Bw406kHz),
        (LoraBandwidth::Bw406kHz, 406_000, LoraBandwidth::Bw812kHz),
        (LoraBandwidth::Bw812kHz, 812_000, LoraBandwidth::Bw203kHz),
    ] {
        assert_eq!(bandwidth.hz(), hz);
        assert_eq!(bandwidth.next(), next);
        assert!(!bandwidth.is_sub_ghz());
        let modulation = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf7,
            bandwidth,
            coding_rate: CodingRate::Cr45,
        };
        let profile = profile.with_modulation(modulation).unwrap();
        let half_width = hz.div_ceil(2);
        for frequency in [2_400_000_000 + half_width, 2_483_500_000 - half_width] {
            assert!(profile.with_frequency(Frequency::new(frequency)).is_ok());
        }
        for frequency in [
            2_400_000_000 + half_width - 1,
            2_483_500_000 - half_width + 1,
            0,
            u32::MAX,
        ] {
            assert_eq!(
                profile.with_frequency(Frequency::new(frequency)),
                Err(ChannelOutsideBand)
            );
        }
        for sf in 5..=12 {
            let sf = SpreadingFactor::from_number(sf).unwrap();
            let modulation = Modulation::Lora {
                spreading_factor: sf,
                bandwidth,
                coding_rate: CodingRate::Cr45,
            };
            let changed = profile.with_modulation(modulation).unwrap();
            let minimum = if (sf as u8) <= 6 { 12 } else { 8 };
            assert!(changed.with_preamble(PreambleSymbols::new(minimum)).is_ok());
            assert_eq!(
                changed.with_preamble(PreambleSymbols::new(minimum - 1)),
                Err(PreambleTooShort)
            );
        }
    }
    for power in [-18, 13] {
        assert!(profile.with_tx_power(TxPower::new(power)).is_ok());
    }
    for power in [-19, 14, i8::MIN, i8::MAX] {
        assert_eq!(
            profile.with_tx_power(TxPower::new(power)),
            Err(TransmitPowerOutsideRange)
        );
    }
    assert_eq!(
        profile.with_modulation(US915_AUTO_LORA_PROFILE.modulation()),
        Err(BandwidthOutsideBand)
    );
    assert!(US915_AUTO_LORA_PROFILE
        .with_modulation(profile.modulation())
        .is_err());
    assert!(Ghz24Profile::frequency_range()
        .contains_nominal_channel(profile.frequency(), profile.modulation().bandwidth().hz()));
}

#[cfg(feature = "lora-2g4")]
#[test]
fn malformed_or_cross_band_inventory_never_constructs_a_profile() {
    for text in [
        "",
        "G",
        "G,2445000000",
        "G,2445000000,7,8,5,10",
        "G,2445000000,7,8,5,10,18,",
        "G,x,7,8,5,10,18",
        "G,2445000000,x,8,5,10,18",
        "G,2445000000,4,8,5,10,18",
        "G,2445000000,7,x,5,10,18",
        "G,2445000000,7,99,5,10,18",
        "G,2445000000,7,8,x,10,18",
        "G,2445000000,7,8,9,10,18",
        "G,2445000000,7,8,5,x,18",
        "G,2445000000,7,8,5,10,x",
        "G,915000000,7,8,5,10,18",
        "G,2445000000,7,1,5,10,18",
    ] {
        assert_eq!(Ghz24Profile::parse_inventory_config(text), None, "{text}");
    }
}

#[test]
fn band_profiles_preserve_fixed_airtime_limits_and_reject_invalid_limits() {
    use crate::interfaces::AirtimeDutyCycle;
    let profiles = [
        LoRaProfile::SubG(US915_AUTO_LORA_PROFILE),
        #[cfg(feature = "lora-2g4")]
        LoRaProfile::Ghz24(GHZ24_BALANCED_PROFILE),
    ];
    let limit = AirtimeDutyCycle {
        limit_short_per_mille: Some(10),
        limit_long_per_mille: Some(5),
        max_queued_airtime_ms: 2_000,
    };
    for profile in profiles {
        assert_eq!(
            profile.airtime_policy(AirtimePolicy::Fixed(Some(limit))),
            Ok(Some(limit))
        );
        let invalid = AirtimeDutyCycle {
            limit_short_per_mille: Some(1001),
            ..limit
        };
        assert!(profile
            .airtime_policy(AirtimePolicy::Fixed(Some(invalid)))
            .is_err());
    }
}

#[cfg(feature = "lora-2g4")]
#[test]
fn ghz24_airtime_matches_the_vendor_short_interleaver_for_every_phy_payload_size() {
    for sf in 5..=12 {
        for (bandwidth, hz) in [
            (LoraBandwidth::Bw203kHz, 203_000),
            (LoraBandwidth::Bw406kHz, 406_000),
            (LoraBandwidth::Bw812kHz, 812_000),
        ] {
            for coding_rate in [
                CodingRate::Cr45,
                CodingRate::Cr46,
                CodingRate::Cr47,
                CodingRate::Cr48,
            ] {
                let modulation = Modulation::Lora {
                    spreading_factor: SpreadingFactor::from_number(sf).unwrap(),
                    bandwidth,
                    coding_rate,
                };
                for preamble in [18, u16::MAX] {
                    let profile = GHZ24_BALANCED_PROFILE
                        .with_modulation(modulation)
                        .unwrap()
                        .with_preamble(PreambleSymbols::new(preamble))
                        .unwrap();
                    assert_eq!(
                        LoRaProfile::parse_inventory_config(profile.inventory_config().as_str()),
                        Some(LoRaProfile::Ghz24(profile))
                    );
                    for bytes in 0..=255 {
                        // An independently expressed floating-point oracle for SWDR001's
                        // explicit-header, CRC-on, short-interleaver branch.
                        let fine = if sf <= 6 { 1.0 } else { 0.0 };
                        let sf_float = f64::from(sf);
                        let ldro = if (2.0f64.powi(i32::from(sf)) / f64::from(hz)) > 0.016 {
                            2.0
                        } else {
                            0.0
                        };
                        let bits = ((bytes + 2) as f64 * 8.0
                            - (sf_float * 4.0 + fine * 8.0 - 28.0))
                            .max(0.0);
                        let payload = (bits / (4.0 * (sf_float - ldro))).ceil()
                            * f64::from(coding_rate.denominator())
                            + 8.0;
                        let numerator = (4.0 * (f64::from(preamble) + 4.0 + 2.0 * fine + payload)
                            + 1.0)
                            * 2.0f64.powi(i32::from(sf) - 2)
                            - 1.0;
                        let expected = (numerator * 1_000_000.0 / f64::from(hz)).ceil() as u64;
                        assert_eq!(profile.time_on_air_us(bytes), expected);
                        assert_eq!(LoRaProfile::Ghz24(profile).time_on_air_us(bytes), expected);
                    }
                }
            }
        }
    }
    assert_eq!(GHZ24_BALANCED_PROFILE.time_on_air_us(usize::MAX), u64::MAX);
}
