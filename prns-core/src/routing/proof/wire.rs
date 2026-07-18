use super::{EXPLICIT_PROOF_WIRE_LEN, IMPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN};
use crate::crypto::Ed25519Signature;
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::links::LinkId;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext, WireError,
    WirePacketHeader, HEADER_MIN_LEN,
};

/// RNS 1.3.5 `Identity.prove` in its implicit form
pub fn write_implicit_proof_wire_packet(
    packet_hash: &PacketHash,
    signature: &Ed25519Signature,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Proof,
        hops: 0,
        transport_id: None,
        address: packet_hash.proof_destination().to_address(),
        context: WireContext::None,
    };
    if buf.len() < IMPLICIT_PROOF_WIRE_LEN {
        return Err(WireError::BufferTooShort);
    }
    header.write(&mut buf[..HEADER_MIN_LEN])?;
    buf[HEADER_MIN_LEN..IMPLICIT_PROOF_WIRE_LEN].copy_from_slice(&signature.0);
    Ok(IMPLICIT_PROOF_WIRE_LEN)
}

pub fn write_explicit_proof_wire_packet(
    packet_hash: &PacketHash,
    signature: &Ed25519Signature,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Proof,
        hops: 0,
        transport_id: None,
        address: packet_hash.proof_destination().to_address(),
        context: WireContext::None,
    };
    if buf.len() < EXPLICIT_PROOF_WIRE_LEN {
        return Err(WireError::BufferTooShort);
    }
    header.write(&mut buf[..HEADER_MIN_LEN])?;
    buf[HEADER_MIN_LEN..HEADER_MIN_LEN + PACKET_HASH_LEN].copy_from_slice(packet_hash.as_bytes());
    buf[HEADER_MIN_LEN + PACKET_HASH_LEN..EXPLICIT_PROOF_WIRE_LEN].copy_from_slice(&signature.0);
    Ok(EXPLICIT_PROOF_WIRE_LEN)
}

/// Unencrypted per the reference. RNS 1.3.5 `Packet.pack` exemption ("packet proofs over links are not encrypted").
pub fn write_link_proof_wire_packet(
    link_id: &LinkId,
    packet_hash: &PacketHash,
    signature: &Ed25519Signature,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Proof,
        hops: 0,
        transport_id: None,
        address: link_id.to_address(),
        context: WireContext::None,
    };
    if buf.len() < LINK_PROOF_WIRE_LEN {
        return Err(WireError::BufferTooShort);
    }
    header.write(&mut buf[..HEADER_MIN_LEN])?;
    buf[HEADER_MIN_LEN..HEADER_MIN_LEN + PACKET_HASH_LEN].copy_from_slice(packet_hash.as_bytes());
    buf[HEADER_MIN_LEN + PACKET_HASH_LEN..LINK_PROOF_WIRE_LEN].copy_from_slice(&signature.0);
    Ok(LINK_PROOF_WIRE_LEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_link_proof_wire_packet_fills_an_exact_buffer_and_rejects_one_byte_short() {
        let link_id = LinkId::new([0x42; 16]);
        let packet_hash = PacketHash::new([0x7E; PACKET_HASH_LEN]);
        let signature = Ed25519Signature([0xC3; 64]);
        let write =
            |buf: &mut [u8]| write_link_proof_wire_packet(&link_id, &packet_hash, &signature, buf);
        let mut fits = [0u8; LINK_PROOF_WIRE_LEN];
        assert_eq!(write(&mut fits), Ok(LINK_PROOF_WIRE_LEN));
        let mut short = [0u8; LINK_PROOF_WIRE_LEN - 1];
        assert_eq!(write(&mut short), Err(WireError::BufferTooShort));
    }

    #[test]
    fn write_implicit_proof_wire_packet_fills_an_exact_buffer_and_rejects_one_byte_short() {
        let packet_hash = PacketHash::new([0x7E; PACKET_HASH_LEN]);
        let signature = Ed25519Signature([0xC3; 64]);
        let write =
            |buf: &mut [u8]| write_implicit_proof_wire_packet(&packet_hash, &signature, buf);
        let mut fits = [0u8; IMPLICIT_PROOF_WIRE_LEN];
        assert_eq!(write(&mut fits), Ok(IMPLICIT_PROOF_WIRE_LEN));
        let mut short = [0u8; IMPLICIT_PROOF_WIRE_LEN - 1];
        assert_eq!(write(&mut short), Err(WireError::BufferTooShort));
    }
}
