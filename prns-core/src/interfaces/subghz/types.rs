use crate::interfaces::AirtimeDutyCycle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
pub struct FrequencyRange {
    minimum: Frequency,
    maximum: Frequency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrequencyRangeError {
    EmptyOrReversed,
}

impl FrequencyRange {
    pub const fn new(minimum: Frequency, maximum: Frequency) -> Result<Self, FrequencyRangeError> {
        if minimum.hz() >= maximum.hz() {
            return Err(FrequencyRangeError::EmptyOrReversed);
        }
        Ok(Self { minimum, maximum })
    }

    pub(crate) const fn from_ordered_hz(minimum_hz: u32, maximum_hz: u32) -> Self {
        assert!(minimum_hz < maximum_hz);
        Self {
            minimum: Frequency::new(minimum_hz),
            maximum: Frequency::new(maximum_hz),
        }
    }

    pub const fn minimum(self) -> Frequency {
        self.minimum
    }

    pub const fn maximum(self) -> Frequency {
        self.maximum
    }

    pub const fn contains_nominal_channel(self, center: Frequency, bandwidth_hz: u32) -> bool {
        let doubled_center = center.hz() as u64 * 2;
        let doubled_minimum = self.minimum.hz() as u64 * 2;
        let doubled_maximum = self.maximum.hz() as u64 * 2;
        doubled_center >= doubled_minimum + bandwidth_hz as u64
            && doubled_center + bandwidth_hz as u64 <= doubled_maximum
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonotonicMicros(u64);

impl MonotonicMicros {
    pub const fn new(micros: u64) -> Self {
        Self(micros)
    }

    pub const fn micros(self) -> u64 {
        self.0
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RegulatoryRegion {
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
    }
}

impl RegulatoryRegion {
    pub const fn frequency_range(self) -> FrequencyRange {
        crate::interfaces::subghz::regions::specification(self).frequency_range()
    }

    pub const fn default_frequency(self) -> Frequency {
        crate::interfaces::subghz::regions::specification(self)
            .manual_lora_defaults()
            .frequency()
    }

    pub const fn max_tx_power(self) -> TxPower {
        crate::interfaces::subghz::regions::specification(self).maximum_tx_power()
    }

    pub const fn regulatory_duty_cycle(self) -> Option<AirtimeDutyCycle> {
        crate::interfaces::subghz::regions::specification(self)
            .duty_cycle()
            .limit()
    }

    pub const fn label(self) -> &'static str {
        crate::interfaces::subghz::regions::specification(self).label()
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
            Self::Jp920 => Self::Us915,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubGRegion {
    Regulated(RegulatoryRegion),
    Custom,
}

impl SubGRegion {
    pub const fn frequency_range(self) -> FrequencyRange {
        match self {
            Self::Regulated(region) => region.frequency_range(),
            Self::Custom => {
                crate::interfaces::subghz::configuration::CUSTOM_SUBG_SPEC.frequency_range()
            }
        }
    }

    pub const fn default_frequency(self) -> Frequency {
        self.manual_lora_defaults().frequency()
    }

    pub const fn manual_lora_defaults(self) -> crate::interfaces::subghz::ManualLoRaParameters {
        match self {
            Self::Regulated(region) => {
                crate::interfaces::subghz::regions::specification(region).manual_lora_defaults()
            }
            Self::Custom => {
                crate::interfaces::subghz::configuration::CUSTOM_SUBG_SPEC.manual_lora_defaults()
            }
        }
    }

    pub const fn max_tx_power(self) -> TxPower {
        match self {
            Self::Regulated(region) => region.max_tx_power(),
            Self::Custom => {
                crate::interfaces::subghz::configuration::CUSTOM_SUBG_SPEC.maximum_tx_power()
            }
        }
    }

    pub const fn regulatory_duty_cycle(self) -> Option<AirtimeDutyCycle> {
        match self {
            Self::Regulated(region) => region.regulatory_duty_cycle(),
            Self::Custom => crate::interfaces::subghz::configuration::CUSTOM_SUBG_SPEC
                .duty_cycle()
                .limit(),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Regulated(region) => region.label(),
            Self::Custom => crate::interfaces::subghz::configuration::CUSTOM_SUBG_SPEC.label(),
        }
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn contained_channel_centers_never_escape_the_frequency_range() {
        let minimum_hz: u32 = kani::any();
        let maximum_hz: u32 = kani::any();
        let center_hz: u32 = kani::any();
        let bandwidth_hz: u32 = kani::any();
        kani::assume(minimum_hz < maximum_hz);
        let range =
            FrequencyRange::new(Frequency::new(minimum_hz), Frequency::new(maximum_hz)).unwrap();
        if range.contains_nominal_channel(Frequency::new(center_hz), bandwidth_hz) {
            assert!(center_hz >= minimum_hz);
            assert!(center_hz <= maximum_hz);
        }
    }

    #[kani::proof]
    fn accepting_a_wider_channel_also_accepts_every_narrower_channel() {
        let minimum_hz: u32 = kani::any();
        let maximum_hz: u32 = kani::any();
        let center_hz: u32 = kani::any();
        let narrower_hz: u32 = kani::any();
        let wider_hz: u32 = kani::any();
        kani::assume(minimum_hz < maximum_hz);
        kani::assume(narrower_hz <= wider_hz);
        let range =
            FrequencyRange::new(Frequency::new(minimum_hz), Frequency::new(maximum_hz)).unwrap();
        if range.contains_nominal_channel(Frequency::new(center_hz), wider_hz) {
            assert!(range.contains_nominal_channel(Frequency::new(center_hz), narrower_hz));
        }
    }
}
