use core::num::NonZeroUsize;

use heapless::Vec as HeaplessVec;
use prns_core::lemire_index::LemireIndex;

use crate::interfaces::{RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb};
use crate::routing::dedup::{dedup_index_buckets, PacketHash};

use super::{PacketMetricStorage, PacketPhyRetention};

pub struct FixedPacketMetricStorage<Metric, const CAPACITY: usize, const BUCKETS: usize> {
    packet_hashes: HeaplessVec<PacketHash, CAPACITY>,
    metrics: HeaplessVec<Metric, CAPACITY>,
    index: LemireIndex<BUCKETS>,
}

impl<Metric, const CAPACITY: usize, const BUCKETS: usize>
    FixedPacketMetricStorage<Metric, CAPACITY, BUCKETS>
{
    pub const fn new() -> Self {
        const {
            assert!(
                CAPACITY > 0,
                "fixed packet PHY retention capacity must be non-zero"
            );
            assert!(
                CAPACITY < u16::MAX as usize,
                "fixed packet PHY retention exceeds its index slot range"
            );
            assert!(
                BUCKETS >= dedup_index_buckets(CAPACITY),
                "fixed packet PHY index needs two-thirds-load headroom"
            );
        }
        Self {
            packet_hashes: HeaplessVec::new(),
            metrics: HeaplessVec::new(),
            index: LemireIndex::new(),
        }
    }
}

impl<Metric, const CAPACITY: usize, const BUCKETS: usize> Default
    for FixedPacketMetricStorage<Metric, CAPACITY, BUCKETS>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Metric: Copy, const CAPACITY: usize, const BUCKETS: usize> PacketMetricStorage
    for FixedPacketMetricStorage<Metric, CAPACITY, BUCKETS>
{
    type Metric = Metric;

    fn capacity(&self) -> NonZeroUsize {
        match NonZeroUsize::new(CAPACITY) {
            Some(capacity) => capacity,
            None => unreachable!("fixed packet PHY retention capacity is non-zero"),
        }
    }

    fn len(&self) -> usize {
        self.packet_hashes.len()
    }

    fn append(&mut self, packet_hash: PacketHash, metric: Metric) {
        let slot = self.packet_hashes.len();
        assert!(
            self.packet_hashes.push(packet_hash).is_ok(),
            "shared packet PHY retention exceeded fixed hash capacity"
        );
        assert!(
            self.metrics.push(metric).is_ok(),
            "shared packet PHY retention exceeded fixed metric capacity"
        );
        self.index.insert(slot, &self.packet_hashes);
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

pub type FixedPacketPhyRetention<const CAPACITY: usize, const BUCKETS: usize> = PacketPhyRetention<
    FixedPacketMetricStorage<RssiDbm, CAPACITY, BUCKETS>,
    FixedPacketMetricStorage<SnrQuarterDb, CAPACITY, BUCKETS>,
    FixedPacketMetricStorage<SignalQualityTenthsPercent, CAPACITY, BUCKETS>,
>;

pub const fn fixed_packet_phy_retention<const CAPACITY: usize, const BUCKETS: usize>(
) -> FixedPacketPhyRetention<CAPACITY, BUCKETS> {
    PacketPhyRetention::from_storages(
        FixedPacketMetricStorage::new(),
        FixedPacketMetricStorage::new(),
        FixedPacketMetricStorage::new(),
    )
}
