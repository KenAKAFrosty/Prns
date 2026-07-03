use crate::interfaces::InterfaceId;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader, HEADER_MIN_LEN,
};

pub fn iface(byte: u8) -> InterfaceId {
    InterfaceId::new([byte; 8])
}

pub fn header_bytes(packet_type: PacketType) -> [u8; HEADER_MIN_LEN] {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type,
        hops: 0,
        transport_id: None,
        destination: DestinationHash::new([0xA5; 16]),
        context: WireContext::None,
    };
    let mut bytes = [0u8; HEADER_MIN_LEN];
    assert_eq!(header.write(&mut bytes).unwrap(), HEADER_MIN_LEN);
    bytes
}
