use core::num::NonZeroUsize;

use alloc::vec::Vec;
use prns_core::lemire_index::HeapLemireIndex;

use crate::interfaces::{RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb};
use crate::routing::dedup::PacketHash;

use super::{PacketMetricStorage, PacketPhyRetention};

pub const RNS_1_4_2_PACKET_PHY_CAPACITY: usize = 512;

pub struct HeapPacketMetricStorage<Metric, const CAPACITY: usize> {
    packet_hashes: Vec<PacketHash>,
    metrics: Vec<Metric>,
    index: HeapLemireIndex,
}

impl<Metric, const CAPACITY: usize> Default for HeapPacketMetricStorage<Metric, CAPACITY> {
    fn default() -> Self {
        const {
            assert!(
                CAPACITY > 0,
                "packet PHY retention capacity must be non-zero"
            );
            assert!(
                CAPACITY < u32::MAX as usize,
                "heap packet PHY retention exceeds its index slot range"
            );
        }
        Self {
            packet_hashes: Vec::with_capacity(CAPACITY),
            metrics: Vec::with_capacity(CAPACITY),
            index: HeapLemireIndex::default(),
        }
    }
}

impl<Metric: Copy, const CAPACITY: usize> PacketMetricStorage
    for HeapPacketMetricStorage<Metric, CAPACITY>
{
    type Metric = Metric;

    fn capacity(&self) -> NonZeroUsize {
        match NonZeroUsize::new(CAPACITY) {
            Some(capacity) => capacity,
            None => unreachable!("packet PHY retention capacity is non-zero"),
        }
    }

    fn len(&self) -> usize {
        self.packet_hashes.len()
    }

    fn append(&mut self, packet_hash: PacketHash, metric: Metric) {
        self.packet_hashes.push(packet_hash);
        self.metrics.push(metric);
        self.index
            .insert(self.packet_hashes.len() - 1, &self.packet_hashes);
    }

    fn replace(&mut self, slot: usize, packet_hash: PacketHash, metric: Metric) {
        self.index.remove_slot(slot, &self.packet_hashes);
        self.packet_hashes[slot] = packet_hash;
        self.metrics[slot] = metric;
        self.index.insert(slot, &self.packet_hashes);
    }

    fn get(&self, packet_hash: PacketHash) -> Option<Metric> {
        self.index
            .get(&packet_hash, &self.packet_hashes)
            .map(|slot| self.metrics[slot])
    }
}

pub type HeapPacketPhyRetention = PacketPhyRetention<
    HeapPacketMetricStorage<RssiDbm, RNS_1_4_2_PACKET_PHY_CAPACITY>,
    HeapPacketMetricStorage<SnrQuarterDb, RNS_1_4_2_PACKET_PHY_CAPACITY>,
    HeapPacketMetricStorage<SignalQualityTenthsPercent, RNS_1_4_2_PACKET_PHY_CAPACITY>,
>;
