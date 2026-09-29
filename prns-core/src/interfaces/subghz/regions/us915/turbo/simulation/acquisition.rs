use super::super::{
    acquisition_beacon_listen_window_us, AcquisitionBeacon, AcquisitionCandidate,
    AcquisitionObservation, AcquisitionOutcome, AcquisitionTracker,
    AcquisitionTrackerConfigurationError, AcquisitionTrackerError, MaximumTransmitUncertainty,
    TurboGlobalSlot, TurboPhyProfile, TurboProfileError, TurboRadioDwell,
    ACQUISITION_BEACON_BASE_OFFSET_US, ACQUISITION_BEACON_BYTES,
    ACQUISITION_BEACON_CONTENTION_SLOTS, ACQUISITION_BEACON_CONTENTION_SLOT_US,
    TURBO_CHANNEL_COUNT, US915_TURBO_SPEC,
};
use super::{percentile, random_event, DeterministicRng};
use crate::interfaces::subghz::MonotonicMicros;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquisitionSimulation {
    pub seed: u64,
    pub trials: u32,
    pub scanner_dwell_us: u64,
    pub beacon_opportunity_per_mille: u16,
    pub packet_loss_per_mille: u16,
    pub maximum_search_us: u64,
    pub maximum_clock_drift_ppm: u32,
    pub actual_clock_drift_ppm: i32,
    pub observation_timing_uncertainty_us: u64,
    pub timestamp_error_span_us: u64,
    pub maximum_transmit_uncertainty: MaximumTransmitUncertainty,
    pub environment: AcquisitionEnvironment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionEnvironment {
    SingleSchedule,
    MixedSchedules {
        alternate_per_mille: u16,
        alternate_cycle_offset: u8,
    },
    MixedCompletionTiming {
        displaced_per_mille: u16,
        completion_displacement_us: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionSimulationError {
    EmptyTrials,
    EmptyScannerDwell,
    EmptySearchPeriod,
    EmptyObservationTimingUncertainty,
    ProbabilityOutsideRange { per_mille: u16 },
    ActualClockDriftExceedsBound { actual_ppm: i32, maximum_ppm: u32 },
    TimestampErrorExceedsDeclaredUncertainty { error_us: u64, uncertainty_us: u64 },
    ObservationTimingUncertaintyExceedsTransmitMaximum { actual_us: u64, maximum_us: u64 },
    AlternateCycleOffsetOutsideRange { cycle_offset: u8 },
    EmptyCompletionDisplacement,
    CompletionDisplacementOutsideSlot { completion_displacement_us: u64 },
    ScheduleRangeExceeded { global_slot: u64 },
    TrackerConfiguration(AcquisitionTrackerConfigurationError),
    Tracker(AcquisitionTrackerError),
    Profile(TurboProfileError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquisitionSimulationResult {
    pub acquired_trials: u32,
    pub missed_trials: u32,
    pub p50_acquisition_us: u64,
    pub p95_acquisition_us: u64,
    pub p99_acquisition_us: u64,
    pub maximum_acquisition_us: u64,
    pub average_scanner_rx_us: u64,
    pub average_scanner_retunes: u64,
    pub beacons_transmitted: u64,
    pub maintenance_airtime_parts_per_million: u32,
    pub contradiction_resets: u64,
    pub off_primary_schedule_acquisitions: u32,
    pub maximum_primary_phase_error_us: u64,
}

pub fn simulate_acquisition(
    input: AcquisitionSimulation,
    profile: TurboPhyProfile,
) -> Result<AcquisitionSimulationResult, AcquisitionSimulationError> {
    if input.trials == 0 {
        return Err(AcquisitionSimulationError::EmptyTrials);
    }
    if input.scanner_dwell_us == 0 {
        return Err(AcquisitionSimulationError::EmptyScannerDwell);
    }
    if input.maximum_search_us == 0 {
        return Err(AcquisitionSimulationError::EmptySearchPeriod);
    }
    if input.observation_timing_uncertainty_us == 0 {
        return Err(AcquisitionSimulationError::EmptyObservationTimingUncertainty);
    }
    profile
        .validate()
        .map_err(AcquisitionSimulationError::Profile)?;
    let environmental_event_per_mille = validate_environment(input.environment, profile)?;
    for per_mille in [
        input.beacon_opportunity_per_mille,
        input.packet_loss_per_mille,
        environmental_event_per_mille,
    ] {
        if per_mille > 1_000 {
            return Err(AcquisitionSimulationError::ProbabilityOutsideRange { per_mille });
        }
    }
    if input.actual_clock_drift_ppm.unsigned_abs() > input.maximum_clock_drift_ppm {
        return Err(AcquisitionSimulationError::ActualClockDriftExceedsBound {
            actual_ppm: input.actual_clock_drift_ppm,
            maximum_ppm: input.maximum_clock_drift_ppm,
        });
    }
    if input.timestamp_error_span_us > input.observation_timing_uncertainty_us {
        return Err(
            AcquisitionSimulationError::TimestampErrorExceedsDeclaredUncertainty {
                error_us: input.timestamp_error_span_us,
                uncertainty_us: input.observation_timing_uncertainty_us,
            },
        );
    }
    if input.observation_timing_uncertainty_us > input.maximum_transmit_uncertainty.micros() {
        return Err(
            AcquisitionSimulationError::ObservationTimingUncertaintyExceedsTransmitMaximum {
                actual_us: input.observation_timing_uncertainty_us,
                maximum_us: input.maximum_transmit_uncertainty.micros(),
            },
        );
    }
    AcquisitionTracker::new(
        input.maximum_clock_drift_ppm,
        input.maximum_transmit_uncertainty,
    )
    .map_err(AcquisitionSimulationError::TrackerConfiguration)?;
    let mut rng = DeterministicRng::new(input.seed);
    let mut latencies = std::vec::Vec::with_capacity(input.trials as usize);
    let mut total_rx_us = 0u64;
    let mut total_retunes = 0u64;
    let mut considered_beacon_slots = 0u64;
    let mut beacons_transmitted = 0u64;
    let mut contradiction_resets = 0u64;
    let mut off_primary_schedule_acquisitions = 0u32;
    let mut maximum_primary_phase_error_us = 0u64;
    let beacon_airtime_us = profile.time_on_air_us(ACQUISITION_BEACON_BYTES);
    let beacon_listen_window_us = acquisition_beacon_listen_window_us(profile);
    for _ in 0..input.trials {
        let scan_origin = rng.next_u64() % input.scanner_dwell_us;
        let scan_channel = rng.next_u64() as usize % TURBO_CHANNEL_COUNT;
        let mut tracker = AcquisitionTracker::new(
            input.maximum_clock_drift_ppm,
            input.maximum_transmit_uncertainty,
        )
        .map_err(AcquisitionSimulationError::TrackerConfiguration)?;
        let mut candidate = None;
        let mut followed_through_slot = None;
        let mut beacon_cycle = 0u64;
        let mut trial_rx_us = 0u64;
        let mut trial_retunes = 0u64;
        let acquired_at = loop {
            let candidate_position = rng.next_u64() % TURBO_CHANNEL_COUNT as u64;
            let wall_global_slot = beacon_cycle
                .saturating_mul(TURBO_CHANNEL_COUNT as u64)
                .saturating_add(candidate_position);
            let Ok(wall_global_slot) = TurboGlobalSlot::new(wall_global_slot) else {
                break None;
            };
            let environmental_event = random_event(&mut rng, environmental_event_per_mille);
            let source_global_slot =
                source_global_slot(wall_global_slot, input.environment, environmental_event)?;
            let source_slot = US915_TURBO_SPEC.slot_for_global_slot(source_global_slot);
            let beacon = AcquisitionBeacon::from_entropy(source_slot.cycle(), rng.next_u16());
            let wall_completion_us = wall_global_slot
                .index()
                .saturating_mul(US915_TURBO_SPEC.slot_us())
                .saturating_add(beacon.completes_at_slot_offset_us(profile))
                .saturating_add(completion_displacement(
                    input.environment,
                    environmental_event,
                ));
            if wall_completion_us > input.maximum_search_us {
                break None;
            }
            considered_beacon_slots = considered_beacon_slots.saturating_add(1);
            let beacon_available = random_event(&mut rng, input.beacon_opportunity_per_mille);
            if beacon_available {
                beacons_transmitted = beacons_transmitted.saturating_add(1);
            }
            let schedule_channel = source_slot.channel_index().index();
            let wall_beacon_start_us = wall_completion_us.saturating_sub(beacon_airtime_us);
            let local_beacon_start_us =
                receiver_time(wall_beacon_start_us, input.actual_clock_drift_ppm);
            let local_completion_us =
                receiver_time(wall_completion_us, input.actual_clock_drift_ppm);
            let received_without_retune = if let Some(estimate) = candidate {
                let previous_slot = followed_through_slot.unwrap_or(wall_global_slot.index());
                let followed_slots = wall_global_slot.index().saturating_sub(previous_slot);
                trial_retunes = trial_retunes.saturating_add(followed_slots);
                trial_rx_us = trial_rx_us
                    .saturating_add(followed_slots.saturating_mul(beacon_listen_window_us));
                followed_through_slot = Some(wall_global_slot.index());
                candidate_receives_beacon(
                    estimate,
                    local_beacon_start_us,
                    local_completion_us,
                    schedule_channel,
                    profile,
                )?
            } else {
                let scan_step_at_start =
                    local_beacon_start_us.saturating_add(scan_origin) / input.scanner_dwell_us;
                let scan_step_at_end = local_completion_us
                    .saturating_sub(1)
                    .saturating_add(scan_origin)
                    / input.scanner_dwell_us;
                trial_retunes = scan_step_at_end;
                let listening_channel = (scan_channel
                    + scan_step_at_start as usize * US915_TURBO_SPEC.scan_stride())
                    % TURBO_CHANNEL_COUNT;
                scan_step_at_start == scan_step_at_end && listening_channel == schedule_channel
            };
            let lost = random_event(&mut rng, input.packet_loss_per_mille);
            if beacon_available && received_without_retune && !lost {
                if candidate.is_none() {
                    trial_rx_us = wall_completion_us;
                    followed_through_slot = Some(wall_global_slot.index());
                }
                let timestamp_error_us =
                    random_signed_span(&mut rng, input.timestamp_error_span_us);
                let received_at = MonotonicMicros::new(apply_signed_offset(
                    local_completion_us,
                    timestamp_error_us,
                ));
                let observation = AcquisitionObservation::from_beacon(
                    received_at,
                    schedule_channel,
                    beacon,
                    profile,
                    input.observation_timing_uncertainty_us,
                );
                match tracker
                    .observe(observation)
                    .map_err(AcquisitionSimulationError::Tracker)?
                {
                    AcquisitionOutcome::Candidate {
                        candidate: next_candidate,
                    } => candidate = Some(next_candidate),
                    AcquisitionOutcome::ContradictionReset {
                        candidate: next_candidate,
                    } => {
                        contradiction_resets = contradiction_resets.saturating_add(1);
                        candidate = Some(next_candidate);
                    }
                    AcquisitionOutcome::Operational { schedule } => {
                        let window = schedule
                            .window_at(received_at)
                            .map_err(AcquisitionTrackerError::Clock)
                            .map_err(AcquisitionSimulationError::Tracker)?;
                        let phase_error_us =
                            circular_phase_error(window.center_schedule_us(), wall_completion_us);
                        maximum_primary_phase_error_us =
                            maximum_primary_phase_error_us.max(phase_error_us);
                        if phase_error_us > window.uncertainty_us() {
                            off_primary_schedule_acquisitions =
                                off_primary_schedule_acquisitions.saturating_add(1);
                        }
                        break Some(wall_completion_us);
                    }
                }
            }
            beacon_cycle = beacon_cycle.saturating_add(1);
        };
        if candidate.is_none() {
            trial_rx_us = input.maximum_search_us;
            trial_retunes = input
                .maximum_search_us
                .saturating_sub(1)
                .saturating_add(scan_origin)
                / input.scanner_dwell_us;
        } else if acquired_at.is_none() {
            let last_followed = followed_through_slot.unwrap_or(0);
            let final_slot = input.maximum_search_us.saturating_sub(1) / US915_TURBO_SPEC.slot_us();
            let remaining_slots = final_slot.saturating_sub(last_followed);
            trial_retunes = trial_retunes.saturating_add(remaining_slots);
            trial_rx_us =
                trial_rx_us.saturating_add(remaining_slots.saturating_mul(beacon_listen_window_us));
        }
        total_rx_us = total_rx_us.saturating_add(trial_rx_us);
        total_retunes = total_retunes.saturating_add(trial_retunes);
        if let Some(acquired_at) = acquired_at {
            latencies.push(acquired_at);
        }
    }
    latencies.sort_unstable();
    let acquired_trials = latencies.len() as u32;
    let missed_trials = input.trials.saturating_sub(acquired_trials);
    let maintenance_airtime_parts_per_million = beacons_transmitted
        .saturating_mul(beacon_airtime_us)
        .saturating_mul(1_000_000)
        .div_ceil(
            considered_beacon_slots
                .saturating_mul(US915_TURBO_SPEC.cycle_us())
                .max(1),
        )
        .min(1_000_000) as u32;
    Ok(AcquisitionSimulationResult {
        acquired_trials,
        missed_trials,
        p50_acquisition_us: percentile(&latencies, 50),
        p95_acquisition_us: percentile(&latencies, 95),
        p99_acquisition_us: percentile(&latencies, 99),
        maximum_acquisition_us: latencies.last().copied().unwrap_or(0),
        average_scanner_rx_us: total_rx_us / u64::from(input.trials),
        average_scanner_retunes: total_retunes / u64::from(input.trials),
        beacons_transmitted,
        maintenance_airtime_parts_per_million,
        contradiction_resets,
        off_primary_schedule_acquisitions,
        maximum_primary_phase_error_us,
    })
}

fn validate_environment(
    environment: AcquisitionEnvironment,
    profile: TurboPhyProfile,
) -> Result<u16, AcquisitionSimulationError> {
    match environment {
        AcquisitionEnvironment::SingleSchedule => Ok(0),
        AcquisitionEnvironment::MixedSchedules {
            alternate_per_mille,
            alternate_cycle_offset,
        } => {
            if alternate_cycle_offset == 0 || alternate_cycle_offset >= TURBO_CHANNEL_COUNT as u8 {
                return Err(
                    AcquisitionSimulationError::AlternateCycleOffsetOutsideRange {
                        cycle_offset: alternate_cycle_offset,
                    },
                );
            }
            Ok(alternate_per_mille)
        }
        AcquisitionEnvironment::MixedCompletionTiming {
            displaced_per_mille,
            completion_displacement_us,
        } => {
            if completion_displacement_us == 0 {
                return Err(AcquisitionSimulationError::EmptyCompletionDisplacement);
            }
            let latest_nominal_completion_us = ACQUISITION_BEACON_BASE_OFFSET_US
                .saturating_add(
                    u64::from(ACQUISITION_BEACON_CONTENTION_SLOTS - 1)
                        .saturating_mul(ACQUISITION_BEACON_CONTENTION_SLOT_US),
                )
                .saturating_add(profile.time_on_air_us(ACQUISITION_BEACON_BYTES));
            if latest_nominal_completion_us.saturating_add(completion_displacement_us)
                >= US915_TURBO_SPEC.slot_us()
            {
                return Err(
                    AcquisitionSimulationError::CompletionDisplacementOutsideSlot {
                        completion_displacement_us,
                    },
                );
            }
            Ok(displaced_per_mille)
        }
    }
}

fn source_global_slot(
    wall_global_slot: TurboGlobalSlot,
    environment: AcquisitionEnvironment,
    environmental_event: bool,
) -> Result<TurboGlobalSlot, AcquisitionSimulationError> {
    let cycle_offset = match (environment, environmental_event) {
        (
            AcquisitionEnvironment::MixedSchedules {
                alternate_cycle_offset,
                ..
            },
            true,
        ) => u64::from(alternate_cycle_offset),
        (
            AcquisitionEnvironment::SingleSchedule
            | AcquisitionEnvironment::MixedSchedules { .. }
            | AcquisitionEnvironment::MixedCompletionTiming { .. },
            false,
        )
        | (
            AcquisitionEnvironment::SingleSchedule
            | AcquisitionEnvironment::MixedCompletionTiming { .. },
            true,
        ) => 0,
    };
    let global_slot = wall_global_slot
        .index()
        .checked_add(cycle_offset.saturating_mul(TURBO_CHANNEL_COUNT as u64))
        .ok_or(AcquisitionSimulationError::ScheduleRangeExceeded {
            global_slot: u64::MAX,
        })?;
    TurboGlobalSlot::new(global_slot)
        .map_err(|_| AcquisitionSimulationError::ScheduleRangeExceeded { global_slot })
}

fn completion_displacement(environment: AcquisitionEnvironment, environmental_event: bool) -> u64 {
    match (environment, environmental_event) {
        (
            AcquisitionEnvironment::MixedCompletionTiming {
                completion_displacement_us,
                ..
            },
            true,
        ) => completion_displacement_us,
        _ => 0,
    }
}

fn candidate_receives_beacon(
    candidate: AcquisitionCandidate,
    local_beacon_start_us: u64,
    local_completion_us: u64,
    source_channel: usize,
    profile: TurboPhyProfile,
) -> Result<bool, AcquisitionSimulationError> {
    let Ok(dwell) = TurboRadioDwell::new(
        MonotonicMicros::new(local_beacon_start_us),
        MonotonicMicros::new(local_completion_us),
    ) else {
        return Ok(false);
    };
    let Ok(lease) = candidate.authorize_receive(dwell) else {
        return Ok(false);
    };
    if lease.channel_index().index() != source_channel {
        return Ok(false);
    }
    let beginning = candidate
        .window_at(dwell.begins_at())
        .map_err(AcquisitionTrackerError::Clock)
        .map_err(AcquisitionSimulationError::Tracker)?;
    let before_end = candidate
        .window_at(MonotonicMicros::new(dwell.ends_before().micros() - 1))
        .map_err(AcquisitionTrackerError::Clock)
        .map_err(AcquisitionSimulationError::Tracker)?;
    let beginning_offset = beginning.center_schedule_us() % US915_TURBO_SPEC.slot_us();
    let ending_offset = before_end.center_schedule_us() % US915_TURBO_SPEC.slot_us();
    let listen_end_us = ACQUISITION_BEACON_BASE_OFFSET_US
        .saturating_add(acquisition_beacon_listen_window_us(profile));
    Ok(beginning_offset >= ACQUISITION_BEACON_BASE_OFFSET_US && ending_offset < listen_end_us)
}

fn receiver_time(schedule_us: u64, drift_ppm: i32) -> u64 {
    let adjustment_us = (u128::from(schedule_us) * u128::from(drift_ppm.unsigned_abs()) / 1_000_000)
        .min(u128::from(u64::MAX)) as u64;
    if drift_ppm >= 0 {
        schedule_us.saturating_add(adjustment_us)
    } else {
        schedule_us.saturating_sub(adjustment_us)
    }
}

fn random_signed_span(rng: &mut DeterministicRng, span: u64) -> i128 {
    if span == 0 {
        return 0;
    }
    let magnitude = if span == u64::MAX {
        rng.next_u64()
    } else {
        rng.next_u64() % (span + 1)
    };
    if rng.next_u64() & 1 == 0 {
        -i128::from(magnitude)
    } else {
        i128::from(magnitude)
    }
}

fn apply_signed_offset(value: u64, offset: i128) -> u64 {
    if offset >= 0 {
        value.saturating_add(offset as u64)
    } else {
        value.saturating_sub(offset.unsigned_abs().min(u128::from(u64::MAX)) as u64)
    }
}

fn circular_phase_error(observed_schedule_us: u64, true_schedule_us: u64) -> u64 {
    let supercycle_us = US915_TURBO_SPEC.supercycle_us();
    let observed_phase = observed_schedule_us % supercycle_us;
    let true_phase = true_schedule_us % supercycle_us;
    let direct = observed_phase.abs_diff(true_phase);
    direct.min(supercycle_us.saturating_sub(direct))
}
