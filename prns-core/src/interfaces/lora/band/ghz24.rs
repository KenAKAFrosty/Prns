use super::super::{
    CodingRate, Frequency, LoraBandwidth, Modulation, PreambleSymbols, SpreadingFactor, TxPower,
    INVENTORY_CONFIG_CAP,
};
use crate::interfaces::subghz::FrequencyRange;
use core::fmt::Write;
use heapless::String;

const FREQUENCIES: FrequencyRange = FrequencyRange::from_ordered_hz(2_400_000_000, 2_483_500_000);
const MINIMUM_POWER_DBM: i8 = -18;
const MAXIMUM_POWER_DBM: i8 = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ghz24ProfileError {
    ChannelOutsideBand,
    BandwidthOutsideBand,
    TransmitPowerOutsideRange,
    PreambleTooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ghz24Profile {
    frequency: Frequency,
    modulation: Modulation,
    tx_power: TxPower,
    preamble: PreambleSymbols,
}

pub const GHZ24_BALANCED_PROFILE: Ghz24Profile = Ghz24Profile {
    frequency: Frequency::new(2_445_000_000),
    modulation: Modulation::Lora {
        spreading_factor: SpreadingFactor::Sf7,
        bandwidth: LoraBandwidth::Bw812kHz,
        coding_rate: CodingRate::Cr45,
    },
    tx_power: TxPower::new(10),
    preamble: PreambleSymbols::new(18),
};

impl Ghz24Profile {
    pub const fn new(
        frequency: Frequency,
        modulation: Modulation,
        tx_power: TxPower,
        preamble: PreambleSymbols,
    ) -> Result<Self, Ghz24ProfileError> {
        let profile = Self {
            frequency,
            modulation,
            tx_power,
            preamble,
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
    pub const fn frequency_range() -> FrequencyRange {
        FREQUENCIES
    }
    pub const fn validate(self) -> Result<(), Ghz24ProfileError> {
        if self.modulation.bandwidth().is_sub_ghz() {
            return Err(Ghz24ProfileError::BandwidthOutsideBand);
        }
        if !FREQUENCIES.contains_nominal_channel(self.frequency, self.modulation.bandwidth().hz()) {
            return Err(Ghz24ProfileError::ChannelOutsideBand);
        }
        if self.tx_power.dbm() < MINIMUM_POWER_DBM || self.tx_power.dbm() > MAXIMUM_POWER_DBM {
            return Err(Ghz24ProfileError::TransmitPowerOutsideRange);
        }
        let minimum_preamble = match self.modulation.spreading_factor() {
            SpreadingFactor::Sf5 | SpreadingFactor::Sf6 => 12,
            SpreadingFactor::Sf7
            | SpreadingFactor::Sf8
            | SpreadingFactor::Sf9
            | SpreadingFactor::Sf10
            | SpreadingFactor::Sf11
            | SpreadingFactor::Sf12 => 8,
        };
        if self.preamble.count() < minimum_preamble {
            return Err(Ghz24ProfileError::PreambleTooShort);
        }
        Ok(())
    }
    pub const fn with_frequency(self, frequency: Frequency) -> Result<Self, Ghz24ProfileError> {
        Self::new(frequency, self.modulation, self.tx_power, self.preamble)
    }
    pub const fn with_modulation(self, modulation: Modulation) -> Result<Self, Ghz24ProfileError> {
        Self::new(self.frequency, modulation, self.tx_power, self.preamble)
    }
    pub const fn with_tx_power(self, power: TxPower) -> Result<Self, Ghz24ProfileError> {
        Self::new(self.frequency, self.modulation, power, self.preamble)
    }
    pub const fn with_preamble(self, preamble: PreambleSymbols) -> Result<Self, Ghz24ProfileError> {
        Self::new(self.frequency, self.modulation, self.tx_power, preamble)
    }
    /// Explicit header, CRC, and short interleaving, following Semtech SWDR001's
    /// `lr11xx_radio_get_lora_time_on_air_numerator`. Round once to microseconds
    /// after rounding the coded payload to complete symbol blocks.
    pub const fn time_on_air_us(self, frame_bytes: usize) -> u64 {
        let sf = self.modulation.spreading_factor() as u128;
        let fine_sync = if sf <= 6 { 1 } else { 0 };
        let header_bits = 4 * sf + 8 * fine_sync - 28;
        let payload_bits = (8 * (frame_bytes as u128 + 2)).saturating_sub(header_bits);
        let bits_per_symbol = sf
            - if self.modulation.is_low_data_rate() {
                2
            } else {
                0
            };
        let coded_blocks = payload_bits.div_ceil(4 * bits_per_symbol);
        let data_symbols = coded_blocks * self.modulation.coding_rate().denominator() as u128 + 8;
        let symbols = self.preamble.count() as u128 + 4 + 2 * fine_sync + data_symbols;
        let numerator = ((4 * symbols + 1) * (1 << (sf - 2)) - 1) * 1_000_000;
        let duration = numerator.div_ceil(self.modulation.bandwidth().hz() as u128);
        if duration > u64::MAX as u128 {
            u64::MAX
        } else {
            duration as u64
        }
    }

    pub fn inventory_config(self) -> String<INVENTORY_CONFIG_CAP> {
        let mut text = String::new();
        // All fields have fixed integer bounds; the longest representation is 43 bytes.
        let _ = write!(
            text,
            "G,{},{},{},{},{},{}",
            self.frequency.hz(),
            self.modulation.spreading_factor() as u8,
            super::super::profile::bandwidth_inventory_code(self.modulation.bandwidth()),
            self.modulation.coding_rate().denominator(),
            self.tx_power.dbm(),
            self.preamble.count()
        );
        text
    }
    pub fn parse_inventory_config(text: &str) -> Option<Self> {
        let mut parts = text.strip_prefix("G,")?.split(',');
        let frequency = Frequency::new(parts.next()?.parse().ok()?);
        let spreading_factor = SpreadingFactor::from_number(parts.next()?.parse().ok()?)?;
        let bandwidth =
            super::super::profile::bandwidth_from_inventory_code(parts.next()?.parse().ok()?)?;
        let coding_rate = CodingRate::from_denominator(parts.next()?.parse().ok()?)?;
        let power = TxPower::new(parts.next()?.parse().ok()?);
        let preamble = PreambleSymbols::new(parts.next()?.parse().ok()?);
        if parts.next().is_some() {
            return None;
        }
        Self::new(
            frequency,
            Modulation::Lora {
                spreading_factor,
                bandwidth,
                coding_rate,
            },
            power,
            preamble,
        )
        .ok()
    }
}
