use super::Announce;
use crate::crypto::Ed25519Signature;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, TransportId, WireContext,
    WireError, WirePacketHeader, HEADER_MAX_LEN, HEADER_MIN_LEN,
};

pub(crate) fn write_originated_announce_from_signed_material(
    destination: crate::wire::DestinationHash,
    has_ratchet: bool,
    path_response: bool,
    signed_material: &[u8],
    fields_before_signature: usize,
    signature: &Ed25519Signature,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    const DESTINATION_BYTES: usize = crate::wire::TRUNCATED_HASH_BYTE_LEN;
    let Some(payload_without_signature) = signed_material.get(DESTINATION_BYTES..) else {
        return Err(WireError::BufferTooShort);
    };
    let Some((fields, app_data)) =
        payload_without_signature.split_at_checked(fields_before_signature)
    else {
        return Err(WireError::BufferTooShort);
    };
    let total_len = HEADER_MIN_LEN + fields.len() + signature.0.len() + app_data.len();
    if buf.len() < total_len {
        return Err(WireError::BufferTooShort);
    }
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: if has_ratchet {
            ContextFlag::Set
        } else {
            ContextFlag::Unset
        },
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Announce,
        hops: 0,
        transport_id: None,
        address: destination.to_address(),
        context: if path_response {
            WireContext::PathResponse
        } else {
            WireContext::None
        },
    };
    header.write(&mut buf[..HEADER_MIN_LEN])?;
    let mut offset = HEADER_MIN_LEN;
    buf[offset..offset + fields.len()].copy_from_slice(fields);
    offset += fields.len();
    buf[offset..offset + signature.0.len()].copy_from_slice(&signature.0);
    offset += signature.0.len();
    buf[offset..offset + app_data.len()].copy_from_slice(app_data);
    Ok(total_len)
}

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

/// RNS 1.4.2 `Destination.announce(path_response=True)`
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

/// RNS 1.4.2 `Transport.jobs()` announce retransmission
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
    let total_len = header_len + announce.wire_bytes();
    if buf.len() < total_len {
        return Err(WireError::BufferTooShort);
    }
    header.write(&mut buf[..header_len])?;
    announce
        .to_wire(&mut buf[header_len..])
        .map_err(|_| WireError::BufferTooShort)?;
    Ok(total_len)
}
