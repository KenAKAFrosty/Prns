use super::Announce;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, TransportId, WireContext,
    WireError, WirePacketHeader, HEADER_MAX_LEN, HEADER_MIN_LEN,
};

pub fn write_announce_wire_packet(
    announce: &Announce,
    hops: u8,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    frame_announce_wire_packet(
        announce,
        hops,
        PropagationType::Broadcast,
        None,
        WireContext::None,
        buf,
    )
}

/// RNS 1.3.5 `Destination.announce(path_response=True)`
pub fn write_path_response_announce_wire_packet(
    announce: &Announce,
    hops: u8,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    frame_announce_wire_packet(
        announce,
        hops,
        PropagationType::Broadcast,
        None,
        WireContext::PathResponse,
        buf,
    )
}

/// RNS 1.3.5 `Transport.jobs()` announce retransmission
pub fn write_retransmitted_announce_wire_packet(
    announce: &Announce,
    hops: u8,
    via: TransportId,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    frame_announce_wire_packet(
        announce,
        hops,
        PropagationType::Transport,
        Some(via),
        WireContext::None,
        buf,
    )
}

pub fn write_relayed_path_response_wire_packet(
    announce: &Announce,
    hops: u8,
    via: TransportId,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    frame_announce_wire_packet(
        announce,
        hops,
        PropagationType::Transport,
        Some(via),
        WireContext::PathResponse,
        buf,
    )
}

fn frame_announce_wire_packet(
    announce: &Announce,
    hops: u8,
    propagation: PropagationType,
    transport_id: Option<TransportId>,
    context: WireContext,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let context_flag = if announce.ratchet.is_some() {
        ContextFlag::Set
    } else {
        ContextFlag::Unset
    };
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag,
        propagation,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Announce,
        hops,
        transport_id,
        address: announce.destination.to_address(),
        context,
    };
    let header_len = if transport_id.is_some() {
        HEADER_MAX_LEN
    } else {
        HEADER_MIN_LEN
    };
    let total_len = header_len + announce.wire_len();
    if buf.len() < total_len {
        return Err(WireError::BufferTooShort);
    }
    header.write(&mut buf[..header_len])?;
    announce
        .to_wire(&mut buf[header_len..])
        .map_err(|_| WireError::BufferTooShort)?;
    Ok(total_len)
}
