mod acquisition;

pub use acquisition::{
    simulate_acquisition, AcquisitionEnvironment, AcquisitionSimulation,
    AcquisitionSimulationError, AcquisitionSimulationResult,
};

use super::{
    CapabilitySupport, ChannelAccess, ChannelAccessAction, ChannelAccessEvent, ContentionClass,
    ContentionPolicy, DatagramId, MaximumTransmitUncertainty, ScheduleMicros,
    TransmissionTimingBudget, TrustedScheduleClock, TrustedTimeSource, TurboHardwareSupport,
    TurboPhyProfile, TurboProfileError, Us915TurboConfiguration, Us915TurboTransmitter,
    UtcTimescale, TURBO_LOGICAL_PACKET_MAX, US915_TURBO_SPEC,
};
use crate::interfaces::subghz::regions::us915::frequency_hopping::{
    AntennaGainDeciDb, ConductedPowerDbm, MeasuredTwentyDbBandwidth, Us915PowerInputs,
};
use crate::interfaces::subghz::MonotonicMicros;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentionSimulation {
    pub seed: u64,
    pub nodes: u8,
    pub queued_packets_per_node: u8,
    pub logical_packet_bytes: usize,
    pub rounds: u32,
    pub packet_loss_per_mille: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentionSimulationResult {
    pub delivered_packets: u64,
    pub collisions: u64,
    pub occupied_airtime_us: u64,
    pub p95_competing_node_latency_us: u64,
    pub jain_fairness_millionths: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentionSimulationError {
    EmptyNodes,
    EmptyQueue,
    EmptyRounds,
    EmptyLogicalPacket,
    ProbabilityOutsideRange { per_mille: u16 },
    LogicalPacketTooLarge { bytes: usize, maximum: usize },
    Profile(TurboProfileError),
    CoreRejectedScenario,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionMeters {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropagationModel {
    reference_loss_db: f64,
    path_loss_exponent: f64,
    fading_span_db: f64,
    receiver_sensitivity_dbm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropagationModelError {
    NonFiniteQuantity,
    NonPositivePathLossExponent,
    NegativeFadingSpan,
}

impl PropagationModel {
    pub fn new(
        reference_loss_db: f64,
        path_loss_exponent: f64,
        fading_span_db: f64,
        receiver_sensitivity_dbm: f64,
    ) -> Result<Self, PropagationModelError> {
        if !reference_loss_db.is_finite()
            || !path_loss_exponent.is_finite()
            || !fading_span_db.is_finite()
            || !receiver_sensitivity_dbm.is_finite()
        {
            return Err(PropagationModelError::NonFiniteQuantity);
        }
        if path_loss_exponent <= 0.0 {
            return Err(PropagationModelError::NonPositivePathLossExponent);
        }
        if fading_span_db < 0.0 {
            return Err(PropagationModelError::NegativeFadingSpan);
        }
        Ok(Self {
            reference_loss_db,
            path_loss_exponent,
            fading_span_db,
            receiver_sensitivity_dbm,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkSimulation {
    pub seed: u64,
    pub trials: u32,
    pub transmit_power_dbm: f64,
    pub transmitter: PositionMeters,
    pub receiver: PositionMeters,
    pub obstruction_loss_db: f64,
    pub model: PropagationModel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSimulationError {
    EmptyTrials,
    NonFiniteQuantity,
    NegativeObstructionLoss,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkSimulationResult {
    pub distance_meters: f64,
    pub nominal_received_power_dbm: f64,
    pub nominal_link_margin_db: f64,
    pub deliveries: u32,
    pub delivery_per_mille: u16,
}

pub fn simulate_contention(
    input: ContentionSimulation,
    profile: TurboPhyProfile,
) -> Result<ContentionSimulationResult, ContentionSimulationError> {
    if input.nodes == 0 {
        return Err(ContentionSimulationError::EmptyNodes);
    }
    if input.queued_packets_per_node == 0 {
        return Err(ContentionSimulationError::EmptyQueue);
    }
    if input.rounds == 0 {
        return Err(ContentionSimulationError::EmptyRounds);
    }
    if input.logical_packet_bytes == 0 {
        return Err(ContentionSimulationError::EmptyLogicalPacket);
    }
    if input.packet_loss_per_mille > 1_000 {
        return Err(ContentionSimulationError::ProbabilityOutsideRange {
            per_mille: input.packet_loss_per_mille,
        });
    }
    if input.logical_packet_bytes > TURBO_LOGICAL_PACKET_MAX {
        return Err(ContentionSimulationError::LogicalPacketTooLarge {
            bytes: input.logical_packet_bytes,
            maximum: TURBO_LOGICAL_PACKET_MAX,
        });
    }
    profile
        .validate()
        .map_err(ContentionSimulationError::Profile)?;
    let mut rng = DeterministicRng::new(input.seed);
    let mut delivered = std::vec![0u64; input.nodes as usize];
    let mut remaining = std::vec![input.queued_packets_per_node; input.nodes as usize];
    let mut latencies = std::vec::Vec::new();
    let mut collisions = 0u64;
    let mut occupied_airtime_us = 0u64;
    let hardware_support = TurboHardwareSupport {
        bit_rate: CapabilitySupport::Supported,
        frequency_deviation: CapabilitySupport::Supported,
        receiver_bandwidth: CapabilitySupport::Supported,
        gaussian_filter: CapabilitySupport::Supported,
        modulation_index: CapabilitySupport::Supported,
        whitening: CapabilitySupport::Supported,
        variable_length_packets: CapabilitySupport::Supported,
        preamble: CapabilitySupport::Supported,
        sync_word: CapabilitySupport::Supported,
        crc: CapabilitySupport::Supported,
    };
    let timing = TransmissionTimingBudget::new(1_000, 500, 1_000, 250, 1_000)
        .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?;
    let power = Us915PowerInputs::new(
        ConductedPowerDbm::new(22).map_err(|_| ContentionSimulationError::CoreRejectedScenario)?,
        AntennaGainDeciDb::new(0),
        ConductedPowerDbm::new(22).map_err(|_| ContentionSimulationError::CoreRejectedScenario)?,
    );
    let mut transmitters = std::vec::Vec::with_capacity(input.nodes as usize);
    for node in 0..input.nodes {
        let clock = TrustedScheduleClock::new(
            MonotonicMicros::new(0),
            ScheduleMicros::new(0),
            100,
            1,
            TrustedTimeSource::Gnss,
            UtcTimescale::PosixUnsmeared,
        )
        .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?;
        let node_configuration = Us915TurboConfiguration::new(
            MeasuredTwentyDbBandwidth::new(400_000)
                .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?,
            timing,
            MaximumTransmitUncertainty::new(5_000)
                .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?,
            power,
            hardware_support,
            super::TurboTransmitterInstanceId::new(u64::from(node) + 1)
                .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?,
        );
        transmitters.push(
            Us915TurboTransmitter::new(MonotonicMicros::new(0), clock, node_configuration)
                .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?,
        );
    }
    let payload = [0u8; TURBO_LOGICAL_PACKET_MAX];
    for round in 0..input.rounds {
        if remaining.iter().all(|queued| *queued == 0) {
            break;
        }
        let slot_origin_us =
            10_000_000u64.saturating_add(u64::from(round) * US915_TURBO_SPEC.slot_us());
        let access_begins_us = slot_origin_us.saturating_add(5_000);
        let channel = US915_TURBO_SPEC
            .slot_at(ScheduleMicros::new(access_begins_us))
            .channel_index()
            .index();
        let mut grants = std::vec::Vec::new();
        for (node, queued) in remaining.iter().copied().enumerate() {
            if queued == 0 {
                continue;
            }
            let mut access = ChannelAccess::begin(
                ContentionPolicy::turbo(),
                channel,
                ContentionClass::Fresh {
                    recent_airtime_per_mille: 0,
                },
                rng.next_u16(),
                MonotonicMicros::new(access_begins_us),
            )
            .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?;
            let mut access_time_us = access_begins_us;
            let grant = loop {
                access_time_us = access_time_us.saturating_add(1_000);
                match access
                    .observe(
                        MonotonicMicros::new(access_time_us),
                        ChannelAccessEvent::Clear,
                    )
                    .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?
                {
                    ChannelAccessAction::PerformFinalClear => {
                        access_time_us = access_time_us.saturating_add(1);
                        match access
                            .observe(
                                MonotonicMicros::new(access_time_us),
                                ChannelAccessEvent::Clear,
                            )
                            .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?
                        {
                            ChannelAccessAction::Granted(grant) => break grant,
                            _ => return Err(ContentionSimulationError::CoreRejectedScenario),
                        }
                    }
                    ChannelAccessAction::Wait => {}
                    _ => return Err(ContentionSimulationError::CoreRejectedScenario),
                }
            };
            grants.push((node, access_time_us, grant));
        }
        let Some(earliest_us) = grants.iter().map(|(_, time, _)| *time).min() else {
            break;
        };
        let simultaneous = grants
            .iter()
            .filter(|(_, time, _)| *time == earliest_us)
            .count();
        if simultaneous > 1 {
            collisions = collisions.saturating_add(1);
        }
        for (node, grant_time_us, grant) in grants {
            if grant_time_us != earliest_us {
                continue;
            }
            let prepared = transmitters[node]
                .prepare_after_final_clear(
                    grant,
                    MonotonicMicros::new(grant_time_us),
                    DatagramId::new([round as u8, node as u8, remaining[node]]),
                    &payload[..input.logical_packet_bytes],
                )
                .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?;
            let keyed_airtime_us = prepared.keyed_airtime_us();
            let starts_at_us = grant_time_us.saturating_add(1);
            let active = transmitters[node]
                .mark_rf_started(prepared, MonotonicMicros::new(starts_at_us))
                .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?;
            let completed_at_us = starts_at_us
                .saturating_add(keyed_airtime_us)
                .saturating_add(
                    if input.logical_packet_bytes > super::TURBO_FRAME_DATA_MAX {
                        timing.interframe_us()
                    } else {
                        0
                    },
                );
            transmitters[node]
                .complete(
                    active,
                    MonotonicMicros::new(completed_at_us),
                    keyed_airtime_us,
                )
                .map_err(|_| ContentionSimulationError::CoreRejectedScenario)?;
            occupied_airtime_us = occupied_airtime_us.saturating_add(keyed_airtime_us);
            let delivered_packet =
                simultaneous == 1 && !random_event(&mut rng, input.packet_loss_per_mille);
            if delivered_packet {
                remaining[node] -= 1;
                delivered[node] = delivered[node].saturating_add(1);
                latencies.push(completed_at_us.saturating_sub(10_000_000));
            }
        }
    }
    latencies.sort_unstable();
    let sum: u64 = delivered.iter().sum();
    let squares: u128 = delivered
        .iter()
        .map(|value| u128::from(*value) * u128::from(*value))
        .sum();
    let fairness = if squares == 0 {
        0
    } else {
        (u128::from(sum) * u128::from(sum) * 1_000_000 / (input.nodes as u128 * squares)) as u32
    };
    Ok(ContentionSimulationResult {
        delivered_packets: sum,
        collisions,
        occupied_airtime_us,
        p95_competing_node_latency_us: percentile(&latencies, 95),
        jain_fairness_millionths: fairness,
    })
}

pub fn simulate_link(input: LinkSimulation) -> Result<LinkSimulationResult, LinkSimulationError> {
    if input.trials == 0 {
        return Err(LinkSimulationError::EmptyTrials);
    }
    if !input.transmit_power_dbm.is_finite()
        || !input.transmitter.x.is_finite()
        || !input.transmitter.y.is_finite()
        || !input.transmitter.z.is_finite()
        || !input.receiver.x.is_finite()
        || !input.receiver.y.is_finite()
        || !input.receiver.z.is_finite()
        || !input.obstruction_loss_db.is_finite()
    {
        return Err(LinkSimulationError::NonFiniteQuantity);
    }
    if input.obstruction_loss_db < 0.0 {
        return Err(LinkSimulationError::NegativeObstructionLoss);
    }
    let dx = input.transmitter.x - input.receiver.x;
    let dy = input.transmitter.y - input.receiver.y;
    let dz = input.transmitter.z - input.receiver.z;
    let distance_meters = (dx * dx + dy * dy + dz * dz).sqrt();
    let path_loss_db = input.model.reference_loss_db
        + 10.0 * input.model.path_loss_exponent * distance_meters.max(1.0).log10();
    let nominal_received_power_dbm =
        input.transmit_power_dbm - path_loss_db - input.obstruction_loss_db;
    let nominal_link_margin_db = nominal_received_power_dbm - input.model.receiver_sensitivity_dbm;
    let mut rng = DeterministicRng::new(input.seed);
    let mut deliveries = 0u32;
    for _ in 0..input.trials {
        let unit = f64::from(rng.next_u16()) / f64::from(u16::MAX);
        let fading_db = (unit * 2.0 - 1.0) * input.model.fading_span_db;
        if nominal_received_power_dbm + fading_db >= input.model.receiver_sensitivity_dbm {
            deliveries = deliveries.saturating_add(1);
        }
    }
    Ok(LinkSimulationResult {
        distance_meters,
        nominal_received_power_dbm,
        nominal_link_margin_db,
        deliveries,
        delivery_per_mille: (u64::from(deliveries) * 1_000 / u64::from(input.trials)) as u16,
    })
}

fn random_event(rng: &mut DeterministicRng, probability_per_mille: u16) -> bool {
    rng.next_u16() as u32 * 1_000 / (u16::MAX as u32 + 1) < u32::from(probability_per_mille)
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let index = (values.len() - 1) * percentile / 100;
    values[index]
}

struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn next_u16(&mut self) -> u16 {
        self.next_u64() as u16
    }
}
