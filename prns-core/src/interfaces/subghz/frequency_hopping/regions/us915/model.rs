use core::array;

use super::super::super::super::{Frequency, Region};
use super::super::super::{HoppingRegion, ObservationWindow};

const FCC_CHANNEL_CLASS_THRESHOLD_HZ: u32 = 250_000;
const FCC_MAXIMUM_HOPPING_CHANNEL_BANDWIDTH_HZ: u32 = 500_000;
const FCC_MINIMUM_CARRIER_SEPARATION_HZ: u32 = 25_000;
const FCC_NARROW_MINIMUM_CHANNEL_COUNT: usize = 50;
const FCC_NARROW_OBSERVATION_WINDOW_US: u64 = 20_000_000;
const FCC_WIDE_MINIMUM_CHANNEL_COUNT: usize = 25;
const FCC_WIDE_OBSERVATION_WINDOW_US: u64 = 10_000_000;
const FCC_REGULATORY_MAXIMUM_CHANNEL_OCCUPANCY_US: u64 = 400_000;
const FCC_HIGH_POWER_MINIMUM_CHANNEL_COUNT: usize = 50;
const FCC_QUARTER_WATT_MILLIWATTS: u16 = 250;
const FCC_ONE_WATT_MILLIWATTS: u16 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasuredTwentyDbBandwidth(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasuredTwentyDbBandwidthError {
    Zero,
}

impl MeasuredTwentyDbBandwidth {
    pub const fn new(hz: u32) -> Result<Self, MeasuredTwentyDbBandwidthError> {
        if hz == 0 {
            return Err(MeasuredTwentyDbBandwidthError::Zero);
        }
        Ok(Self(hz))
    }

    pub const fn hz(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelOccupancyLimit(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelOccupancyLimitError {
    Zero,
    ExceedsRegulatoryMaximum { requested_us: u64, maximum_us: u64 },
}

impl ChannelOccupancyLimit {
    pub const fn new(micros: u64) -> Result<Self, ChannelOccupancyLimitError> {
        if micros == 0 {
            return Err(ChannelOccupancyLimitError::Zero);
        }
        if micros > FCC_REGULATORY_MAXIMUM_CHANNEL_OCCUPANCY_US {
            return Err(ChannelOccupancyLimitError::ExceedsRegulatoryMaximum {
                requested_us: micros,
                maximum_us: FCC_REGULATORY_MAXIMUM_CHANNEL_OCCUPANCY_US,
            });
        }
        Ok(Self(micros))
    }

    pub const fn micros(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Us915HoppingModel {
    measured_twenty_db_bandwidth: MeasuredTwentyDbBandwidth,
    channel_occupancy_limit: ChannelOccupancyLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Us915HoppingModelError {
    ChannelBandwidthExceedsMaximum { measured_hz: u32, maximum_hz: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Us915PowerClass {
    QuarterWatt,
    OneWatt,
}

impl Us915PowerClass {
    pub const fn maximum_conducted_milliwatts(self) -> u16 {
        match self {
            Self::QuarterWatt => FCC_QUARTER_WATT_MILLIWATTS,
            Self::OneWatt => FCC_ONE_WATT_MILLIWATTS,
        }
    }

    pub const fn maximum_whole_dbm(self) -> i8 {
        match self {
            Self::QuarterWatt => 23,
            Self::OneWatt => 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConductedPowerDbm(i8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConductedPowerError {
    OutsideRepresentableRange { dbm: i8 },
}

impl ConductedPowerDbm {
    pub const fn new(dbm: i8) -> Result<Self, ConductedPowerError> {
        if dbm < -20 || dbm > 30 {
            return Err(ConductedPowerError::OutsideRepresentableRange { dbm });
        }
        Ok(Self(dbm))
    }

    pub const fn dbm(self) -> i8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AntennaGainDeciDb(u16);

impl AntennaGainDeciDb {
    pub const fn new(deci_db: u16) -> Self {
        Self(deci_db)
    }

    pub const fn deci_db(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Us915PowerInputs {
    radio_maximum: ConductedPowerDbm,
    antenna_gain: AntennaGainDeciDb,
    laboratory_maximum: ConductedPowerDbm,
}

impl Us915PowerInputs {
    pub const fn new(
        radio_maximum: ConductedPowerDbm,
        antenna_gain: AntennaGainDeciDb,
        laboratory_maximum: ConductedPowerDbm,
    ) -> Self {
        Self {
            radio_maximum,
            antenna_gain,
            laboratory_maximum,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Us915PowerBudget {
    class: Us915PowerClass,
    selected: ConductedPowerDbm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Us915PowerBudgetError {
    EmptyHopSet,
    AntennaAdjustmentExceedsPowerRange { excess_gain_db: u16 },
}

impl Us915PowerBudget {
    pub const fn for_hop_count(
        channel_count: usize,
        inputs: Us915PowerInputs,
    ) -> Result<Self, Us915PowerBudgetError> {
        if channel_count == 0 {
            return Err(Us915PowerBudgetError::EmptyHopSet);
        }
        let class = if channel_count >= FCC_HIGH_POWER_MINIMUM_CHANNEL_COUNT {
            Us915PowerClass::OneWatt
        } else {
            Us915PowerClass::QuarterWatt
        };
        let excess_gain_deci_db = inputs.antenna_gain.deci_db().saturating_sub(60);
        let excess_gain_db = excess_gain_deci_db.div_ceil(10);
        if excess_gain_db > 50 {
            return Err(Us915PowerBudgetError::AntennaAdjustmentExceedsPowerRange {
                excess_gain_db,
            });
        }
        let Some(antenna_adjusted_dbm) =
            class.maximum_whole_dbm().checked_sub(excess_gain_db as i8)
        else {
            return Err(Us915PowerBudgetError::AntennaAdjustmentExceedsPowerRange {
                excess_gain_db,
            });
        };
        if antenna_adjusted_dbm < -20 {
            return Err(Us915PowerBudgetError::AntennaAdjustmentExceedsPowerRange {
                excess_gain_db,
            });
        }
        let selected_dbm = if antenna_adjusted_dbm < inputs.radio_maximum.dbm() {
            antenna_adjusted_dbm
        } else {
            inputs.radio_maximum.dbm()
        };
        let selected_dbm = if selected_dbm < inputs.laboratory_maximum.dbm() {
            selected_dbm
        } else {
            inputs.laboratory_maximum.dbm()
        };
        Ok(Self {
            class,
            selected: ConductedPowerDbm(selected_dbm),
        })
    }

    pub const fn class(self) -> Us915PowerClass {
        self.class
    }

    pub const fn selected(self) -> ConductedPowerDbm {
        self.selected
    }
}

impl Us915HoppingModel {
    pub const fn new(
        measured_twenty_db_bandwidth: MeasuredTwentyDbBandwidth,
        channel_occupancy_limit: ChannelOccupancyLimit,
    ) -> Result<Self, Us915HoppingModelError> {
        if measured_twenty_db_bandwidth.hz() > FCC_MAXIMUM_HOPPING_CHANNEL_BANDWIDTH_HZ {
            return Err(Us915HoppingModelError::ChannelBandwidthExceedsMaximum {
                measured_hz: measured_twenty_db_bandwidth.hz(),
                maximum_hz: FCC_MAXIMUM_HOPPING_CHANNEL_BANDWIDTH_HZ,
            });
        }
        Ok(Self {
            measured_twenty_db_bandwidth,
            channel_occupancy_limit,
        })
    }

    pub const fn measured_twenty_db_bandwidth(self) -> MeasuredTwentyDbBandwidth {
        self.measured_twenty_db_bandwidth
    }

    pub const fn channel_occupancy_limit(self) -> ChannelOccupancyLimit {
        self.channel_occupancy_limit
    }

    pub const fn regulatory_maximum_channel_occupancy_us(self) -> u64 {
        FCC_REGULATORY_MAXIMUM_CHANNEL_OCCUPANCY_US
    }

    pub const fn minimum_channel_count(self) -> usize {
        if self.measured_twenty_db_bandwidth.hz() < FCC_CHANNEL_CLASS_THRESHOLD_HZ {
            FCC_NARROW_MINIMUM_CHANNEL_COUNT
        } else {
            FCC_WIDE_MINIMUM_CHANNEL_COUNT
        }
    }

    pub const fn observation_window_us(self) -> u64 {
        if self.measured_twenty_db_bandwidth.hz() < FCC_CHANNEL_CLASS_THRESHOLD_HZ {
            FCC_NARROW_OBSERVATION_WINDOW_US
        } else {
            FCC_WIDE_OBSERVATION_WINDOW_US
        }
    }

    pub const fn minimum_carrier_separation_hz(self) -> u32 {
        let measured_hz = self.measured_twenty_db_bandwidth.hz();
        if measured_hz > FCC_MINIMUM_CARRIER_SEPARATION_HZ {
            measured_hz
        } else {
            FCC_MINIMUM_CARRIER_SEPARATION_HZ
        }
    }

    pub const fn assess_airtime(self, airtime_us: u64) -> FrameDwell {
        let maximum_us = self.channel_occupancy_limit.micros();
        if airtime_us <= maximum_us {
            FrameDwell::WithinLimit {
                airtime_us,
                remaining_us: maximum_us - airtime_us,
            }
        } else {
            FrameDwell::ExceedsLimit {
                airtime_us,
                excess_us: airtime_us - maximum_us,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDwell {
    WithinLimit { airtime_us: u64, remaining_us: u64 },
    ExceedsLimit { airtime_us: u64, excess_us: u64 },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Us915HopSet<const N: usize> {
    channels: [Frequency; N],
    model: Us915HoppingModel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopSetError {
    InsufficientChannels {
        provided: usize,
        required: usize,
    },
    ChannelOutsideBand {
        index: usize,
        center_hz: u32,
        minimum_center_hz: u32,
        maximum_center_hz: u32,
    },
    ChannelsTooClose {
        first_index: usize,
        second_index: usize,
        separation_hz: u32,
        minimum_separation_hz: u32,
    },
    BandCannotFitChannels {
        channels: usize,
        available_center_span_hz: u32,
        minimum_separation_hz: u32,
    },
}

impl<const N: usize> Us915HopSet<N> {
    pub fn new(model: Us915HoppingModel, channels: [Frequency; N]) -> Result<Self, HopSetError> {
        if N < model.minimum_channel_count() {
            return Err(HopSetError::InsufficientChannels {
                provided: N,
                required: model.minimum_channel_count(),
            });
        }
        let (minimum_center_hz, maximum_center_hz) = allowed_center_band(model);
        for (index, channel) in channels.iter().enumerate() {
            if !(minimum_center_hz..=maximum_center_hz).contains(&channel.hz()) {
                return Err(HopSetError::ChannelOutsideBand {
                    index,
                    center_hz: channel.hz(),
                    minimum_center_hz,
                    maximum_center_hz,
                });
            }
        }
        let minimum_separation_hz = model.minimum_carrier_separation_hz();
        for first_index in 0..N {
            for second_index in first_index + 1..N {
                let separation_hz = channels[first_index]
                    .hz()
                    .abs_diff(channels[second_index].hz());
                if separation_hz < minimum_separation_hz {
                    return Err(HopSetError::ChannelsTooClose {
                        first_index,
                        second_index,
                        separation_hz,
                        minimum_separation_hz,
                    });
                }
            }
        }
        Ok(Self { channels, model })
    }

    pub fn evenly_spaced(model: Us915HoppingModel) -> Result<Self, HopSetError> {
        if N < model.minimum_channel_count() {
            return Err(HopSetError::InsufficientChannels {
                provided: N,
                required: model.minimum_channel_count(),
            });
        }
        let (minimum_center_hz, maximum_center_hz) = allowed_center_band(model);
        let available_center_span_hz = maximum_center_hz - minimum_center_hz;
        let intervals = N.saturating_sub(1);
        let minimum_separation_hz = model.minimum_carrier_separation_hz();
        if intervals == 0
            || u64::from(available_center_span_hz)
                < (intervals as u64).saturating_mul(u64::from(minimum_separation_hz))
        {
            return Err(HopSetError::BandCannotFitChannels {
                channels: N,
                available_center_span_hz,
                minimum_separation_hz,
            });
        }
        let channels = array::from_fn(|index| {
            let offset_hz = u64::from(available_center_span_hz) * index as u64 / intervals as u64;
            Frequency::new(minimum_center_hz + offset_hz as u32)
        });
        Self::new(model, channels)
    }

    pub const fn model(&self) -> Us915HoppingModel {
        self.model
    }

    pub const fn channels(&self) -> &[Frequency; N] {
        &self.channels
    }
}

impl<const N: usize> HoppingRegion<N> for Us915HopSet<N> {
    fn radio_region(&self) -> Region {
        Region::Us915
    }

    fn channels(&self) -> &[Frequency; N] {
        &self.channels
    }

    fn observation_window(&self) -> ObservationWindow {
        ObservationWindow::from_known_nonzero(self.model.observation_window_us())
    }

    fn channel_occupancy_limit(&self) -> ChannelOccupancyLimit {
        self.model.channel_occupancy_limit()
    }
}

fn allowed_center_band(model: Us915HoppingModel) -> (u32, u32) {
    let (minimum_hz, maximum_hz) = Region::Us915.band();
    let measured_hz = model.measured_twenty_db_bandwidth().hz();
    let lower_half_hz = measured_hz / 2;
    let upper_half_hz = measured_hz - lower_half_hz;
    (minimum_hz + lower_half_hz, maximum_hz - upper_half_hz)
}
