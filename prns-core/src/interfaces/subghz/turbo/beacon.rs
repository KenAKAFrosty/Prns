use super::frame::{
    decode_frame, encode_acquisition, DecodedTurboFrame, EncodedTurboFrame, TurboFrameError,
};
use super::profile::TurboPhyProfile;
use super::schedule::{SupercycleCycle, TURBO_CYCLE_US};

pub const ACQUISITION_BEACON_BYTES: usize = 3;
pub const ACQUISITION_BEACON_CONTENTION_SLOTS: u8 = 16;
pub const ACQUISITION_BEACON_BASE_OFFSET_US: u64 = 20_000;
pub const ACQUISITION_BEACON_CONTENTION_SLOT_US: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquisitionBeacon {
    cycle: SupercycleCycle,
    contention_slot: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionBeaconError {
    Frame(TurboFrameError),
    NotAnAcquisitionFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionSlotActivity {
    Quiet,
    NormalTrafficObserved,
    ChannelBusy,
    AssessmentUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionBeaconSuppression {
    InitialQuietCycleIncomplete,
    AlreadyAttemptedThisCycle,
    DifferentCandidateSlot,
    NormalTrafficObserved,
    ChannelBusy,
    AssessmentUnavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AcquisitionBeaconPlan {
    Contend {
        beacon: AcquisitionBeacon,
    },
    Suppress {
        reason: AcquisitionBeaconSuppression,
    },
}

pub struct AcquisitionBeaconController {
    eligible_at_cycle: u64,
    last_attempted_cycle: u64,
}

impl AcquisitionBeaconController {
    pub const fn new(current_base_cycle: u64) -> Self {
        Self {
            eligible_at_cycle: current_base_cycle.saturating_add(1),
            last_attempted_cycle: u64::MAX,
        }
    }

    pub fn observe_normal_traffic(&mut self, current_base_cycle: u64) {
        self.eligible_at_cycle = current_base_cycle.saturating_add(2);
    }

    pub fn plan(
        &mut self,
        global_slot: u64,
        activity: AcquisitionSlotActivity,
        node_entropy: u64,
    ) -> AcquisitionBeaconPlan {
        let current_base_cycle = global_slot / super::TURBO_CHANNEL_COUNT as u64;
        let current_position = global_slot % super::TURBO_CHANNEL_COUNT as u64;
        let reason = match activity {
            AcquisitionSlotActivity::NormalTrafficObserved => {
                self.observe_normal_traffic(current_base_cycle);
                Some(AcquisitionBeaconSuppression::NormalTrafficObserved)
            }
            AcquisitionSlotActivity::ChannelBusy => Some(AcquisitionBeaconSuppression::ChannelBusy),
            AcquisitionSlotActivity::AssessmentUnavailable => {
                Some(AcquisitionBeaconSuppression::AssessmentUnavailable)
            }
            AcquisitionSlotActivity::Quiet => None,
        };
        if let Some(reason) = reason {
            return AcquisitionBeaconPlan::Suppress { reason };
        }
        if current_base_cycle < self.eligible_at_cycle {
            return AcquisitionBeaconPlan::Suppress {
                reason: AcquisitionBeaconSuppression::InitialQuietCycleIncomplete,
            };
        }
        if self.last_attempted_cycle == current_base_cycle {
            return AcquisitionBeaconPlan::Suppress {
                reason: AcquisitionBeaconSuppression::AlreadyAttemptedThisCycle,
            };
        }
        let mixed_entropy = node_entropy
            ^ current_base_cycle.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ node_entropy.rotate_left((current_base_cycle % 63) as u32 + 1);
        let candidate_position = mixed_entropy % super::TURBO_CHANNEL_COUNT as u64;
        if current_position != candidate_position {
            return AcquisitionBeaconPlan::Suppress {
                reason: AcquisitionBeaconSuppression::DifferentCandidateSlot,
            };
        }
        self.last_attempted_cycle = current_base_cycle;
        let cycle = SupercycleCycle::from_base_cycle(current_base_cycle);
        AcquisitionBeaconPlan::Contend {
            beacon: AcquisitionBeacon::from_entropy(cycle, mixed_entropy as u16),
        }
    }
}

impl AcquisitionBeacon {
    pub const fn new(
        cycle: SupercycleCycle,
        contention_slot: u8,
    ) -> Result<Self, AcquisitionBeaconError> {
        if contention_slot >= ACQUISITION_BEACON_CONTENTION_SLOTS {
            return Err(AcquisitionBeaconError::Frame(
                TurboFrameError::AcquisitionContentionSlotOutsideRange { contention_slot },
            ));
        }
        Ok(Self {
            cycle,
            contention_slot,
        })
    }

    pub const fn from_entropy(cycle: SupercycleCycle, entropy: u16) -> Self {
        let contention_slot =
            ((entropy as u32 * ACQUISITION_BEACON_CONTENTION_SLOTS as u32) >> 16) as u8;
        Self {
            cycle,
            contention_slot,
        }
    }

    pub const fn cycle(self) -> SupercycleCycle {
        self.cycle
    }

    pub const fn contention_slot(self) -> u8 {
        self.contention_slot
    }

    pub fn encode(self) -> Result<EncodedTurboFrame, AcquisitionBeaconError> {
        encode_acquisition(self.cycle, self.contention_slot).map_err(AcquisitionBeaconError::Frame)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, AcquisitionBeaconError> {
        match decode_frame(bytes).map_err(AcquisitionBeaconError::Frame)? {
            DecodedTurboFrame::Acquisition {
                cycle,
                contention_slot,
            } => Self::new(cycle, contention_slot),
            _ => Err(AcquisitionBeaconError::NotAnAcquisitionFrame),
        }
    }

    pub const fn starts_at_slot_offset_us(self) -> u64 {
        ACQUISITION_BEACON_BASE_OFFSET_US
            + self.contention_slot as u64 * ACQUISITION_BEACON_CONTENTION_SLOT_US
    }

    pub const fn completes_at_slot_offset_us(self, profile: TurboPhyProfile) -> u64 {
        self.starts_at_slot_offset_us()
            .saturating_add(profile.time_on_air_us(ACQUISITION_BEACON_BYTES))
    }

    pub const fn candidate_base_cycle(self, supercycle_number: u64) -> u64 {
        supercycle_number
            .saturating_mul(super::TURBO_CHANNEL_COUNT as u64)
            .saturating_add(self.cycle.index() as u64)
    }
}

pub const fn acquisition_beacon_listen_window_us(profile: TurboPhyProfile) -> u64 {
    ACQUISITION_BEACON_CONTENTION_SLOTS as u64 * ACQUISITION_BEACON_CONTENTION_SLOT_US
        + profile.time_on_air_us(ACQUISITION_BEACON_BYTES)
}

pub const fn base_cycle_at(schedule_us: u64) -> u64 {
    schedule_us / TURBO_CYCLE_US
}
