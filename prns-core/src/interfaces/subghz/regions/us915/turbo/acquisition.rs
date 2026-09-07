use super::clock::{
    validate_drift_bound, AcquisitionCandidate, ClockError, ClockWindow,
    MaximumTransmitUncertainty, ScheduleMicros,
};
use super::profile::TurboPhyProfile;
use super::schedule::TurboChannelIndex;
use super::spec::{TURBO_CHANNEL_COUNT, US915_TURBO_SPEC};
use super::AcquisitionBeacon;
use crate::interfaces::subghz::MonotonicMicros;

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
    TimingUncertaintyExceeded { actual_us: u64, maximum_us: u64 },
    NonMonotonicObservation,
    Clock(ClockError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionTrackerConfigurationError {
    Clock(ClockError),
    TransmitUncertaintyBelowAcquisitionFloor { actual_us: u64, minimum_us: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedAcquiredSchedule {
    candidate: AcquisitionCandidate,
}

impl ValidatedAcquiredSchedule {
    pub const fn observed_at(self) -> MonotonicMicros {
        self.candidate.observed_at()
    }

    pub const fn observations(self) -> u8 {
        self.candidate.observations()
    }

    pub const fn distinct_channels(self) -> u8 {
        self.candidate.distinct_channels()
    }

    pub fn window_at(self, now: MonotonicMicros) -> Result<ClockWindow, ClockError> {
        self.candidate.window_at(now)
    }

    pub fn transmit_window_at(
        self,
        now: MonotonicMicros,
        maximum_uncertainty: MaximumTransmitUncertainty,
    ) -> Result<ClockWindow, ClockError> {
        self.window_at(now)?
            .require_transmit_uncertainty(maximum_uncertainty)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionOutcome {
    Candidate { candidate: AcquisitionCandidate },
    Operational { schedule: ValidatedAcquiredSchedule },
    ContradictionReset { candidate: AcquisitionCandidate },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionCorroboration {
    NoEstimate,
    Consistent,
    Contradiction,
    ClockWindowCrossesChannelBoundary,
}

pub struct AcquisitionTracker {
    estimate: Option<AcquisitionCandidate>,
    observations: u8,
    observed_channels: [bool; TURBO_CHANNEL_COUNT],
    maximum_drift_ppm: u32,
    maximum_transmit_uncertainty: MaximumTransmitUncertainty,
}

impl AcquisitionTracker {
    pub const fn new(
        maximum_drift_ppm: u32,
        maximum_transmit_uncertainty: MaximumTransmitUncertainty,
    ) -> Result<Self, AcquisitionTrackerConfigurationError> {
        if let Err(error) = validate_drift_bound(maximum_drift_ppm) {
            return Err(AcquisitionTrackerConfigurationError::Clock(error));
        }
        if maximum_transmit_uncertainty.micros() < MINIMUM_ACQUISITION_UNCERTAINTY_US {
            return Err(
                AcquisitionTrackerConfigurationError::TransmitUncertaintyBelowAcquisitionFloor {
                    actual_us: maximum_transmit_uncertainty.micros(),
                    minimum_us: MINIMUM_ACQUISITION_UNCERTAINTY_US,
                },
            );
        }
        Ok(Self {
            estimate: None,
            observations: 0,
            observed_channels: [false; TURBO_CHANNEL_COUNT],
            maximum_drift_ppm,
            maximum_transmit_uncertainty,
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
        if observation.completion_offset_us >= US915_TURBO_SPEC.slot_us() {
            return Err(AcquisitionTrackerError::CompletionOutsideSlot {
                completion_offset_us: observation.completion_offset_us,
            });
        }
        if observation.timing_uncertainty_us == 0 {
            return Err(AcquisitionTrackerError::EmptyTimingUncertainty);
        }
        let timing_uncertainty_us = observation
            .timing_uncertainty_us
            .max(MINIMUM_ACQUISITION_UNCERTAINTY_US);
        if timing_uncertainty_us > self.maximum_transmit_uncertainty.micros() {
            return Err(AcquisitionTrackerError::TimingUncertaintyExceeded {
                actual_us: timing_uncertainty_us,
                maximum_us: self.maximum_transmit_uncertainty.micros(),
            });
        }
        let channel_index = TurboChannelIndex::new(observation.channel_index).map_err(|_| {
            AcquisitionTrackerError::ChannelOutsideHopSet {
                channel_index: observation.channel_index,
            }
        })?;
        let position = US915_TURBO_SPEC
            .slot_position_for_channel(observation.cycle, channel_index)
            .index();
        let observed_phase_us = observation.cycle.index() as u64 * US915_TURBO_SPEC.cycle_us()
            + position as u64 * US915_TURBO_SPEC.slot_us()
            + observation.completion_offset_us;

        let Some(previous) = self.estimate else {
            let candidate = self.start(observation, observed_phase_us)?;
            return Ok(AcquisitionOutcome::Candidate { candidate });
        };
        if observation.received_at <= previous.observed_at() {
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
            .saturating_add(timing_uncertainty_us);
        if error_us > tolerated_us {
            let candidate = self.start(observation, observed_phase_us)?;
            return Ok(AcquisitionOutcome::ContradictionReset { candidate });
        }

        self.observations = self.observations.saturating_add(1);
        self.observed_channels[observation.channel_index] = true;
        let distinct_channels = self
            .observed_channels
            .iter()
            .filter(|observed| **observed)
            .count() as u8;
        let candidate = AcquisitionCandidate::new(
            observation.received_at,
            ScheduleMicros::new(aligned_schedule_us),
            timing_uncertainty_us,
            self.maximum_drift_ppm,
            self.observations,
            distinct_channels,
        )
        .map_err(AcquisitionTrackerError::Clock)?;
        self.estimate = Some(candidate);
        Ok(self.outcome_for(candidate))
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
        let earliest_channel = US915_TURBO_SPEC
            .slot_at(ScheduleMicros::new(predicted.earliest_schedule_us()))
            .channel_index();
        let latest_channel = US915_TURBO_SPEC
            .slot_at(ScheduleMicros::new(predicted.latest_schedule_us()))
            .channel_index();
        if earliest_channel != latest_channel {
            return Ok(AcquisitionCorroboration::ClockWindowCrossesChannelBoundary);
        }
        if earliest_channel.index() == channel_index {
            Ok(AcquisitionCorroboration::Consistent)
        } else {
            Ok(AcquisitionCorroboration::Contradiction)
        }
    }

    fn start(
        &mut self,
        observation: AcquisitionObservation,
        schedule_phase_us: u64,
    ) -> Result<AcquisitionCandidate, AcquisitionTrackerError> {
        self.observations = 1;
        self.observed_channels = [false; TURBO_CHANNEL_COUNT];
        self.observed_channels[observation.channel_index] = true;
        let candidate = AcquisitionCandidate::new(
            observation.received_at,
            ScheduleMicros::new(schedule_phase_us),
            observation
                .timing_uncertainty_us
                .max(MINIMUM_ACQUISITION_UNCERTAINTY_US),
            self.maximum_drift_ppm,
            1,
            1,
        )
        .map_err(AcquisitionTrackerError::Clock)?;
        self.estimate = Some(candidate);
        Ok(candidate)
    }

    fn outcome_for(&self, candidate: AcquisitionCandidate) -> AcquisitionOutcome {
        if candidate.observations() < US915_TURBO_SPEC.acquisition_observations()
            || candidate.distinct_channels() < US915_TURBO_SPEC.acquisition_distinct_channels()
        {
            return AcquisitionOutcome::Candidate { candidate };
        }
        AcquisitionOutcome::Operational {
            schedule: ValidatedAcquiredSchedule { candidate },
        }
    }
}

fn align_phase_near(phase_us: u64, target_us: u64) -> u64 {
    let supercycle_us = US915_TURBO_SPEC.supercycle_us();
    let base = target_us / supercycle_us * supercycle_us;
    let candidate = base.saturating_add(phase_us);
    let lower = candidate.saturating_sub(supercycle_us);
    let upper = candidate.saturating_add(supercycle_us);
    [lower, candidate, upper]
        .into_iter()
        .min_by_key(|value| value.abs_diff(target_us))
        .unwrap_or(candidate)
}
