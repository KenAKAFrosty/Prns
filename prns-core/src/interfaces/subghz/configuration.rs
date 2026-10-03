use crate::interfaces::lora::{
    CodingRate, LoraBandwidth, Modulation, PreambleSymbols, RadioProfile, RadioProfileError,
    SpreadingFactor,
};
use crate::interfaces::AirtimeDutyCycle;

use super::{Frequency, FrequencyRange, SubGRegion, TxPower};

const DUTY_QUEUE_BUDGET_MS: u32 = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubGMode {
    AutoLoRa,
    ManualLoRa,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportedSubGModes(u8);

impl SupportedSubGModes {
    const AUTO_LORA: u8 = 1 << 0;
    const MANUAL_LORA: u8 = 1 << 1;

    pub const fn manual_lora_only() -> Self {
        Self(Self::MANUAL_LORA)
    }

    pub const fn auto_and_manual_lora() -> Self {
        Self(Self::AUTO_LORA | Self::MANUAL_LORA)
    }

    pub const fn supports(self, mode: SubGMode) -> bool {
        let mask = match mode {
            SubGMode::AutoLoRa => Self::AUTO_LORA,
            SubGMode::ManualLoRa => Self::MANUAL_LORA,
        };
        self.0 & mask != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualLoRaParameters {
    frequency: Frequency,
    modulation: Modulation,
    tx_power: TxPower,
    preamble: PreambleSymbols,
}

impl ManualLoRaParameters {
    pub const fn new(
        frequency: Frequency,
        modulation: Modulation,
        tx_power: TxPower,
        preamble: PreambleSymbols,
    ) -> Self {
        Self {
            frequency,
            modulation,
            tx_power,
            preamble,
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
}

impl From<RadioProfile> for ManualLoRaParameters {
    fn from(profile: RadioProfile) -> Self {
        Self::new(
            profile.frequency(),
            profile.modulation(),
            profile.tx_power(),
            profile.preamble(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionalDutyCycle {
    NoModeledLimit,
    Modeled(AirtimeDutyCycle),
}

impl RegionalDutyCycle {
    pub const fn limit(self) -> Option<AirtimeDutyCycle> {
        match self {
            Self::NoModeledLimit => None,
            Self::Modeled(limit) => Some(limit),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoLoRaAvailability {
    Unavailable,
    Available(ManualLoRaParameters),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionalSubGSpec {
    region: SubGRegion,
    label: &'static str,
    frequency_range: FrequencyRange,
    manual_lora_defaults: ManualLoRaParameters,
    maximum_tx_power: TxPower,
    duty_cycle: RegionalDutyCycle,
    auto_lora: AutoLoRaAvailability,
}

impl RegionalSubGSpec {
    pub(crate) const fn new(
        region: SubGRegion,
        label: &'static str,
        frequency_range: FrequencyRange,
        manual_lora_defaults: ManualLoRaParameters,
        maximum_tx_power: TxPower,
        duty_cycle: RegionalDutyCycle,
        auto_lora: AutoLoRaAvailability,
    ) -> Self {
        Self {
            region,
            label,
            frequency_range,
            manual_lora_defaults,
            maximum_tx_power,
            duty_cycle,
            auto_lora,
        }
    }

    pub const fn region(self) -> SubGRegion {
        self.region
    }

    pub const fn label(self) -> &'static str {
        self.label
    }

    pub const fn frequency_range(self) -> FrequencyRange {
        self.frequency_range
    }

    pub const fn manual_lora_defaults(self) -> ManualLoRaParameters {
        self.manual_lora_defaults
    }

    pub const fn maximum_tx_power(self) -> TxPower {
        self.maximum_tx_power
    }

    pub const fn duty_cycle(self) -> RegionalDutyCycle {
        self.duty_cycle
    }

    pub const fn auto_lora(self) -> AutoLoRaAvailability {
        self.auto_lora
    }

    pub const fn supported_modes(self) -> SupportedSubGModes {
        match self.auto_lora {
            AutoLoRaAvailability::Unavailable => SupportedSubGModes::manual_lora_only(),
            AutoLoRaAvailability::Available(_) => SupportedSubGModes::auto_and_manual_lora(),
        }
    }
}

const MANUAL_DEFAULT_MODULATION: Modulation = Modulation::Lora {
    spreading_factor: SpreadingFactor::Sf7,
    bandwidth: LoraBandwidth::Bw125kHz,
    coding_rate: CodingRate::Cr45,
};
const MANUAL_DEFAULT_PREAMBLE: PreambleSymbols = PreambleSymbols::new(18);

pub(crate) const fn manual_defaults(frequency_hz: u32, power_dbm: i8) -> ManualLoRaParameters {
    ManualLoRaParameters::new(
        Frequency::new(frequency_hz),
        MANUAL_DEFAULT_MODULATION,
        TxPower::new(power_dbm),
        MANUAL_DEFAULT_PREAMBLE,
    )
}

pub(crate) const fn one_percent_duty_cycle() -> RegionalDutyCycle {
    RegionalDutyCycle::Modeled(AirtimeDutyCycle {
        limit_short_per_mille: None,
        limit_long_per_mille: Some(10),
        max_queued_airtime_ms: DUTY_QUEUE_BUDGET_MS,
    })
}

pub(crate) const fn ten_percent_duty_cycle() -> RegionalDutyCycle {
    RegionalDutyCycle::Modeled(AirtimeDutyCycle {
        limit_short_per_mille: None,
        limit_long_per_mille: Some(100),
        max_queued_airtime_ms: DUTY_QUEUE_BUDGET_MS,
    })
}

pub(crate) const CUSTOM_SUBG_SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Custom,
    "Custom",
    FrequencyRange::from_ordered_hz(150_000_000, 960_000_000),
    manual_defaults(915_000_000, 22),
    TxPower::new(22),
    RegionalDutyCycle::NoModeledLimit,
    AutoLoRaAvailability::Unavailable,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubGConfigurationKind {
    AutoLoRa(RadioProfile),
    ManualLoRa(RadioProfile),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubGConfiguration {
    kind: SubGConfigurationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSubGMode {
    LoRa(RadioProfile),
}

impl SubGConfiguration {
    pub const fn auto_lora_for(region: SubGRegion) -> Result<Self, SubGConfigurationError> {
        let availability = match region {
            SubGRegion::Regulated(region) => {
                crate::interfaces::subghz::regions::specification(region).auto_lora()
            }
            SubGRegion::Custom => CUSTOM_SUBG_SPEC.auto_lora(),
        };
        match availability {
            AutoLoRaAvailability::Available(parameters) => match RadioProfile::new(
                region,
                parameters.frequency(),
                parameters.modulation(),
                parameters.tx_power(),
                parameters.preamble(),
            ) {
                Ok(profile) => Ok(Self::auto_lora(profile)),
                Err(error) => Err(SubGConfigurationError::InvalidLoRaProfile(error)),
            },
            AutoLoRaAvailability::Unavailable => {
                Err(SubGConfigurationError::AutoLoRaUnavailable { region })
            }
        }
    }

    pub fn manual_lora_for(
        region: SubGRegion,
        parameters: ManualLoRaParameters,
    ) -> Result<Self, SubGConfigurationError> {
        let profile = RadioProfile::new(
            region,
            parameters.frequency(),
            parameters.modulation(),
            parameters.tx_power(),
            parameters.preamble(),
        )
        .map_err(SubGConfigurationError::InvalidLoRaProfile)?;
        Ok(Self::manual_lora(profile))
    }

    pub const fn region(self) -> SubGRegion {
        match self.kind {
            SubGConfigurationKind::AutoLoRa(profile)
            | SubGConfigurationKind::ManualLoRa(profile) => profile.region(),
        }
    }

    pub const fn mode(self) -> SubGMode {
        match self.kind {
            SubGConfigurationKind::AutoLoRa(_) => SubGMode::AutoLoRa,
            SubGConfigurationKind::ManualLoRa(_) => SubGMode::ManualLoRa,
        }
    }

    pub const fn resolve(self) -> ResolvedSubGMode {
        match self.kind {
            SubGConfigurationKind::AutoLoRa(profile)
            | SubGConfigurationKind::ManualLoRa(profile) => ResolvedSubGMode::LoRa(profile),
        }
    }

    pub(crate) const fn auto_lora(profile: RadioProfile) -> Self {
        Self {
            kind: SubGConfigurationKind::AutoLoRa(profile),
        }
    }

    pub const fn manual_lora(profile: RadioProfile) -> Self {
        Self {
            kind: SubGConfigurationKind::ManualLoRa(profile),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubGConfigurationState {
    Unconfigured,
    Configured(SubGConfiguration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubGConfigurationError {
    AutoLoRaUnavailable { region: SubGRegion },
    InvalidLoRaProfile(RadioProfileError),
}

pub(super) mod sealed {
    pub trait RegionalSubGPolicy {}
}

pub trait RegionalSubGPolicy: sealed::RegionalSubGPolicy {
    const SPEC: RegionalSubGSpec;

    fn manual_lora(
        parameters: ManualLoRaParameters,
    ) -> Result<SubGConfiguration, SubGConfigurationError> {
        SubGConfiguration::manual_lora_for(Self::SPEC.region(), parameters)
    }
}

pub struct CustomSubGPolicy;

impl CustomSubGPolicy {
    pub fn manual_lora(
        parameters: ManualLoRaParameters,
    ) -> Result<SubGConfiguration, SubGConfigurationError> {
        SubGConfiguration::manual_lora_for(SubGRegion::Custom, parameters)
    }
}

pub const fn supported_modes(region: SubGRegion) -> SupportedSubGModes {
    match region {
        SubGRegion::Regulated(region) => {
            crate::interfaces::subghz::regions::specification(region).supported_modes()
        }
        SubGRegion::Custom => CUSTOM_SUBG_SPEC.supported_modes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::subghz::regions;
    use crate::interfaces::subghz::RegulatoryRegion;

    #[test]
    fn every_regional_spec_constructs_its_manual_default_and_matches_its_registry_key() {
        for region in RegulatoryRegion::ALL {
            let expected_region = SubGRegion::Regulated(region);
            let specification = regions::specification(region);
            assert_eq!(specification.region(), expected_region);
            assert_eq!(specification.label(), region.label());
            assert!(specification
                .supported_modes()
                .supports(SubGMode::ManualLoRa));
            let configuration = SubGConfiguration::manual_lora_for(
                expected_region,
                specification.manual_lora_defaults(),
            )
            .unwrap();
            assert_eq!(configuration.region(), expected_region);
            assert_eq!(configuration.mode(), SubGMode::ManualLoRa);
            let ResolvedSubGMode::LoRa(profile) = configuration.resolve();
            assert_eq!(profile.validate(), Ok(()));
            assert!(profile.tx_power().dbm() <= specification.maximum_tx_power().dbm());
        }
    }

    #[test]
    fn auto_lora_availability_and_construction_have_one_regional_owner() {
        for region in RegulatoryRegion::ALL {
            let region = SubGRegion::Regulated(region);
            let modes = supported_modes(region);
            match SubGConfiguration::auto_lora_for(region) {
                Ok(configuration) => {
                    assert!(modes.supports(SubGMode::AutoLoRa));
                    assert_eq!(configuration.region(), region);
                    assert_eq!(configuration.mode(), SubGMode::AutoLoRa);
                }
                Err(SubGConfigurationError::AutoLoRaUnavailable {
                    region: unavailable,
                }) => {
                    assert_eq!(unavailable, region);
                    assert!(!modes.supports(SubGMode::AutoLoRa));
                }
                Err(SubGConfigurationError::InvalidLoRaProfile(_)) => {
                    panic!("regional Auto LoRa profiles are prevalidated")
                }
            }
        }
        assert_eq!(
            SubGConfiguration::auto_lora_for(SubGRegion::Custom),
            Err(SubGConfigurationError::AutoLoRaUnavailable {
                region: SubGRegion::Custom,
            })
        );
        assert_eq!(
            SubGConfiguration::auto_lora_for(SubGRegion::Regulated(RegulatoryRegion::Us915)),
            Ok(regions::us915::Us915::auto_lora())
        );
    }

    #[test]
    fn marker_specs_are_the_same_values_served_by_the_regional_registry() {
        assert_eq!(
            regions::au915::SPEC,
            regions::specification(RegulatoryRegion::Au915)
        );
        assert_eq!(
            regions::eu433::SPEC,
            regions::specification(RegulatoryRegion::Eu433)
        );
        assert_eq!(
            regions::eu865::SPEC,
            regions::specification(RegulatoryRegion::Eu865)
        );
        assert_eq!(
            regions::eu868::SPEC,
            regions::specification(RegulatoryRegion::Eu868)
        );
        assert_eq!(
            regions::eu869::SPEC,
            regions::specification(RegulatoryRegion::Eu869)
        );
        assert_eq!(
            regions::as923::SPEC,
            regions::specification(RegulatoryRegion::As923)
        );
        assert_eq!(
            regions::in865::SPEC,
            regions::specification(RegulatoryRegion::In865)
        );
        assert_eq!(
            regions::cn470::SPEC,
            regions::specification(RegulatoryRegion::Cn470)
        );
        assert_eq!(
            regions::kr920::SPEC,
            regions::specification(RegulatoryRegion::Kr920)
        );
        assert_eq!(
            regions::jp920::SPEC,
            regions::specification(RegulatoryRegion::Jp920)
        );
        assert_eq!(
            regions::us915::SPEC,
            regions::specification(RegulatoryRegion::Us915)
        );
    }

    #[test]
    fn occupied_channel_validation_accepts_edges_and_rejects_one_hertz_crossings() {
        let range = regions::us915::SPEC.frequency_range();
        let bandwidth_hz = LoraBandwidth::Bw500kHz.hz();
        assert!(range.contains_nominal_channel(Frequency::new(902_250_000), bandwidth_hz));
        assert!(range.contains_nominal_channel(Frequency::new(927_750_000), bandwidth_hz));
        assert!(!range.contains_nominal_channel(Frequency::new(902_249_999), bandwidth_hz));
        assert!(!range.contains_nominal_channel(Frequency::new(927_750_001), bandwidth_hz));
    }
}
