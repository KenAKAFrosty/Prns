use core::num::NonZeroUsize;

use crate::interfaces::{PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb};
use crate::routing::dedup::PacketHash;

pub(crate) trait PacketMetricStorage {
    type Metric: Copy;

    fn capacity(&self) -> NonZeroUsize;
    fn len(&self) -> usize;
    fn append(&mut self, packet_hash: PacketHash, metric: Self::Metric);
    fn replace(&mut self, slot: usize, packet_hash: PacketHash, metric: Self::Metric);
    fn get(&self, packet_hash: PacketHash) -> Option<Self::Metric>;
}

struct PacketMetricRetention<Storage> {
    storage: Storage,
    next_evict: usize,
}

impl<Storage: PacketMetricStorage> PacketMetricRetention<Storage> {
    fn new(storage: Storage) -> Self {
        Self {
            storage,
            next_evict: 0,
        }
    }

    fn remember(&mut self, packet_hash: PacketHash, metric: Storage::Metric) {
        let capacity = self.storage.capacity().get();
        if self.storage.len() < capacity {
            self.storage.append(packet_hash, metric);
            return;
        }
        let slot = self.next_evict;
        self.storage.replace(slot, packet_hash, metric);
        self.next_evict = (self.next_evict + 1) % capacity;
    }

    fn get(&self, packet_hash: PacketHash) -> Option<Storage::Metric> {
        self.storage.get(packet_hash)
    }
}

pub(crate) struct PacketPhyRetention<RssiStorage, SnrStorage, QualityStorage> {
    rssi: PacketMetricRetention<RssiStorage>,
    snr: PacketMetricRetention<SnrStorage>,
    quality: PacketMetricRetention<QualityStorage>,
}

impl<RssiStorage, SnrStorage, QualityStorage>
    PacketPhyRetention<RssiStorage, SnrStorage, QualityStorage>
where
    RssiStorage: PacketMetricStorage<Metric = RssiDbm>,
    SnrStorage: PacketMetricStorage<Metric = SnrQuarterDb>,
    QualityStorage: PacketMetricStorage<Metric = SignalQualityTenthsPercent>,
{
    fn new(rssi: RssiStorage, snr: SnrStorage, quality: QualityStorage) -> Self {
        Self {
            rssi: PacketMetricRetention::new(rssi),
            snr: PacketMetricRetention::new(snr),
            quality: PacketMetricRetention::new(quality),
        }
    }

    pub(crate) fn remember(&mut self, packet_hash: PacketHash, stats: PacketPhyStats) {
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

    pub(crate) fn get(&self, packet_hash: PacketHash) -> Option<PacketPhyStats> {
        let stats = PacketPhyStats {
            rssi: self.rssi.get(packet_hash),
            snr: self.snr.get(packet_hash),
            quality: self.quality.get(packet_hash),
        };
        (!stats.is_empty()).then_some(stats)
    }
}

impl<RssiStorage, SnrStorage, QualityStorage> Default
    for PacketPhyRetention<RssiStorage, SnrStorage, QualityStorage>
where
    RssiStorage: Default + PacketMetricStorage<Metric = RssiDbm>,
    SnrStorage: Default + PacketMetricStorage<Metric = SnrQuarterDb>,
    QualityStorage: Default + PacketMetricStorage<Metric = SignalQualityTenthsPercent>,
{
    fn default() -> Self {
        Self::new(
            RssiStorage::default(),
            SnrStorage::default(),
            QualityStorage::default(),
        )
    }
}
