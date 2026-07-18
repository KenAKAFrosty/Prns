use crate::engine::ClassifiedInboundPacket;
use crate::interfaces::PacketPhyStats;
use crate::runtime::InterfaceInspectionStore;

pub(super) fn retain_packet_phy<Store: InterfaceInspectionStore>(
    store: &Store,
    packet: &ClassifiedInboundPacket<'_>,
    packet_phy: PacketPhyStats,
) {
    if !Store::RETAINS_PACKET_PHY || packet_phy.is_empty() {
        return;
    }
    if let Some(packet_hash) = packet.packet_hash() {
        store.remember_packet_phy(packet_hash, packet_phy);
    }
}
