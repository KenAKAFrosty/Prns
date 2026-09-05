use super::profile::{TurboPhyProfile, US915_TURBO_PHY};
use crate::interfaces::subghz::regions::us915::frequency_hopping::{
    ChannelOccupancyLimit, MeasuredTwentyDbBandwidth, Us915HoppingModel,
};
use crate::interfaces::subghz::{Frequency, RegulatoryRegion};

pub const TURBO_CHANNEL_COUNT: usize = 51;

const TURBO_SLOT_US: u64 = 400_000;
const TURBO_CHANNEL_OCCUPANCY_BUDGET_US: u64 = 390_000;
const TURBO_MINIMUM_MEASURED_BANDWIDTH_HZ: u32 = 250_000;
const TURBO_MAXIMUM_MEASURED_BANDWIDTH_HZ: u32 = 500_000;
const TURBO_BOOT_QUARANTINE_US: u64 = 10_000_000;
const TURBO_SCAN_STRIDE: usize = 7;
const TURBO_SCAN_DWELL_US: u64 = 341_000;

const TURBO_CHANNELS: [Frequency; TURBO_CHANNEL_COUNT] = channels();

const TURBO_CHANNEL_ORDER: [u8; TURBO_CHANNEL_COUNT] = [
    23, 35, 4, 16, 32, 45, 7, 19, 43, 31, 18, 0, 48, 28, 2, 15, 30, 42, 9, 21, 49, 34, 3, 22, 37,
    8, 20, 39, 1, 27, 14, 41, 29, 17, 47, 5, 33, 46, 13, 25, 44, 12, 24, 40, 11, 26, 38, 10, 50,
    36, 6,
];

pub static US915_TURBO_SPEC: Us915TurboSpec = Us915TurboSpec::new(Us915TurboSpecParameters {
    channels: TURBO_CHANNELS,
    channel_order: TURBO_CHANNEL_ORDER,
    phy: US915_TURBO_PHY,
    slot_us: TURBO_SLOT_US,
    channel_occupancy_budget_us: TURBO_CHANNEL_OCCUPANCY_BUDGET_US,
    minimum_measured_bandwidth_hz: TURBO_MINIMUM_MEASURED_BANDWIDTH_HZ,
    maximum_measured_bandwidth_hz: TURBO_MAXIMUM_MEASURED_BANDWIDTH_HZ,
    boot_quarantine_us: TURBO_BOOT_QUARANTINE_US,
    scan_stride: TURBO_SCAN_STRIDE,
    scan_dwell_us: TURBO_SCAN_DWELL_US,
});

struct Us915TurboSpecParameters {
    channels: [Frequency; TURBO_CHANNEL_COUNT],
    channel_order: [u8; TURBO_CHANNEL_COUNT],
    phy: TurboPhyProfile,
    slot_us: u64,
    channel_occupancy_budget_us: u64,
    minimum_measured_bandwidth_hz: u32,
    maximum_measured_bandwidth_hz: u32,
    boot_quarantine_us: u64,
    scan_stride: usize,
    scan_dwell_us: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Us915TurboSpec {
    channels: [Frequency; TURBO_CHANNEL_COUNT],
    channel_order: [u8; TURBO_CHANNEL_COUNT],
    channel_order_position: [u8; TURBO_CHANNEL_COUNT],
    phy: TurboPhyProfile,
    slot_us: u64,
    occupancy_window_us: u64,
    channel_occupancy_budget_us: u64,
    minimum_measured_bandwidth_hz: u32,
    maximum_measured_bandwidth_hz: u32,
    boot_quarantine_us: u64,
    scan_stride: usize,
    scan_dwell_us: u64,
}

impl Us915TurboSpec {
    const fn new(parameters: Us915TurboSpecParameters) -> Self {
        assert!(parameters.slot_us != 0);
        assert!(parameters.channel_occupancy_budget_us != 0);
        assert!(parameters.channel_occupancy_budget_us < parameters.slot_us);
        assert!(
            parameters.minimum_measured_bandwidth_hz <= parameters.maximum_measured_bandwidth_hz
        );
        assert!(parameters.boot_quarantine_us != 0);
        assert!(parameters.scan_stride != 0);
        assert!(greatest_common_divisor(parameters.scan_stride, TURBO_CHANNEL_COUNT) == 1);
        assert!(parameters.scan_dwell_us != 0);
        assert!(!parameters.scan_dwell_us.is_multiple_of(parameters.slot_us));
        assert!(!parameters.slot_us.is_multiple_of(parameters.scan_dwell_us));
        assert!(matches!(parameters.phy.validate(), Ok(())));
        let occupancy_limit = ChannelOccupancyLimit::from_known_within_regulatory_maximum(
            parameters.channel_occupancy_budget_us,
        );
        let minimum_bandwidth =
            MeasuredTwentyDbBandwidth::from_known_nonzero(parameters.minimum_measured_bandwidth_hz);
        let maximum_bandwidth =
            MeasuredTwentyDbBandwidth::from_known_nonzero(parameters.maximum_measured_bandwidth_hz);
        let minimum_model = Us915HoppingModel::from_known_valid(minimum_bandwidth, occupancy_limit);
        let maximum_model = Us915HoppingModel::from_known_valid(maximum_bandwidth, occupancy_limit);
        assert!(minimum_model.minimum_channel_count() <= TURBO_CHANNEL_COUNT);
        assert!(maximum_model.minimum_channel_count() <= TURBO_CHANNEL_COUNT);
        assert!(minimum_model.observation_window_us() == maximum_model.observation_window_us());
        validate_channels(
            parameters.channels,
            parameters.maximum_measured_bandwidth_hz,
        );
        let channel_order_position = channel_order_positions(parameters.channel_order);
        Self {
            channels: parameters.channels,
            channel_order: parameters.channel_order,
            channel_order_position,
            phy: parameters.phy,
            slot_us: parameters.slot_us,
            occupancy_window_us: minimum_model.observation_window_us(),
            channel_occupancy_budget_us: parameters.channel_occupancy_budget_us,
            minimum_measured_bandwidth_hz: parameters.minimum_measured_bandwidth_hz,
            maximum_measured_bandwidth_hz: parameters.maximum_measured_bandwidth_hz,
            boot_quarantine_us: parameters.boot_quarantine_us,
            scan_stride: parameters.scan_stride,
            scan_dwell_us: parameters.scan_dwell_us,
        }
    }

    pub const fn channels(&self) -> &[Frequency; TURBO_CHANNEL_COUNT] {
        &self.channels
    }

    pub const fn phy(&self) -> TurboPhyProfile {
        self.phy
    }

    pub const fn slot_us(&self) -> u64 {
        self.slot_us
    }

    pub const fn cycle_us(&self) -> u64 {
        self.slot_us * TURBO_CHANNEL_COUNT as u64
    }

    pub const fn supercycle_slots(&self) -> u64 {
        TURBO_CHANNEL_COUNT as u64 * TURBO_CHANNEL_COUNT as u64
    }

    pub const fn supercycle_us(&self) -> u64 {
        self.slot_us * self.supercycle_slots()
    }

    pub const fn occupancy_window_us(&self) -> u64 {
        self.occupancy_window_us
    }

    pub const fn channel_occupancy_budget_us(&self) -> u64 {
        self.channel_occupancy_budget_us
    }

    pub const fn minimum_measured_bandwidth_hz(&self) -> u32 {
        self.minimum_measured_bandwidth_hz
    }

    pub const fn maximum_measured_bandwidth_hz(&self) -> u32 {
        self.maximum_measured_bandwidth_hz
    }

    pub const fn boot_quarantine_us(&self) -> u64 {
        self.boot_quarantine_us
    }

    pub const fn scan_stride(&self) -> usize {
        self.scan_stride
    }

    pub const fn scan_dwell_us(&self) -> u64 {
        self.scan_dwell_us
    }

    pub(crate) const fn channel_order_at(&self, index: usize) -> u8 {
        self.channel_order[index]
    }

    pub(crate) const fn channel_order_position(&self, channel_index: usize) -> u8 {
        self.channel_order_position[channel_index]
    }
}

const fn channels() -> [Frequency; TURBO_CHANNEL_COUNT] {
    let mut frequencies = [Frequency::new(902_500_000); TURBO_CHANNEL_COUNT];
    let mut index = 0;
    while index < TURBO_CHANNEL_COUNT {
        frequencies[index] = Frequency::new(902_500_000 + index as u32 * 500_000);
        index += 1;
    }
    frequencies
}

const fn validate_channels(
    channels: [Frequency; TURBO_CHANNEL_COUNT],
    maximum_measured_bandwidth_hz: u32,
) {
    let frequency_range = RegulatoryRegion::Us915.frequency_range();
    let mut index = 0;
    while index < TURBO_CHANNEL_COUNT {
        assert!(frequency_range
            .contains_nominal_channel(channels[index], maximum_measured_bandwidth_hz));
        if index != 0 {
            assert!(channels[index - 1].hz() < channels[index].hz());
            assert!(
                channels[index].hz().abs_diff(channels[index - 1].hz())
                    >= maximum_measured_bandwidth_hz
            );
        }
        index += 1;
    }
}

const fn channel_order_positions(
    channel_order: [u8; TURBO_CHANNEL_COUNT],
) -> [u8; TURBO_CHANNEL_COUNT] {
    let mut seen = [false; TURBO_CHANNEL_COUNT];
    let mut positions = [0; TURBO_CHANNEL_COUNT];
    let mut index = 0;
    while index < TURBO_CHANNEL_COUNT {
        let channel_index = channel_order[index] as usize;
        assert!(channel_index < TURBO_CHANNEL_COUNT);
        assert!(!seen[channel_index]);
        seen[channel_index] = true;
        positions[channel_index] = index as u8;
        index += 1;
    }
    positions
}

const fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}
