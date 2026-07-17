use crate::crypto::Ed25519Signature;
use crate::engine::FanTarget;
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::{InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceMode};
use crate::routing::announce::Announce;
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::links::LinkId;
use crate::routing::proof::{
    EXPLICIT_PROOF_WIRE_LEN, IMPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN,
};
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WirePacketHeader, HEADER_MAX_LEN, HEADER_MIN_LEN,
    TRUNCATED_HASH_BYTE_LEN,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressSerializeError {
    BufferTooShort,
}

pub fn write_announce_wire_packet(
    announce: &Announce,
    hops: u8,
    buf: &mut [u8],
) -> Result<usize, EgressSerializeError> {
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
) -> Result<usize, EgressSerializeError> {
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
) -> Result<usize, EgressSerializeError> {
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
) -> Result<usize, EgressSerializeError> {
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
) -> Result<usize, EgressSerializeError> {
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
        return Err(EgressSerializeError::BufferTooShort);
    }
    header
        .write(&mut buf[..header_len])
        .map_err(|_| EgressSerializeError::BufferTooShort)?;
    announce
        .to_wire(&mut buf[header_len..])
        .map_err(|_| EgressSerializeError::BufferTooShort)?;
    Ok(total_len)
}

/// RNS 1.3.5 `Identity.prove` in its implicit form
pub fn write_implicit_proof_wire_packet(
    packet_hash: &PacketHash,
    signature: &Ed25519Signature,
    buf: &mut [u8],
) -> Result<usize, EgressSerializeError> {
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
        return Err(EgressSerializeError::BufferTooShort);
    }
    header
        .write(&mut buf[..HEADER_MIN_LEN])
        .map_err(|_| EgressSerializeError::BufferTooShort)?;
    buf[HEADER_MIN_LEN..IMPLICIT_PROOF_WIRE_LEN].copy_from_slice(&signature.0);
    Ok(IMPLICIT_PROOF_WIRE_LEN)
}

pub fn write_explicit_proof_wire_packet(
    packet_hash: &PacketHash,
    signature: &Ed25519Signature,
    buf: &mut [u8],
) -> Result<usize, EgressSerializeError> {
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
        return Err(EgressSerializeError::BufferTooShort);
    }
    header
        .write(&mut buf[..HEADER_MIN_LEN])
        .map_err(|_| EgressSerializeError::BufferTooShort)?;
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
) -> Result<usize, EgressSerializeError> {
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
        return Err(EgressSerializeError::BufferTooShort);
    }
    header
        .write(&mut buf[..HEADER_MIN_LEN])
        .map_err(|_| EgressSerializeError::BufferTooShort)?;
    buf[HEADER_MIN_LEN..HEADER_MIN_LEN + PACKET_HASH_LEN].copy_from_slice(packet_hash.as_bytes());
    buf[HEADER_MIN_LEN + PACKET_HASH_LEN..LINK_PROOF_WIRE_LEN].copy_from_slice(&signature.0);
    Ok(LINK_PROOF_WIRE_LEN)
}

/// RNS derives `rnstransport.path.request` from the name alone; [`crate::routing::announce::derive_plain_destination_hash`] reproduces that derivation.
pub const PATH_REQUEST_DESTINATION: DestinationHash = DestinationHash::new([
    0x6b, 0x9f, 0x66, 0x01, 0x4d, 0x98, 0x53, 0xfa, 0xab, 0x22, 0x0f, 0xba, 0x47, 0xd0, 0x27, 0x61,
]);

pub const PATH_REQUEST_PAYLOAD_LEN: usize = TRUNCATED_HASH_BYTE_LEN * 2;

/// RNS 1.3.5 `Transport.request_path`
pub fn write_path_request_wire_packet(
    destination: DestinationHash,
    requester_transport_id: Option<TransportId>,
    id: &[u8; TRUNCATED_HASH_BYTE_LEN],
    buf: &mut [u8],
) -> Result<usize, EgressSerializeError> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Plain,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: PATH_REQUEST_DESTINATION.to_address(),
        context: WireContext::None,
    };
    let payload_len = match requester_transport_id {
        Some(_) => PATH_REQUEST_PAYLOAD_LEN + TRUNCATED_HASH_BYTE_LEN,
        None => PATH_REQUEST_PAYLOAD_LEN,
    };
    let total_len = HEADER_MIN_LEN + payload_len;
    if buf.len() < total_len {
        return Err(EgressSerializeError::BufferTooShort);
    }
    header
        .write(&mut buf[..HEADER_MIN_LEN])
        .map_err(|_| EgressSerializeError::BufferTooShort)?;
    let payload = &mut buf[HEADER_MIN_LEN..total_len];
    payload[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(destination.as_bytes());
    let id_offset = match requester_transport_id {
        Some(via) => {
            payload[TRUNCATED_HASH_BYTE_LEN..TRUNCATED_HASH_BYTE_LEN * 2]
                .copy_from_slice(via.as_bytes());
            TRUNCATED_HASH_BYTE_LEN * 2
        }
        None => TRUNCATED_HASH_BYTE_LEN,
    };
    payload[id_offset..].copy_from_slice(id);
    Ok(total_len)
}

pub(crate) fn firable_on(
    descriptor: &InterfaceDescriptor,
    source: InterfaceId,
    next_hop_mode: Option<InterfaceMode>,
) -> bool {
    let transports = if descriptor.id == source {
        descriptor.capabilities.allows_same_interface_repeat()
    } else {
        descriptor.capabilities.allows_transport()
    };
    transports
        && mode_allows_announce_egress(
            descriptor.mode,
            next_hop_mode,
            descriptor.common.forwarding.announces_from_internal,
        )
}

/// RNS 1.3.5 `Transport.outbound` announce mode gating.
fn mode_allows_announce_egress(
    egress: InterfaceMode,
    next_hop_mode: Option<InterfaceMode>,
    announces_from_internal: bool,
) -> bool {
    use InterfaceMode::{AccessPoint, Boundary, Full, Gateway, Internal, PointToPoint, Roaming};
    if !announces_from_internal && next_hop_mode == Some(Internal) {
        return false;
    }
    match egress {
        AccessPoint => false,
        Roaming => match next_hop_mode {
            None | Some(Roaming | Boundary) => false,
            Some(Full | PointToPoint | AccessPoint | Gateway | Internal) => true,
        },
        Boundary => match next_hop_mode {
            None | Some(Roaming) => false,
            Some(Full | PointToPoint | AccessPoint | Gateway | Boundary | Internal) => true,
        },
        Internal => !matches!(next_hop_mode, Some(Boundary)),
        Full | PointToPoint | Gateway => true,
    }
}

/// A fleet is one shared medium, so the engine emits one [`crate::engine::Directive::SendAnnounceToFleet`] per fleet instead of one send per member.
/// That collapses the per-member eligibility verdicts into the single [`FanTarget`] the broadcast carries.
/// The collapse is sound because a supervisor's members are uniform: the only member-by-member difference is whether the flood's own source interface is withheld, and the fan target captures exactly that.
pub(crate) fn fleet_announce_fan_target(
    interfaces: AttachedInterfaces<'_>,
    supervisor: InterfaceKind,
    source: InterfaceId,
    directed_to: Option<InterfaceId>,
) -> FanTarget {
    if let Some(target) = directed_to {
        return FanTarget::Only(target);
    }
    if source.kind() != supervisor.member_kind() {
        return FanTarget::All;
    }
    let source_repeats = interfaces
        .iter()
        .find(|c| c.id == source)
        .is_some_and(|c| c.capabilities.allows_same_interface_repeat());
    if source_repeats {
        FanTarget::All
    } else {
        FanTarget::AllExcept(source)
    }
}

/// Whether the fan would reach at least one member of the supervisor's fleet among the attached interfaces.
/// A flood that arrived from the fleet's only member fans to everyone except that member, which is nobody.
/// The caller skips the fleet directive then, rather than spend the fleet's one shared lane delivering to no one.
pub(crate) fn fleet_fan_target_reaches_any_member(
    interfaces: AttachedInterfaces<'_>,
    supervisor: InterfaceKind,
    fan_target: FanTarget,
) -> bool {
    let Some(member_kind) = supervisor.member_kind() else {
        return false;
    };
    interfaces
        .iter()
        .filter(|descriptor| descriptor.id.kind() == Some(member_kind))
        .any(|descriptor| match fan_target {
            FanTarget::All => true,
            FanTarget::Only(target) => descriptor.id == target,
            FanTarget::AllExcept(excluded) => descriptor.id != excluded,
        })
}

#[derive(Debug)]
pub struct ReemitAnnounce<'a> {
    pub announce: Announce<'a>,
    pub emit_hops: u8,
    pub via: TransportId,
    pub target: InterfaceId,
    pub is_path_response: bool,
}

impl ReemitAnnounce<'_> {
    pub fn to_wire(&self, buf: &mut [u8]) -> Result<usize, EgressSerializeError> {
        if self.is_path_response {
            write_relayed_path_response_wire_packet(&self.announce, self.emit_hops, self.via, buf)
        } else {
            write_retransmitted_announce_wire_packet(&self.announce, self.emit_hops, self.via, buf)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fleet_flood_to_a_lone_source_member_reaches_nobody() {
        use crate::engine::test_support::routable_descriptor;

        let source = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x42, 0, 0, 0, 0, 0, 0]);
        let other = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x77, 0, 0, 0, 0, 0, 0]);

        let lone = [routable_descriptor(source)];
        assert!(
            !fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&lone),
                InterfaceKind::BluetoothAuto,
                FanTarget::AllExcept(source)
            ),
            "a flood whose fleet's only member is the source it arrived on reaches nobody"
        );

        let pair = [routable_descriptor(source), routable_descriptor(other)];
        assert!(
            fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&pair),
                InterfaceKind::BluetoothAuto,
                FanTarget::AllExcept(source)
            ),
            "with a second peer present the flood reaches it"
        );
        assert!(
            fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&lone),
                InterfaceKind::BluetoothAuto,
                FanTarget::All
            ),
            "an unconditional flood reaches the lone member"
        );
        assert!(
            !fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&[routable_descriptor(InterfaceId::new([0xFE; 8]))]),
                InterfaceKind::BluetoothAuto,
                FanTarget::All
            ),
            "a flood selects nobody when no member of the fleet's kind is attached"
        );
    }

    const TEST_VIA: TransportId = TransportId::new([0x7A; 16]);

    const RNS_1_3_5_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn bytes_from_hex(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    // Minted from RNS 1.3.5: a leaf path request for destination [0x22; 16] with id [0xAB; 16]. Wire bytes: `08 00 <dest:6b9f…2761> 00 <requested:22…> <id:ab…>`.
    const RNS_1_3_5_PATH_REQUEST: &str = "08006b9f66014d9853faab220fba47d02761002222222222\
                                          2222222222222222222222abababababababababababababababab";

    // The transport form inserts the requester's transport id between the requested destination and the id.
    const RNS_1_3_5_PATH_REQUEST_TRANSPORT: &str =
        "08006b9f66014d9853faab220fba47d027610022222222222222222222222222222222\
         7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7aabababababababababababababababab";

    #[test]
    fn path_request_destination_matches_rns_1_3_5() {
        assert_eq!(
            PATH_REQUEST_DESTINATION,
            DestinationHash::new(
                bytes_from_hex("6b9f66014d9853faab220fba47d02761")
                    .try_into()
                    .unwrap()
            ),
        );
    }

    #[test]
    fn write_path_request_reproduces_the_rns_1_3_5_wire() {
        let mut buf = [0u8; HEADER_MIN_LEN + PATH_REQUEST_PAYLOAD_LEN];
        let n = write_path_request_wire_packet(
            DestinationHash::new([0x22; 16]),
            None,
            &[0xAB; 16],
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], bytes_from_hex(RNS_1_3_5_PATH_REQUEST).as_slice());
    }

    #[test]
    fn write_transport_path_request_reproduces_the_rns_1_3_5_wire() {
        let mut buf = [0u8; HEADER_MIN_LEN + PATH_REQUEST_PAYLOAD_LEN + TRUNCATED_HASH_BYTE_LEN];
        let n = write_path_request_wire_packet(
            DestinationHash::new([0x22; 16]),
            Some(TransportId::new([0x7a; 16])),
            &[0xAB; 16],
            &mut buf,
        )
        .unwrap();
        assert_eq!(
            &buf[..n],
            bytes_from_hex(RNS_1_3_5_PATH_REQUEST_TRANSPORT).as_slice()
        );
    }

    #[test]
    fn write_path_request_into_a_short_buffer_is_rejected() {
        let mut tiny = [0u8; HEADER_MIN_LEN + PATH_REQUEST_PAYLOAD_LEN - 1];
        assert_eq!(
            write_path_request_wire_packet(
                DestinationHash::new([0x22; 16]),
                None,
                &[0xAB; 16],
                &mut tiny
            ),
            Err(EgressSerializeError::BufferTooShort),
        );
    }

    #[test]
    fn internal_mode_blocks_boundary_announces_but_accepts_internal_announces() {
        assert!(!mode_allows_announce_egress(
            InterfaceMode::Internal,
            Some(InterfaceMode::Boundary),
            true,
        ));
        assert!(mode_allows_announce_egress(
            InterfaceMode::Internal,
            Some(InterfaceMode::Internal),
            true,
        ));
    }

    #[test]
    fn announces_from_internal_can_close_the_internal_to_boundary_direction() {
        assert!(!mode_allows_announce_egress(
            InterfaceMode::Boundary,
            Some(InterfaceMode::Internal),
            false,
        ));
        assert!(mode_allows_announce_egress(
            InterfaceMode::Boundary,
            Some(InterfaceMode::Internal),
            true,
        ));
    }

    #[test]
    fn a_path_response_is_a_normal_announce_with_only_the_context_byte_flipped() {
        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();

        let mut normal = [0u8; 500];
        let n = write_announce_wire_packet(&announce, 0, &mut normal).unwrap();
        let mut response = [0u8; 500];
        let m = write_path_response_announce_wire_packet(&announce, 0, &mut response).unwrap();
        assert_eq!(n, m);

        let context_offset = HEADER_MIN_LEN - 1;
        assert_eq!(normal[context_offset], WireContext::None.to_byte());
        assert_eq!(
            response[context_offset],
            WireContext::PathResponse.to_byte()
        );

        let mut patched = response;
        patched[context_offset] = WireContext::None.to_byte();
        assert_eq!(
            &patched[..m],
            &normal[..n],
            "the only difference from a normal announce is the context byte",
        );

        let (re_header, re_payload) = WirePacketHeader::parse(&response[..m]).unwrap();
        assert_eq!(re_header.context, WireContext::PathResponse);
        assert_eq!(re_header.packet_type, PacketType::Announce);
        assert_eq!(
            Announce::from_wire(&re_header, re_payload).unwrap(),
            announce
        );
    }

    #[test]
    fn reemit_announce_to_wire_produces_a_well_formed_wire_packet() {
        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&orig_header, orig_payload).unwrap();

        let directive = ReemitAnnounce {
            announce,
            emit_hops: orig_header.hops + 1,
            via: TEST_VIA,
            target: iface(0xAA),
            is_path_response: false,
        };

        let mut buf = [0u8; 500];
        let n = directive.to_wire(&mut buf).unwrap();
        let wire = &buf[..n];

        let (parsed_header, parsed_payload) = WirePacketHeader::parse(wire).unwrap();
        assert_eq!(parsed_header.packet_type, PacketType::Announce);
        assert_eq!(parsed_header.destination_type, DestinationType::Single);
        assert_eq!(parsed_header.propagation, PropagationType::Transport);
        assert_eq!(parsed_header.transport_id, Some(TEST_VIA));
        assert_eq!(parsed_header.hops, orig_header.hops + 1);
        assert_eq!(parsed_header.address, orig_header.address);
        assert_eq!(parsed_header.context, WireContext::None);
        assert_eq!(parsed_payload, orig_payload);
    }

    #[test]
    fn a_directed_path_response_reemit_carries_the_path_response_context() {
        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&orig_header, orig_payload).unwrap();

        let directive = ReemitAnnounce {
            announce,
            emit_hops: orig_header.hops + 1,
            via: TEST_VIA,
            target: iface(0xAA),
            is_path_response: true,
        };

        let mut buf = [0u8; 500];
        let n = directive.to_wire(&mut buf).unwrap();
        let (parsed_header, parsed_payload) = WirePacketHeader::parse(&buf[..n]).unwrap();

        assert_eq!(parsed_header.context, WireContext::PathResponse);
        assert_eq!(parsed_header.propagation, PropagationType::Transport);
        assert_eq!(parsed_header.transport_id, Some(TEST_VIA));
        assert_eq!(parsed_header.packet_type, PacketType::Announce);
        assert_eq!(parsed_header.hops, orig_header.hops + 1);
        assert_eq!(parsed_header.address, orig_header.address);
        assert_eq!(parsed_payload, orig_payload);
    }

    #[test]
    fn to_wire_with_buffer_too_short_returns_buffer_too_short() {
        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&orig_header, orig_payload).unwrap();

        let directive = ReemitAnnounce {
            announce,
            emit_hops: 1,
            via: TEST_VIA,
            target: iface(0xAB),
            is_path_response: false,
        };

        let mut tiny_buf = [0u8; 8];
        assert!(matches!(
            directive.to_wire(&mut tiny_buf),
            Err(EgressSerializeError::BufferTooShort)
        ));
    }

    #[test]
    fn to_wire_with_exactly_sized_buffer_succeeds() {
        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&orig_header, orig_payload).unwrap();
        let exact_len = HEADER_MAX_LEN + announce.wire_len();

        let directive = ReemitAnnounce {
            announce,
            emit_hops: 9,
            via: TEST_VIA,
            target: iface(0xAC),
            is_path_response: false,
        };

        let mut exact_buf = std::vec![0u8; exact_len];
        let written = directive.to_wire(&mut exact_buf).unwrap();
        assert_eq!(written, exact_len);

        let (header, payload) = WirePacketHeader::parse(&exact_buf).unwrap();
        assert_eq!(header.hops, 9);
        assert_eq!(header.address, orig_header.address);
        assert_eq!(payload, orig_payload);
    }

    #[test]
    fn to_wire_output_round_trips_to_an_equivalent_announce() {
        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let orig_announce = Announce::from_wire(&orig_header, orig_payload).unwrap();

        let directive = ReemitAnnounce {
            announce: orig_announce.clone(),
            emit_hops: 5,
            via: TEST_VIA,
            target: iface(0x42),
            is_path_response: false,
        };

        let mut buf = [0u8; 500];
        let n = directive.to_wire(&mut buf).unwrap();
        let (re_header, re_payload) = WirePacketHeader::parse(&buf[..n]).unwrap();
        let re_announce = Announce::from_wire(&re_header, re_payload).unwrap();

        assert_eq!(re_header.hops, 5);
        assert_eq!(re_announce, orig_announce);
    }

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
        assert_eq!(write(&mut short), Err(EgressSerializeError::BufferTooShort));
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
        assert_eq!(write(&mut short), Err(EgressSerializeError::BufferTooShort));
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
    use crate::routing::announce::{
        AnnounceId, DottedNameHash, IdentityPublicKeys, ANNOUNCE_FIXED_FIELDS_LEN,
    };
    use crate::wire::DestinationHash;

    const APP_DATA_LEN: usize = 2;
    const ANNOUNCE_WIRE_LEN: usize = ANNOUNCE_FIXED_FIELDS_LEN + APP_DATA_LEN;
    const EXACT_REEMIT_LEN: usize = HEADER_MAX_LEN + ANNOUNCE_WIRE_LEN;
    static APP_DATA: [u8; APP_DATA_LEN] = [0xA5, 0x5A];

    fn arbitrary_announce() -> Announce<'static> {
        Announce {
            destination: DestinationHash::new(kani::any()),
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey(kani::any())),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey(kani::any())),
            },
            dotted_name_hash: DottedNameHash::new(kani::any()),
            announce_id: AnnounceId::from_wire(kani::any()),
            ratchet: None,
            signature: Ed25519Signature(kani::any()),
            app_data: &APP_DATA,
        }
    }

    #[kani::proof]
    fn reemit_announce_exact_buffer_serializes_header_and_payload_length() {
        let announce = arbitrary_announce();
        let emit_hops: u8 = kani::any();
        let via = TransportId::new(kani::any());
        let target = InterfaceId::new(kani::any());
        let directive = ReemitAnnounce {
            announce: announce.clone(),
            emit_hops,
            via,
            target,
            is_path_response: false,
        };

        let mut buf = [0u8; EXACT_REEMIT_LEN];
        let written = directive.to_wire(&mut buf).unwrap();
        assert_eq!(written, EXACT_REEMIT_LEN);

        let (header, payload) = WirePacketHeader::parse(&buf).unwrap();
        assert_eq!(header.ifac_flag, IfacFlag::Open);
        assert_eq!(header.context_flag, ContextFlag::Unset);
        assert_eq!(header.propagation, PropagationType::Transport);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.hops, emit_hops);
        assert_eq!(header.transport_id, Some(via));
        assert_eq!(
            DestinationHash::from_address(header.address),
            announce.destination
        );
        assert_eq!(header.context, WireContext::None);
        assert_eq!(payload.len(), ANNOUNCE_WIRE_LEN);
        assert_eq!(directive.target, target);
    }

    #[kani::proof]
    fn reemit_announce_short_buffer_rejects_before_a_full_packet_is_written() {
        let announce = arbitrary_announce();
        let directive = ReemitAnnounce {
            announce,
            emit_hops: kani::any(),
            via: TransportId::new(kani::any()),
            target: InterfaceId::new(kani::any()),
            is_path_response: false,
        };

        let mut buf = [0u8; EXACT_REEMIT_LEN - 1];
        assert_eq!(
            directive.to_wire(&mut buf),
            Err(EgressSerializeError::BufferTooShort)
        );
    }
}
