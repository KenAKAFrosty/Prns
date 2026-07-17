mod core;
mod impls;

#[cfg(feature = "tokio-host")]
pub(super) use impls::heap::HeapPacketPhyRetention;

#[cfg(feature = "embassy-host")]
pub(super) use impls::fixed::{fixed_packet_phy_retention, FixedPacketPhyRetention};

#[cfg(test)]
mod tests {
    use super::core::{PacketMetricStorage, PacketPhyRetention};
    use super::impls::fixed::{fixed_packet_phy_retention, FixedPacketPhyRetention};
    #[cfg(feature = "tokio-host")]
    use super::impls::heap::{HeapPacketPhyRetention, RNS_1_3_8_PACKET_PHY_CAPACITY};
    use crate::interfaces::{PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb};
    use crate::routing::dedup::{dedup_index_buckets, PacketHash};

    const FIXED_CAPACITY: usize = 8;
    const FIXED_BUCKETS: usize = dedup_index_buckets(FIXED_CAPACITY);

    fn packet_hash(value: u16) -> PacketHash {
        let mut bytes = [0; 32];
        bytes[..2].copy_from_slice(&value.to_le_bytes());
        PacketHash::new(bytes)
    }

    fn assert_retention_contract<RssiStorage, SnrStorage, QualityStorage>(
        mut new_retention: impl FnMut() -> PacketPhyRetention<RssiStorage, SnrStorage, QualityStorage>,
        capacity: usize,
    ) where
        RssiStorage: PacketMetricStorage<Metric = RssiDbm>,
        SnrStorage: PacketMetricStorage<Metric = SnrQuarterDb>,
        QualityStorage: PacketMetricStorage<Metric = SignalQualityTenthsPercent>,
    {
        let mut retention = new_retention();
        let partial = packet_hash(7);
        retention.remember(
            partial,
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-87)),
                snr: None,
                quality: None,
            },
        );
        retention.remember(
            partial,
            PacketPhyStats {
                rssi: None,
                snr: Some(SnrQuarterDb::new(-9)),
                quality: SignalQualityTenthsPercent::new(875),
            },
        );
        assert_eq!(
            retention.get(partial),
            Some(PacketPhyStats {
                rssi: Some(RssiDbm::new(-87)),
                snr: Some(SnrQuarterDb::new(-9)),
                quality: SignalQualityTenthsPercent::new(875),
            })
        );

        let capacity = capacity as u16;
        let mut retention = new_retention();
        for value in 0..=capacity {
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
            retention.get(packet_hash(capacity)),
            Some(PacketPhyStats {
                rssi: Some(RssiDbm::new(capacity as i16)),
                snr: None,
                quality: None,
            })
        );

        let mut retention = new_retention();
        let repeated = packet_hash(7);
        retention.remember(
            repeated,
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-90)),
                snr: None,
                quality: None,
            },
        );
        retention.remember(
            repeated,
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-80)),
                snr: None,
                quality: None,
            },
        );
        for value in 1_000..1_000 + capacity - 2 {
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
            retention.get(repeated).and_then(|stats| stats.rssi),
            Some(RssiDbm::new(-90))
        );
        retention.remember(
            packet_hash(2_000),
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-60)),
                snr: None,
                quality: None,
            },
        );
        assert_eq!(
            retention.get(repeated).and_then(|stats| stats.rssi),
            Some(RssiDbm::new(-80))
        );

        let mut retention = new_retention();
        let retained_snr = packet_hash(7);
        retention.remember(
            retained_snr,
            PacketPhyStats {
                rssi: None,
                snr: Some(SnrQuarterDb::new(-9)),
                quality: None,
            },
        );
        for value in 1_000..=1_000 + capacity {
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

    #[cfg(feature = "tokio-host")]
    #[test]
    fn heap_backend_obeys_the_shared_retention_contract() {
        assert_retention_contract(
            HeapPacketPhyRetention::default,
            RNS_1_3_8_PACKET_PHY_CAPACITY,
        );
    }

    #[test]
    fn fixed_backend_obeys_the_shared_retention_contract() {
        let new_retention = fixed_packet_phy_retention::<FIXED_CAPACITY, FIXED_BUCKETS>;
        let _: FixedPacketPhyRetention<FIXED_CAPACITY, FIXED_BUCKETS> = new_retention();
        assert_retention_contract(new_retention, FIXED_CAPACITY);
    }
}
