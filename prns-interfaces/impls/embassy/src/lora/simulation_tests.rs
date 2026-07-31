use core::cmp::{max, min};

use prns_core::interfaces::lora::{
    ModemPreset, RadioProfile, DEFAULT_915_PROFILE, LORA_MAX_PAYLOAD, LORA_SINGLE_FRAME_MAX,
};

use super::airtime_quantum::{AirtimeQuantum, ServiceAge};
use super::channel_access::{
    ChannelAccess, ChannelAccessAction, ChannelObservation, ChannelTiming, ContentionPriority,
};
use super::{packet_airtime, LORA_TX_QUEUE_BYTES};

const MAX_NODES: usize = 8;
const HISTORY_LEN: usize = 32;
const SHORT_WINDOW_MS: u64 = 15_000;
const SIMULATION_MS: u64 = 3_600_000;
const RNODE_QUEUE_PACKETS: usize = 200;

const TINY: [usize; 1] = [1];
const HUNDRED: [usize; 1] = [100];
const ANNOUNCE: [usize; 1] = [220];
const MAX_SINGLE: [usize; 1] = [LORA_SINGLE_FRAME_MAX - 1];
const MAX_SPLIT: [usize; 1] = [LORA_MAX_PAYLOAD];
const MIXED: [usize; 5] = [1, 100, 220, LORA_SINGLE_FRAME_MAX - 1, LORA_MAX_PAYLOAD];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Prns,
    Rnode,
}

#[derive(Clone, Copy)]
struct Traffic {
    name: &'static str,
    lengths: &'static [usize],
}

const TRAFFIC: [Traffic; 6] = [
    Traffic {
        name: "tiny",
        lengths: &TINY,
    },
    Traffic {
        name: "100-byte",
        lengths: &HUNDRED,
    },
    Traffic {
        name: "announce",
        lengths: &ANNOUNCE,
    },
    Traffic {
        name: "maximum-single",
        lengths: &MAX_SINGLE,
    },
    Traffic {
        name: "maximum-split",
        lengths: &MAX_SPLIT,
    },
    Traffic {
        name: "mixed",
        lengths: &MIXED,
    },
];

#[derive(Clone, Copy)]
struct AirtimeHistory {
    starts_ms: [u64; HISTORY_LEN],
    ends_ms: [u64; HISTORY_LEN],
    cursor: usize,
    len: usize,
}

impl AirtimeHistory {
    const fn new() -> Self {
        Self {
            starts_ms: [0; HISTORY_LEN],
            ends_ms: [0; HISTORY_LEN],
            cursor: 0,
            len: 0,
        }
    }

    fn record(&mut self, start_ms: u64, airtime_us: u64) {
        let duration_ms = airtime_us.saturating_add(999) / 1_000;
        self.starts_ms[self.cursor] = start_ms;
        self.ends_ms[self.cursor] = start_ms.saturating_add(duration_ms);
        self.cursor = (self.cursor + 1) % HISTORY_LEN;
        self.len = min(self.len + 1, HISTORY_LEN);
    }

    fn short_per_mille(self, now_ms: u64) -> u16 {
        let window_start_ms = now_ms.saturating_sub(SHORT_WINDOW_MS);
        let mut occupied_ms = 0u64;
        for index in 0..self.len {
            let start_ms = max(self.starts_ms[index], window_start_ms);
            let end_ms = min(self.ends_ms[index], now_ms);
            occupied_ms = occupied_ms.saturating_add(end_ms.saturating_sub(start_ms));
        }
        occupied_ms
            .saturating_mul(1_000)
            .saturating_div(SHORT_WINDOW_MS)
            .min(1_000) as u16
    }
}

#[derive(Clone, Copy)]
struct RnodeAccess {
    difs_clear_ms: u64,
    cw_wait_passed: bool,
    remaining_ms: u64,
}

impl RnodeAccess {
    const fn new() -> Self {
        Self {
            difs_clear_ms: 0,
            cw_wait_passed: false,
            remaining_ms: 0,
        }
    }

    fn observe_clear(
        &mut self,
        timing: ChannelTiming,
        entropy: u16,
        short_airtime_per_mille: u16,
    ) -> bool {
        if self.difs_clear_ms < timing.slot_ms() * 2 {
            self.difs_clear_ms = self.difs_clear_ms.saturating_add(timing.sample_ms());
            return false;
        }
        if !self.cw_wait_passed {
            let band = self_airtime_band(short_airtime_per_mille);
            let slots = u64::from(entropy % 15) + u64::from(band) * 15;
            self.remaining_ms = slots.saturating_mul(timing.slot_ms());
            self.cw_wait_passed = true;
            return self.remaining_ms == 0;
        }
        self.remaining_ms = self.remaining_ms.saturating_sub(timing.sample_ms());
        self.remaining_ms == 0
    }

    fn observe_busy(&mut self) {
        // Stable RNode restarts DIFS but deliberately preserves cw_wait_passed
        // and the selected wait across channel activity.
        self.difs_clear_ms = 0;
    }

    fn reset_after_attempt(&mut self) {
        *self = Self::new();
    }
}

struct Node {
    family: Family,
    rng: u32,
    access: Option<ChannelAccess>,
    rnode_access: RnodeAccess,
    service_age: ServiceAge,
    continuation: bool,
    eligible_at_ms: u64,
    poll_at_ms: u64,
    history: AirtimeHistory,
    delivered_us: u64,
    remaining_us: Option<u64>,
}

impl Node {
    fn new(
        family: Family,
        profile: RadioProfile,
        index: usize,
        workload_us: Option<u64>,
        seed_salt: u32,
    ) -> Self {
        Self {
            family,
            rng: 0x9e37_79b9 ^ (index as u32 + 1) ^ seed_salt.wrapping_mul(0x85eb_ca6b),
            access: None,
            rnode_access: RnodeAccess::new(),
            service_age: ServiceAge::new(profile),
            continuation: false,
            eligible_at_ms: 0,
            poll_at_ms: 0,
            history: AirtimeHistory::new(),
            delivered_us: 0,
            remaining_us: workload_us,
        }
    }

    fn entropy(&mut self) -> u16 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as u16
    }
}

#[derive(Clone, Copy)]
struct SimulationResult {
    busy_us: u64,
    attempted_us: u64,
    collided_us: u64,
    collisions: u64,
    delivered_us: [u64; MAX_NODES],
    elapsed_ms: u64,
}

fn profile(preset: ModemPreset) -> RadioProfile {
    RadioProfile {
        modulation: preset.modulation(),
        ..DEFAULT_915_PROFILE
    }
}

fn self_airtime_band(short_airtime_per_mille: u16) -> u8 {
    match short_airtime_per_mille {
        0..=70 => 0,
        71..=450 => 1,
        451..=840 => 2,
        _ => 3,
    }
}

fn packet_airtime_us(profile: RadioProfile, len: usize) -> u64 {
    let packet = [0u8; LORA_MAX_PAYLOAD];
    packet_airtime(&packet[..len], &profile)
}

fn opportunity_airtime_us(
    family: Family,
    profile: RadioProfile,
    traffic: Traffic,
    rnode_limited: bool,
    fifo_bytes: usize,
) -> u64 {
    match family {
        Family::Prns => {
            let quantum = AirtimeQuantum::for_profile(profile);
            let mut used_us = 0u64;
            let mut queued_bytes = 0usize;
            let mut index = 0usize;
            loop {
                let len = traffic.lengths[index % traffic.lengths.len()];
                let record_bytes = len + 2;
                if index > 0 && queued_bytes.saturating_add(record_bytes) > fifo_bytes {
                    break;
                }
                let packet_us = packet_airtime_us(profile, len);
                if !quantum.permits(used_us, packet_us) {
                    break;
                }
                used_us = used_us.saturating_add(packet_us);
                if index > 0 {
                    queued_bytes += record_bytes;
                }
                index += 1;
            }
            used_us
        }
        Family::Rnode => {
            let mut used_us = 0u64;
            let mut queued_bytes = 0usize;
            let mut index = 0usize;
            loop {
                let len = traffic.lengths[index % traffic.lengths.len()];
                if index >= RNODE_QUEUE_PACKETS || queued_bytes.saturating_add(len) > fifo_bytes {
                    break;
                }
                used_us = used_us.saturating_add(packet_airtime_us(profile, len));
                queued_bytes += len;
                index += 1;
                if rnode_limited {
                    break;
                }
            }
            used_us
        }
    }
}

fn simulate(
    families: &[Family],
    profile: RadioProfile,
    traffic: Traffic,
    rnode_limited: bool,
    workload_us: Option<u64>,
) -> SimulationResult {
    simulate_seeded(families, profile, traffic, rnode_limited, workload_us, 0)
}

fn simulate_seeded(
    families: &[Family],
    profile: RadioProfile,
    traffic: Traffic,
    rnode_limited: bool,
    workload_us: Option<u64>,
    seed_salt: u32,
) -> SimulationResult {
    let node_count = families.len();
    let timing = ChannelTiming::for_profile(profile);
    let sample_ms = timing.sample_ms();
    let logical_packet_us = packet_airtime_us(profile, traffic.lengths[0]);
    let mut nodes: [Node; MAX_NODES] = core::array::from_fn(|index| {
        Node::new(
            families[index.min(node_count - 1)],
            profile,
            index,
            workload_us,
            seed_salt,
        )
    });
    let mut now_ms = 0u64;
    let mut busy_us = 0u64;
    let mut attempted_us = 0u64;
    let mut collided_us = 0u64;
    let mut collisions = 0u64;

    while now_ms < SIMULATION_MS {
        if workload_us.is_some()
            && nodes[..node_count]
                .iter()
                .all(|node| node.remaining_us == Some(0))
        {
            break;
        }
        let mut candidates = [false; MAX_NODES];
        for (index, node) in nodes[..node_count].iter_mut().enumerate() {
            if node.remaining_us == Some(0) {
                continue;
            }
            if now_ms < node.eligible_at_ms || now_ms < node.poll_at_ms {
                continue;
            }
            match node.family {
                Family::Prns => {
                    if node.access.is_none() {
                        let priority = if core::mem::take(&mut node.continuation) {
                            ContentionPriority::Continuation
                        } else {
                            ContentionPriority::Fresh {
                                short_airtime_per_mille: node.history.short_per_mille(now_ms),
                            }
                        };
                        node.access = Some(ChannelAccess::new_at(
                            profile,
                            now_ms,
                            now_ms,
                            logical_packet_us,
                            priority,
                        ));
                    }
                    let entropy = node.entropy();
                    let access = node.access.as_mut().expect("access was just installed");
                    let mut action = access.observe(
                        now_ms,
                        ChannelObservation::Clear,
                        node.service_age.backoff_rate(),
                    );
                    if matches!(action, ChannelAccessAction::NeedBackoffEntropy) {
                        assert!(access.choose_backoff(entropy));
                        action = access.after_entropy();
                    }
                    if matches!(action, ChannelAccessAction::ReadyForFinalCheck) {
                        action = access.final_check(now_ms, ChannelObservation::Clear);
                    }
                    if matches!(action, ChannelAccessAction::Expired) {
                        node.access = None;
                        node.poll_at_ms = now_ms;
                    } else {
                        candidates[index] = matches!(action, ChannelAccessAction::Transmit);
                        if !candidates[index] {
                            node.poll_at_ms = now_ms.saturating_add(
                                access.next_poll_ms(node.service_age.backoff_rate()),
                            );
                        }
                    }
                }
                Family::Rnode => {
                    let entropy = node.entropy();
                    candidates[index] = node.rnode_access.observe_clear(
                        timing,
                        entropy,
                        node.history.short_per_mille(now_ms),
                    );
                    if !candidates[index] {
                        node.poll_at_ms = now_ms.saturating_add(sample_ms);
                    }
                }
            }
        }

        let candidate_count = candidates[..node_count]
            .iter()
            .filter(|candidate| **candidate)
            .count();
        if candidate_count == 0 {
            let next_ms = nodes[..node_count]
                .iter()
                .map(|node| max(node.eligible_at_ms, node.poll_at_ms))
                .min()
                .unwrap_or_else(|| now_ms.saturating_add(1));
            now_ms = if next_ms > now_ms {
                next_ms
            } else {
                now_ms.saturating_add(1)
            };
            continue;
        }

        let mut occupied_us = 0u64;
        let successful = candidate_count == 1;
        if !successful {
            collisions = collisions.saturating_add(1);
        }
        for index in 0..node_count {
            if !candidates[index] {
                continue;
            }
            let attempt_us = opportunity_airtime_us(
                nodes[index].family,
                profile,
                traffic,
                rnode_limited,
                LORA_TX_QUEUE_BYTES,
            )
            .min(nodes[index].remaining_us.unwrap_or(u64::MAX));
            occupied_us = max(occupied_us, attempt_us);
            attempted_us = attempted_us.saturating_add(attempt_us);
            if !successful {
                collided_us = collided_us.saturating_add(attempt_us);
            }
            nodes[index].history.record(now_ms, attempt_us);
            if successful {
                nodes[index].delivered_us = nodes[index].delivered_us.saturating_add(attempt_us);
                if let Some(remaining_us) = nodes[index].remaining_us.as_mut() {
                    *remaining_us = remaining_us.saturating_sub(attempt_us);
                }
            }
            nodes[index].eligible_at_ms =
                now_ms.saturating_add(attempt_us.saturating_add(999) / 1_000);
            nodes[index].poll_at_ms = nodes[index].eligible_at_ms;
            match nodes[index].family {
                Family::Prns => {
                    nodes[index].service_age.consume();
                    if nodes[index].remaining_us != Some(0) {
                        nodes[index].service_age.seed_continuation();
                        nodes[index].continuation = true;
                    }
                    nodes[index].access = None;
                }
                Family::Rnode => nodes[index].rnode_access.reset_after_attempt(),
            }
        }

        let occupied_ms = occupied_us.saturating_add(999) / 1_000;
        let busy_end_ms = now_ms.saturating_add(occupied_ms);
        busy_us = busy_us.saturating_add(
            min(busy_end_ms, SIMULATION_MS)
                .saturating_sub(now_ms)
                .saturating_mul(1_000),
        );
        for index in 0..node_count {
            if candidates[index] {
                continue;
            }
            if successful
                && nodes[index].remaining_us != Some(0)
                && matches!(nodes[index].family, Family::Prns)
            {
                nodes[index].service_age.record_peer_airtime(occupied_us);
            }
            match nodes[index].family {
                Family::Prns => {
                    let Some(access) = nodes[index].access.as_mut() else {
                        continue;
                    };
                    if matches!(
                        access.observe(
                            busy_end_ms,
                            ChannelObservation::Busy,
                            nodes[index].service_age.backoff_rate(),
                        ),
                        ChannelAccessAction::Expired
                    ) {
                        nodes[index].access = None;
                        nodes[index].poll_at_ms = busy_end_ms;
                    } else if let Some(access) = nodes[index].access.as_ref() {
                        nodes[index].poll_at_ms = busy_end_ms.saturating_add(
                            access.next_poll_ms(nodes[index].service_age.backoff_rate()),
                        );
                    }
                }
                Family::Rnode => {
                    nodes[index].rnode_access.observe_busy();
                    nodes[index].poll_at_ms = busy_end_ms.saturating_add(sample_ms);
                }
            }
        }
        now_ms = busy_end_ms;
    }

    let mut delivered_us = [0u64; MAX_NODES];
    for index in 0..node_count {
        delivered_us[index] = nodes[index].delivered_us;
    }
    SimulationResult {
        busy_us,
        attempted_us,
        collided_us,
        collisions,
        delivered_us,
        elapsed_ms: now_ms.max(1),
    }
}

fn jain_at_least(delivered_us: &[u64], numerator: u128, denominator: u128) -> bool {
    let sum = delivered_us
        .iter()
        .map(|value| u128::from(*value))
        .sum::<u128>();
    let squares = delivered_us
        .iter()
        .map(|value| u128::from(*value).pow(2))
        .sum::<u128>();
    denominator * sum.pow(2) >= numerator * delivered_us.len() as u128 * squares
}

fn utilization_per_mille(result: SimulationResult) -> u64 {
    result.busy_us.saturating_mul(1_000) / (result.elapsed_ms * 1_000)
}

#[test]
fn rnode_model_flushes_normal_queue_and_pops_one_when_limited() {
    for preset in [
        ModemPreset::ShortFast,
        ModemPreset::MediumFast,
        ModemPreset::LongFast,
        ModemPreset::LongSlow,
    ] {
        let profile = profile(preset);
        for traffic in TRAFFIC {
            let normal =
                opportunity_airtime_us(Family::Rnode, profile, traffic, false, LORA_TX_QUEUE_BYTES);
            let limited =
                opportunity_airtime_us(Family::Rnode, profile, traffic, true, LORA_TX_QUEUE_BYTES);
            assert_eq!(limited, packet_airtime_us(profile, traffic.lengths[0]));
            assert!(normal >= limited, "{preset:?} {}", traffic.name);
            assert_eq!(
                opportunity_airtime_us(Family::Rnode, profile, traffic, false, 0),
                0
            );
        }
    }
}

#[test]
fn prns_opportunities_are_strictly_quantum_bounded_for_full_and_partial_fifos() {
    for preset in [
        ModemPreset::ShortFast,
        ModemPreset::MediumFast,
        ModemPreset::LongFast,
        ModemPreset::LongSlow,
    ] {
        let profile = profile(preset);
        let quantum_us = AirtimeQuantum::for_profile(profile).us();
        for traffic in TRAFFIC {
            for fifo_bytes in [0, 512, 2_048, LORA_TX_QUEUE_BYTES] {
                let used_us =
                    opportunity_airtime_us(Family::Prns, profile, traffic, false, fifo_bytes);
                assert!(used_us > 0, "{preset:?} {}", traffic.name);
                assert!(used_us <= quantum_us, "{preset:?} {}", traffic.name);
            }
        }
    }
}

#[test]
fn solo_saturated_prns_meets_the_utilization_target_and_rnode_shape() {
    for preset in [
        ModemPreset::ShortFast,
        ModemPreset::MediumFast,
        ModemPreset::LongFast,
        ModemPreset::LongSlow,
    ] {
        let profile = profile(preset);
        for traffic in TRAFFIC {
            let prns = simulate(&[Family::Prns], profile, traffic, false, None);
            let rnode = simulate(&[Family::Rnode], profile, traffic, false, None);
            let prns_util = utilization_per_mille(prns);
            let rnode_util = utilization_per_mille(rnode);
            if matches!(preset, ModemPreset::LongFast) && traffic.name != "mixed" {
                assert!(prns_util >= 890, "{preset:?} {}: {prns_util}", traffic.name);
            }
            assert!(
                prns_util.saturating_add(50) >= rnode_util,
                "{preset:?} {}: PRNS {prns_util}, RNode {rnode_util}",
                traffic.name
            );
        }
    }
}

#[test]
fn saturated_prns_populations_are_fair_and_no_more_collision_prone() {
    for preset in [
        ModemPreset::ShortFast,
        ModemPreset::LongFast,
        ModemPreset::LongSlow,
    ] {
        let profile = profile(preset);
        for traffic in [TRAFFIC[1], TRAFFIC[2], TRAFFIC[4], TRAFFIC[5]] {
            for node_count in [2, 4, 8] {
                let prns_families = [Family::Prns; MAX_NODES];
                let rnode_families = [Family::Rnode; MAX_NODES];
                let mut prns_attempted = 0u128;
                let mut prns_collided = 0u128;
                let mut prns_collision_events = 0u64;
                let mut prns_delivered = [0u64; MAX_NODES];
                let mut rnode_attempted = 0u128;
                let mut rnode_collided = 0u128;
                let mut rnode_collision_events = 0u64;
                for seed_salt in 0..8 {
                    let prns_seed = simulate_seeded(
                        &prns_families[..node_count],
                        profile,
                        traffic,
                        false,
                        None,
                        seed_salt,
                    );
                    let rnode_seed = simulate_seeded(
                        &rnode_families[..node_count],
                        profile,
                        traffic,
                        false,
                        None,
                        seed_salt,
                    );
                    prns_attempted += u128::from(prns_seed.attempted_us);
                    prns_collided += u128::from(prns_seed.collided_us);
                    prns_collision_events += prns_seed.collisions;
                    for (delivered, seed_delivered) in prns_delivered[..node_count]
                        .iter_mut()
                        .zip(&prns_seed.delivered_us[..node_count])
                    {
                        *delivered = delivered.saturating_add(*seed_delivered);
                    }
                    rnode_attempted += u128::from(rnode_seed.attempted_us);
                    rnode_collided += u128::from(rnode_seed.collided_us);
                    rnode_collision_events += rnode_seed.collisions;
                }
                assert!(
                    jain_at_least(&prns_delivered[..node_count], 95, 100),
                    "{preset:?} {} {node_count}: {:?}",
                    traffic.name,
                    &prns_delivered[..node_count]
                );
                assert!(
                    prns_collided * rnode_attempted <= rnode_collided * prns_attempted,
                    "{preset:?} {} {node_count}: PRNS {prns_collision_events}, RNode {rnode_collision_events}",
                    traffic.name
                );
            }
        }
    }
}

#[test]
fn equal_mixed_populations_keep_family_and_per_node_airtime_balanced() {
    for preset in [
        ModemPreset::ShortFast,
        ModemPreset::LongFast,
        ModemPreset::LongSlow,
    ] {
        let profile = profile(preset);
        for traffic in [TRAFFIC[1], TRAFFIC[2], TRAFFIC[4], TRAFFIC[5]] {
            for node_count in [2, 4, 8] {
                let families: [Family; MAX_NODES] = core::array::from_fn(|index| {
                    if index % 2 == 0 {
                        Family::Prns
                    } else {
                        Family::Rnode
                    }
                });
                let workload_us =
                    opportunity_airtime_us(Family::Rnode, profile, traffic, false, 2_048);
                let mixed = simulate(
                    &families[..node_count],
                    profile,
                    traffic,
                    false,
                    Some(workload_us),
                );
                let baseline = simulate(
                    &[Family::Rnode; MAX_NODES][..node_count],
                    profile,
                    traffic,
                    false,
                    Some(workload_us),
                );
                let prns_us = (0..node_count)
                    .filter(|index| matches!(families[*index], Family::Prns))
                    .map(|index| mixed.delivered_us[index])
                    .sum::<u64>();
                let rnode_us = (0..node_count)
                    .filter(|index| matches!(families[*index], Family::Rnode))
                    .map(|index| mixed.delivered_us[index])
                    .sum::<u64>();
                let total_us = prns_us.saturating_add(rnode_us);
                let prns_share_per_mille = prns_us.saturating_mul(1_000) / total_us;
                assert!(
                    (400..=600).contains(&prns_share_per_mille),
                    "{preset:?} {} {node_count}: PRNS share {prns_share_per_mille}",
                    traffic.name
                );
                assert!(
                    jain_at_least(&mixed.delivered_us[..node_count], 90, 100),
                    "{preset:?} {} {node_count}: {:?}",
                    traffic.name,
                    &mixed.delivered_us[..node_count]
                );
                assert!(
                    utilization_per_mille(mixed).abs_diff(utilization_per_mille(baseline)) <= 50,
                    "{preset:?} {} {node_count}: mixed {}, baseline {}",
                    traffic.name,
                    utilization_per_mille(mixed),
                    utilization_per_mille(baseline)
                );
            }
        }
    }
}
