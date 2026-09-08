use prns_core::interfaces::subghz::regions::us915::turbo::{
    simulate_acquisition, simulate_contention, AcquisitionEnvironment, AcquisitionSimulation,
    AcquisitionSimulationError, ContentionSimulation, ContentionSimulationError,
    MaximumTransmitUncertainty, MaximumTransmitUncertaintyError, US915_TURBO_SPEC,
};

const EVIDENCE_SEED: u64 = 0x5052_4e53_5455_5242;

#[derive(Debug)]
enum EvidenceError {
    Acquisition(AcquisitionSimulationError),
    Contention(ContentionSimulationError),
    TransmitUncertainty(MaximumTransmitUncertaintyError),
}

impl core::fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Acquisition(error) => write!(formatter, "acquisition simulation: {error:?}"),
            Self::Contention(error) => write!(formatter, "contention simulation: {error:?}"),
            Self::TransmitUncertainty(error) => {
                write!(formatter, "maximum transmit uncertainty: {error:?}")
            }
        }
    }
}

impl std::error::Error for EvidenceError {}

impl From<AcquisitionSimulationError> for EvidenceError {
    fn from(error: AcquisitionSimulationError) -> Self {
        Self::Acquisition(error)
    }
}

impl From<ContentionSimulationError> for EvidenceError {
    fn from(error: ContentionSimulationError) -> Self {
        Self::Contention(error)
    }
}

impl From<MaximumTransmitUncertaintyError> for EvidenceError {
    fn from(error: MaximumTransmitUncertaintyError) -> Self {
        Self::TransmitUncertainty(error)
    }
}

fn acquisition_input(
    scanner_dwell_us: u64,
    environment: AcquisitionEnvironment,
) -> Result<AcquisitionSimulation, MaximumTransmitUncertaintyError> {
    Ok(AcquisitionSimulation {
        seed: EVIDENCE_SEED,
        trials: 10_000,
        scanner_dwell_us,
        beacon_opportunity_per_mille: 800,
        packet_loss_per_mille: 100,
        maximum_search_us: 1_800_000_000,
        maximum_clock_drift_ppm: 40,
        actual_clock_drift_ppm: 20,
        observation_timing_uncertainty_us: 500,
        timestamp_error_span_us: 250,
        maximum_transmit_uncertainty: MaximumTransmitUncertainty::new(5_000)?,
        environment,
    })
}

fn main() -> Result<(), EvidenceError> {
    println!("acquisition");
    println!("dwell_us,acquired,missed,p50_us,p95_us,p99_us,max_us,average_rx_us,average_retunes,maintenance_parts_per_million,contradiction_resets,off_primary_schedule_acquisitions,maximum_primary_phase_error_us");
    for scanner_dwell_us in [
        101_000,
        217_000,
        299_000,
        US915_TURBO_SPEC.scan_dwell_us(),
        377_000,
        400_000,
    ] {
        let result = simulate_acquisition(
            acquisition_input(scanner_dwell_us, AcquisitionEnvironment::SingleSchedule)?,
            US915_TURBO_SPEC.phy(),
        )?;
        println!(
            "{scanner_dwell_us},{},{},{},{},{},{},{},{},{},{},{},{}",
            result.acquired_trials,
            result.missed_trials,
            result.p50_acquisition_us,
            result.p95_acquisition_us,
            result.p99_acquisition_us,
            result.maximum_acquisition_us,
            result.average_scanner_rx_us,
            result.average_scanner_retunes,
            result.maintenance_airtime_parts_per_million,
            result.contradiction_resets,
            result.off_primary_schedule_acquisitions,
            result.maximum_primary_phase_error_us,
        );
    }

    println!("acquisition_environment");
    println!("environment,acquired,missed,contradiction_resets,off_primary_schedule_acquisitions,maximum_primary_phase_error_us");
    for (environment, name) in [
        (
            AcquisitionEnvironment::MixedSchedules {
                alternate_per_mille: 500,
                alternate_cycle_offset: 1,
            },
            "mixed_schedules_500_per_mille",
        ),
        (
            AcquisitionEnvironment::MixedCompletionTiming {
                displaced_per_mille: 500,
                completion_displacement_us: 2_000,
            },
            "mixed_completion_timing_500_per_mille",
        ),
    ] {
        let result = simulate_acquisition(
            acquisition_input(US915_TURBO_SPEC.scan_dwell_us(), environment)?,
            US915_TURBO_SPEC.phy(),
        )?;
        println!(
            "{name},{},{},{},{},{}",
            result.acquired_trials,
            result.missed_trials,
            result.contradiction_resets,
            result.off_primary_schedule_acquisitions,
            result.maximum_primary_phase_error_us,
        );
    }

    println!("contention");
    println!("nodes,delivered,collisions,occupied_airtime_us,p95_latency_us,fairness_millionths");
    for nodes in [2, 8, 32] {
        let result = simulate_contention(
            ContentionSimulation {
                seed: EVIDENCE_SEED,
                nodes,
                queued_packets_per_node: 16,
                logical_packet_bytes: 500,
                rounds: 1_000,
                packet_loss_per_mille: 100,
            },
            US915_TURBO_SPEC.phy(),
        )?;
        println!(
            "{nodes},{},{},{},{},{}",
            result.delivered_packets,
            result.collisions,
            result.occupied_airtime_us,
            result.p95_competing_node_latency_us,
            result.jain_fairness_millionths,
        );
    }
    Ok(())
}
