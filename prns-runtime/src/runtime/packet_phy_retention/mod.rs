mod core;
mod impls;

pub(super) use impls::heap::HeapPacketPhyRetention;

#[cfg(test)]
mod tests {
    use super::impls::heap::{HeapPacketPhyRetention, RNS_1_3_8_PACKET_PHY_CAPACITY};
    use crate::interfaces::{PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb};
    use crate::routing::dedup::PacketHash;

    fn packet_hash(value: u16) -> PacketHash {
        let mut bytes = [0; 32];
        bytes[..2].copy_from_slice(&value.to_le_bytes());
        PacketHash::new(bytes)
    }

    fn retention() -> HeapPacketPhyRetention {
        HeapPacketPhyRetention::default()
    }

    #[test]
    fn partial_packet_phy_observations_share_one_query() {
        let mut retention = retention();
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
        let mut retention = retention();
        for value in 0..=RNS_1_3_8_PACKET_PHY_CAPACITY as u16 {
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
            retention.get(packet_hash(RNS_1_3_8_PACKET_PHY_CAPACITY as u16)),
            Some(PacketPhyStats {
                rssi: Some(RssiDbm::new(RNS_1_3_8_PACKET_PHY_CAPACITY as i16)),
                snr: None,
                quality: None,
            })
        );
    }

    #[test]
    fn duplicate_packet_hash_advances_to_the_next_observation_after_fifo_eviction() {
        let mut retention = retention();
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
        for value in 1_000..1_000 + RNS_1_3_8_PACKET_PHY_CAPACITY as u16 - 2 {
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
    }

    #[test]
    fn each_packet_phy_metric_has_its_own_rns_sized_retention_window() {
        let mut retention = retention();
        let retained_snr = packet_hash(7);
        retention.remember(
            retained_snr,
            PacketPhyStats {
                rssi: None,
                snr: Some(SnrQuarterDb::new(-9)),
                quality: None,
            },
        );
        for value in 1_000..=1_000 + RNS_1_3_8_PACKET_PHY_CAPACITY as u16 {
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
