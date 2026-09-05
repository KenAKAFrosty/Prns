use super::super::MonotonicMicros;
use super::clock::{AcquiredReceivePhase, ClockError, ScheduleMicros};
use super::profile::TurboPhyProfile;
use super::schedule::{
    channel_index_at, slot_position_for_channel, TURBO_CHANNEL_COUNT, TURBO_CYCLE_US,
    TURBO_SLOT_US, TURBO_SUPERCYCLE_US,
};
use super::AcquisitionBeacon;

const MINIMUM_ACQUISITION_UNCERTAINTY_US: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquisitionObservation {
    received_at: MonotonicMicros,
    channel_index: usize,
    cycle: super::SupercycleCycle,
    completion_offset_us: u64,
    timing_uncertainty_us: u64,
}

impl AcquisitionObservation {
    pub const fn from_beacon(
        received_at: MonotonicMicros,
        channel_index: usize,
        beacon: AcquisitionBeacon,
        profile: TurboPhyProfile,
        timing_uncertainty_us: u64,
    ) -> Self {
        Self {
            received_at,
            channel_index,
            cycle: beacon.cycle(),
            completion_offset_us: beacon.completes_at_slot_offset_us(profile),
            timing_uncertainty_us,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionTrackerError {
    ChannelOutsideHopSet { channel_index: usize },
    CompletionOutsideSlot { completion_offset_us: u64 },
    EmptyTimingUncertainty,
    NonMonotonicObservation,
    Clock(ClockError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionOutcome {
    Provisional { observations: u8 },
    ReceivePhase { phase: AcquiredReceivePhase },
    ContradictionReset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionCorroboration {
    NoEstimate,
    Consistent,
    Contradiction,
    ClockWindowCrossesChannelBoundary,
}

pub struct AcquisitionTracker {
    estimate: Option<AcquiredReceivePhase>,
    observations: u8,
    observed_channels: [bool; TURBO_CHANNEL_COUNT],
    maximum_drift_ppm: u32,
}

impl AcquisitionTracker {
    pub const fn new(maximum_drift_ppm: u32) -> Result<Self, ClockError> {
        if maximum_drift_ppm == 0 {
            return Err(ClockError::EmptyDriftBound);
        }
        Ok(Self {
            estimate: None,
            observations: 0,
            observed_channels: [false; TURBO_CHANNEL_COUNT],
            maximum_drift_ppm,
        })
    }

    pub fn observe(
        &mut self,
        observation: AcquisitionObservation,
    ) -> Result<AcquisitionOutcome, AcquisitionTrackerError> {
        if observation.channel_index >= TURBO_CHANNEL_COUNT {
            return Err(AcquisitionTrackerError::ChannelOutsideHopSet {
                channel_index: observation.channel_index,
            });
        }
        if observation.completion_offset_us >= TURBO_SLOT_US {
            return Err(AcquisitionTrackerError::CompletionOutsideSlot {
                completion_offset_us: observation.completion_offset_us,
            });
        }
        if observation.timing_uncertainty_us == 0 {
            return Err(AcquisitionTrackerError::EmptyTimingUncertainty);
        }
        let position = slot_position_for_channel(observation.cycle, observation.channel_index)
            .map_err(|_| AcquisitionTrackerError::ChannelOutsideHopSet {
                channel_index: observation.channel_index,
            })?;
        let observed_phase_us = observation.cycle.index() as u64 * TURBO_CYCLE_US
            + position as u64 * TURBO_SLOT_US
            + observation.completion_offset_us;

        let Some(previous) = self.estimate else {
            self.start(observation, observed_phase_us)?;
            return Ok(AcquisitionOutcome::Provisional { observations: 1 });
        };
        if observation.received_at < previous.observed_at() {
            return Err(AcquisitionTrackerError::NonMonotonicObservation);
        }
        let predicted = previous
            .window_at(observation.received_at)
            .map_err(AcquisitionTrackerError::Clock)?;
        let aligned_schedule_us =
            align_phase_near(observed_phase_us, predicted.center_schedule_us());
        let error_us = aligned_schedule_us.abs_diff(predicted.center_schedule_us());
        let tolerated_us = predicted
            .uncertainty_us()
            .saturating_add(observation.timing_uncertainty_us);
        if error_us > tolerated_us {
            self.start(observation, observed_phase_us)?;
            return Ok(AcquisitionOutcome::ContradictionReset);
        }

        self.observations = self.observations.saturating_add(1);
        self.observed_channels[observation.channel_index] = true;
        let distinct_channels = self
            .observed_channels
            .iter()
            .filter(|observed| **observed)
            .count() as u8;
        let uncertainty_us = observation
            .timing_uncertainty_us
            .max(MINIMUM_ACQUISITION_UNCERTAINTY_US);
        let phase = AcquiredReceivePhase::new(
            observation.received_at,
            ScheduleMicros::new(aligned_schedule_us),
            uncertainty_us,
            self.maximum_drift_ppm,
            self.observations,
            distinct_channels,
        )
        .map_err(AcquisitionTrackerError::Clock)?;
        self.estimate = Some(phase);
        Ok(AcquisitionOutcome::ReceivePhase { phase })
    }

    pub fn corroborate_normal_traffic(
        &self,
        received_at: MonotonicMicros,
        channel_index: usize,
    ) -> Result<AcquisitionCorroboration, AcquisitionTrackerError> {
        if channel_index >= TURBO_CHANNEL_COUNT {
            return Err(AcquisitionTrackerError::ChannelOutsideHopSet { channel_index });
        }
        let Some(estimate) = self.estimate else {
            return Ok(AcquisitionCorroboration::NoEstimate);
        };
        let predicted = estimate
            .window_at(received_at)
            .map_err(AcquisitionTrackerError::Clock)?;
        let earliest_channel = channel_index_at(predicted.earliest_schedule_us());
        let latest_channel = channel_index_at(predicted.latest_schedule_us());
        if earliest_channel != latest_channel {
            return Ok(AcquisitionCorroboration::ClockWindowCrossesChannelBoundary);
        }
        if earliest_channel == channel_index {
            Ok(AcquisitionCorroboration::Consistent)
        } else {
            Ok(AcquisitionCorroboration::Contradiction)
        }
    }

    fn start(
        &mut self,
        observation: AcquisitionObservation,
        schedule_phase_us: u64,
    ) -> Result<(), AcquisitionTrackerError> {
        self.observations = 1;
        self.observed_channels = [false; TURBO_CHANNEL_COUNT];
        self.observed_channels[observation.channel_index] = true;
        self.estimate = Some(
            AcquiredReceivePhase::new(
                observation.received_at,
                ScheduleMicros::new(schedule_phase_us),
                observation
                    .timing_uncertainty_us
                    .max(MINIMUM_ACQUISITION_UNCERTAINTY_US),
                self.maximum_drift_ppm,
                1,
                1,
            )
            .map_err(AcquisitionTrackerError::Clock)?,
        );
        Ok(())
    }
}

fn align_phase_near(phase_us: u64, target_us: u64) -> u64 {
    let base = target_us / TURBO_SUPERCYCLE_US * TURBO_SUPERCYCLE_US;
    let candidate = base.saturating_add(phase_us);
    let lower = candidate.saturating_sub(TURBO_SUPERCYCLE_US);
    let upper = candidate.saturating_add(TURBO_SUPERCYCLE_US);
    [lower, candidate, upper]
        .into_iter()
        .min_by_key(|value| value.abs_diff(target_us))
        .unwrap_or(candidate)
}
