use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use crate::engine::test_support::{bytes_from_hex, RNS_1_3_5_ANNOUNCE};
use crate::engine::{ClassifiedInboundPacket, InstantMillis};
use crate::interfaces::{InboundPacket, InterfaceId, PacketPhyStats};
use crate::runtime::EmbassyInterfaceStore;

use super::retain_packet_phy;

#[test]
fn packet_phy_retention_reuses_the_classified_packet_hash() {
    const PACKET_PHY_CAPACITY: usize = 8;
    const PACKET_PHY_INDEX_BUCKETS: usize =
        crate::routing::dedup::dedup_index_buckets(PACKET_PHY_CAPACITY);

    let store = EmbassyInterfaceStore::<
        CriticalSectionRawMutex,
        8,
        PACKET_PHY_CAPACITY,
        PACKET_PHY_INDEX_BUCKETS,
    >::new();
    let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let expected = crate::routing::dedup::PacketHash::of_wire_packet(&raw)
        .expect("the fixture is a wire packet");
    let packet = ClassifiedInboundPacket::classify(InboundPacket {
        arrived_at: InstantMillis(7),
        source_interface: InterfaceId::new([0xC7; 8]),
        bytes: &mut raw,
    });
    let packet_phy = PacketPhyStats {
        rssi: Some(crate::interfaces::RssiDbm::new(-103)),
        snr: Some(crate::interfaces::SnrQuarterDb::new(-11)),
        quality: crate::interfaces::SignalQualityTenthsPercent::new(731),
    };

    retain_packet_phy(&store, &packet, packet_phy);

    assert_eq!(packet.packet_hash(), Some(expected));
    assert_eq!(store.packet_phy(expected), Some(packet_phy));
}
