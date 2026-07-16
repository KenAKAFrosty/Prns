use prns_core::lemire_index::HeapLemireIndex;

use crate::interfaces::{PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb};
use crate::routing::dedup::PacketHash;

const PACKET_PHY_CAPACITY: usize = 512;

struct PacketMetricRetention<Metric> {
    packet_hashes: Vec<PacketHash>,
    metrics: Vec<Metric>,
    index: HeapLemireIndex,
    next_evict: usize,
}

impl<Metric: Copy> PacketMetricRetention<Metric> {
    fn new() -> Self {
        Self {
            packet_hashes: Vec::with_capacity(PACKET_PHY_CAPACITY),
            metrics: Vec::with_capacity(PACKET_PHY_CAPACITY),
            index: HeapLemireIndex::default(),
            next_evict: 0,
        }
    }

    fn remember(&mut self, packet_hash: PacketHash, metric: Metric) {
        if self.packet_hashes.len() < PACKET_PHY_CAPACITY {
            self.packet_hashes.push(packet_hash);
            self.metrics.push(metric);
            self.index
                .insert(self.packet_hashes.len() - 1, &self.packet_hashes);
            return;
        }
        let slot = self.next_evict;
        self.index.remove_slot(slot, &self.packet_hashes);
        self.packet_hashes[slot] = packet_hash;
        self.metrics[slot] = metric;
        self.index.insert(slot, &self.packet_hashes);
        self.next_evict = (self.next_evict + 1) % PACKET_PHY_CAPACITY;
    }

    fn get(&self, packet_hash: PacketHash) -> Option<Metric> {
        self.index
            .get(&packet_hash, &self.packet_hashes)
            .map(|slot| self.metrics[slot])
    }
}

pub(super) struct PacketPhyRetention {
    rssi: PacketMetricRetention<RssiDbm>,
    snr: PacketMetricRetention<SnrQuarterDb>,
    quality: PacketMetricRetention<SignalQualityTenthsPercent>,
}

impl PacketPhyRetention {
    pub(super) fn new() -> Self {
        Self {
            rssi: PacketMetricRetention::new(),
            snr: PacketMetricRetention::new(),
            quality: PacketMetricRetention::new(),
        }
    }

    pub(super) fn remember(&mut self, packet_hash: PacketHash, stats: PacketPhyStats) {
        if let Some(rssi) = stats.rssi {
            self.rssi.remember(packet_hash, rssi);
        }
        if let Some(snr) = stats.snr {
            self.snr.remember(packet_hash, snr);
        }
        if let Some(quality) = stats.quality {
            self.quality.remember(packet_hash, quality);
        }
    }

    pub(super) fn get(&self, packet_hash: PacketHash) -> Option<PacketPhyStats> {
        let stats = PacketPhyStats {
            rssi: self.rssi.get(packet_hash),
            snr: self.snr.get(packet_hash),
            quality: self.quality.get(packet_hash),
        };
        (!stats.is_empty()).then_some(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet_hash(value: u16) -> PacketHash {
        let mut bytes = [0; 32];
        bytes[..2].copy_from_slice(&value.to_le_bytes());
        PacketHash::new(bytes)
    }

    #[test]
    fn partial_packet_phy_observations_share_one_query() {
        let mut retention = PacketPhyRetention::new();
        let packet_hash = packet_hash(7);
        retention.remember(
            packet_hash,
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-87)),
                snr: None,
                quality: None,
            },
        );
        retention.remember(
            packet_hash,
            PacketPhyStats {
                rssi: None,
                snr: Some(SnrQuarterDb::new(-9)),
                quality: SignalQualityTenthsPercent::new(875),
            },
        );

        assert_eq!(
            retention.get(packet_hash),
            Some(PacketPhyStats {
                rssi: Some(RssiDbm::new(-87)),
                snr: Some(SnrQuarterDb::new(-9)),
                quality: SignalQualityTenthsPercent::new(875),
            })
        );
    }

    #[test]
    fn packet_phy_retention_evicts_the_oldest_row_at_the_rns_capacity() {
        let mut retention = PacketPhyRetention::new();
        for value in 0..=PACKET_PHY_CAPACITY as u16 {
            retention.remember(
                packet_hash(value),
                PacketPhyStats {
                    rssi: Some(RssiDbm::new(value as i16)),
                    snr: None,
                    quality: None,
                },
            );
        }

        assert_eq!(retention.get(packet_hash(0)), None);
        assert_eq!(
            retention.get(packet_hash(PACKET_PHY_CAPACITY as u16)),
            Some(PacketPhyStats {
                rssi: Some(RssiDbm::new(PACKET_PHY_CAPACITY as i16)),
                snr: None,
                quality: None,
            })
        );
    }

    #[test]
    fn duplicate_packet_hash_advances_to_the_next_observation_after_fifo_eviction() {
        let mut retention = PacketMetricRetention::new();
        let repeated = packet_hash(7);
        retention.remember(repeated, RssiDbm::new(-90));
        retention.remember(repeated, RssiDbm::new(-80));
        for value in 1_000..1_000 + PACKET_PHY_CAPACITY as u16 - 2 {
            retention.remember(packet_hash(value), RssiDbm::new(-70));
        }

        assert_eq!(retention.get(repeated), Some(RssiDbm::new(-90)));

        retention.remember(packet_hash(2_000), RssiDbm::new(-60));

        assert_eq!(retention.get(repeated), Some(RssiDbm::new(-80)));
    }

    #[test]
    fn each_packet_phy_metric_has_its_own_rns_sized_retention_window() {
        let mut retention = PacketPhyRetention::new();
        let retained_snr = packet_hash(7);
        retention.remember(
            retained_snr,
            PacketPhyStats {
                rssi: None,
                snr: Some(SnrQuarterDb::new(-9)),
                quality: None,
            },
        );
        for value in 1_000..=1_000 + PACKET_PHY_CAPACITY as u16 {
            retention.remember(
                packet_hash(value),
                PacketPhyStats {
                    rssi: Some(RssiDbm::new(-70)),
                    snr: None,
                    quality: None,
                },
            );
        }

        assert_eq!(
            retention.get(retained_snr),
            Some(PacketPhyStats {
                rssi: None,
                snr: Some(SnrQuarterDb::new(-9)),
                quality: None,
            })
        );
    }
}
