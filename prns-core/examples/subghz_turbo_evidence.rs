use prns_core::interfaces::subghz::turbo::{
    simulate_acquisition, simulate_contention, AcquisitionSimulation, AcquisitionSimulationError,
    ContentionSimulation, ContentionSimulationError, TURBO_SCAN_DWELL_US, US915_TURBO_PHY,
};

const EVIDENCE_SEED: u64 = 0x5052_4e53_5455_5242;

#[derive(Debug)]
enum EvidenceError {
    Acquisition(AcquisitionSimulationError),
    Contention(ContentionSimulationError),
}

impl core::fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Acquisition(error) => write!(formatter, "acquisition simulation: {error:?}"),
            Self::Contention(error) => write!(formatter, "contention simulation: {error:?}"),
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

fn main() -> Result<(), EvidenceError> {
    println!("acquisition");
    println!("dwell_us,acquired,missed,p50_us,p95_us,p99_us,max_us,average_rx_us,average_retunes,maintenance_parts_per_million");
    for scanner_dwell_us in [
        101_000,
        217_000,
        299_000,
        TURBO_SCAN_DWELL_US,
        377_000,
        400_000,
    ] {
        let result = simulate_acquisition(
            AcquisitionSimulation {
                seed: EVIDENCE_SEED,
                trials: 10_000,
                scanner_dwell_us,
                beacon_opportunity_per_mille: 800,
                packet_loss_per_mille: 100,
                maximum_search_us: 1_800_000_000,
            },
            US915_TURBO_PHY,
        )?;
        println!(
            "{scanner_dwell_us},{},{},{},{},{},{},{},{},{}",
            result.acquired_trials,
            result.missed_trials,
            result.p50_acquisition_us,
            result.p95_acquisition_us,
            result.p99_acquisition_us,
            result.maximum_acquisition_us,
            result.average_scanner_rx_us,
            result.average_scanner_retunes,
            result.maintenance_airtime_parts_per_million,
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
            US915_TURBO_PHY,
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
