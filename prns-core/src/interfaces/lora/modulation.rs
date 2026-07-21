use crate::interfaces::{SignalQualityTenthsPercent, SnrQuarterDb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SpreadingFactor {
    Sf5 = 5,
    Sf6 = 6,
    Sf7 = 7,
    Sf8 = 8,
    Sf9 = 9,
    Sf10 = 10,
    Sf11 = 11,
    Sf12 = 12,
}

impl SpreadingFactor {
    #[cfg(feature = "alloc")]
    pub(crate) const fn from_number(value: u8) -> Option<Self> {
        match value {
            5 => Some(Self::Sf5),
            6 => Some(Self::Sf6),
            7 => Some(Self::Sf7),
            8 => Some(Self::Sf8),
            9 => Some(Self::Sf9),
            10 => Some(Self::Sf10),
            11 => Some(Self::Sf11),
            12 => Some(Self::Sf12),
            _ => None,
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Sf5 => Self::Sf6,
            Self::Sf6 => Self::Sf7,
            Self::Sf7 => Self::Sf8,
            Self::Sf8 => Self::Sf9,
            Self::Sf9 => Self::Sf10,
            Self::Sf10 => Self::Sf11,
            Self::Sf11 => Self::Sf12,
            Self::Sf12 => Self::Sf5,
        }
    }

    pub fn signal_quality(self, snr: SnrQuarterDb) -> Option<SignalQualityTenthsPercent> {
        let spreading_factor = i32::from(self as u8);
        let minimum_quarters = (5 - 2 * spreading_factor) * 4;
        let span_db = 1 + 2 * spreading_factor;
        let numerator_quarters = i32::from(snr.quarters()) - minimum_quarters;
        let tenths_percent = if numerator_quarters <= 0 {
            0
        } else {
            ((numerator_quarters * 250 + span_db / 2) / span_db).min(1_000)
        };
        SignalQualityTenthsPercent::new(tenths_percent as u16)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoraBandwidth {
    Bw125kHz,
    Bw250kHz,
    Bw500kHz,
}

impl LoraBandwidth {
    pub const fn hz(self) -> u32 {
        match self {
            Self::Bw125kHz => 125_000,
            Self::Bw250kHz => 250_000,
            Self::Bw500kHz => 500_000,
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Bw125kHz => Self::Bw250kHz,
            Self::Bw250kHz => Self::Bw500kHz,
            Self::Bw500kHz => Self::Bw125kHz,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodingRate {
    Cr45 = 5,
    Cr46 = 6,
    Cr47 = 7,
    Cr48 = 8,
}

impl CodingRate {
    pub const fn denominator(self) -> u8 {
        self as u8
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Cr45 => Self::Cr46,
            Self::Cr46 => Self::Cr47,
            Self::Cr47 => Self::Cr48,
            Self::Cr48 => Self::Cr45,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    Lora {
        spreading_factor: SpreadingFactor,
        bandwidth: LoraBandwidth,
        coding_rate: CodingRate,
    },
}

#[must_use]
pub const fn nominal_lora_bitrate_bps(
    spreading_factor: u8,
    coding_rate: u8,
    bandwidth_hz: u32,
) -> u32 {
    let sf = spreading_factor as u64;
    let cr = coding_rate as u64;
    let bw = bandwidth_hz as u64;
    if sf == 0 || cr == 0 {
        return 0;
    }
    ((sf * bw * 4) / ((1u64 << sf) * cr)) as u32
}

impl Modulation {
    pub const fn spreading_factor(self) -> SpreadingFactor {
        let Self::Lora {
            spreading_factor, ..
        } = self;
        spreading_factor
    }

    pub const fn nominal_bitrate_bps(self) -> u32 {
        let Self::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        } = self;
        nominal_lora_bitrate_bps(spreading_factor as u8, coding_rate as u8, bandwidth.hz())
    }

    pub const fn is_low_data_rate(&self) -> bool {
        let Self::Lora {
            spreading_factor,
            bandwidth,
            ..
        } = self;
        matches!(
            (spreading_factor, bandwidth),
            (
                SpreadingFactor::Sf11 | SpreadingFactor::Sf12,
                LoraBandwidth::Bw125kHz
            ) | (SpreadingFactor::Sf12, LoraBandwidth::Bw250kHz)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lora_nominal_bitrate_matches_the_standard_formula() {
        let slow = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf8,
            bandwidth: LoraBandwidth::Bw125kHz,
            coding_rate: CodingRate::Cr45,
        };
        assert_eq!(slow.nominal_bitrate_bps(), 3125);
        let fast = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf7,
            bandwidth: LoraBandwidth::Bw500kHz,
            coding_rate: CodingRate::Cr45,
        };
        assert_eq!(fast.nominal_bitrate_bps(), 21875);
    }

    #[test]
    fn low_data_rate_covers_exactly_the_slow_combos() {
        let slow_combos = [
            (SpreadingFactor::Sf11, LoraBandwidth::Bw125kHz),
            (SpreadingFactor::Sf12, LoraBandwidth::Bw125kHz),
            (SpreadingFactor::Sf12, LoraBandwidth::Bw250kHz),
        ];
        for sf in [
            SpreadingFactor::Sf5,
            SpreadingFactor::Sf6,
            SpreadingFactor::Sf7,
            SpreadingFactor::Sf8,
            SpreadingFactor::Sf9,
            SpreadingFactor::Sf10,
            SpreadingFactor::Sf11,
            SpreadingFactor::Sf12,
        ] {
            for bandwidth in [
                LoraBandwidth::Bw125kHz,
                LoraBandwidth::Bw250kHz,
                LoraBandwidth::Bw500kHz,
            ] {
                let modulation = Modulation::Lora {
                    spreading_factor: sf,
                    bandwidth,
                    coding_rate: CodingRate::Cr45,
                };
                assert_eq!(
                    modulation.is_low_data_rate(),
                    slow_combos.contains(&(sf, bandwidth)),
                );
            }
        }
    }

    #[test]
    fn modulation_settings_cycle_through_all_values() {
        let mut sf = SpreadingFactor::Sf5;
        for _ in 0..8 {
            sf = sf.next();
        }
        assert_eq!(sf, SpreadingFactor::Sf5);
        assert_eq!(SpreadingFactor::Sf12.next(), SpreadingFactor::Sf5);

        let mut bw = LoraBandwidth::Bw125kHz;
        for _ in 0..3 {
            bw = bw.next();
        }
        assert_eq!(bw, LoraBandwidth::Bw125kHz);

        let mut cr = CodingRate::Cr45;
        for _ in 0..4 {
            cr = cr.next();
        }
        assert_eq!(cr, CodingRate::Cr45);
    }
}
