use crate::crypto::Ed25519Signature;
use crate::engine::proof::IMPLICIT_PROOF_WIRE_LEN;
use crate::interfaces::InterfaceId;
use crate::routing::announce::Announce;
use crate::routing::dedup::PacketHash;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WirePacketHeader, HEADER_MAX_LEN, HEADER_MIN_LEN,
    TRUNCATED_HASH_BYTE_LEN,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressSerializeError {
    BufferTooShort,
}

/// Frame an announce into a complete broadcast wire packet — header
/// (announce / single / broadcast, this `hops`, no transport id) followed by
/// the announce body — into `buf`, returning the total length written. The
/// engine owns this wire-protocol knowledge in one place: both re-emitting a
/// retained announce ([`EgressDirective::ReemitAnnounce`], with the incremented
/// hop count) and originating our own (`hops` = 0) frame through here, so the
/// two can never drift on header shape.
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

/// RNS 1.3.1 `Destination.announce(path_response=True)`
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

/// RNS 1.3.1 `Transport.jobs()` announce retransmission
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

fn frame_announce_wire_packet(
    announce: &Announce,
    hops: u8,
    propagation: PropagationType,
    transport_id: Option<TransportId>,
    context: WireContext,
    buf: &mut [u8],
) -> Result<usize, EgressSerializeError> {
    let context_flag = if announce.maybe_ratchet.is_some() {
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
        destination: announce.destination,
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

/// RNS 1.3.1 `Identity.prove` in its implicit form
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
        destination: packet_hash.proof_destination(),
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

/// The well-known plain destination every path request is addressed to,
/// `rnstransport.path.request`. A wire-protocol constant: RNS derives it once at
/// startup from the name alone (plain destinations bind to no identity), and
/// [`crate::routing::announce::derive_plain_destination_hash`] reproduces it.
pub const PATH_REQUEST_DESTINATION: DestinationHash = DestinationHash::new([
    0x6b, 0x9f, 0x66, 0x01, 0x4d, 0x98, 0x53, 0xfa, 0xab, 0x22, 0x0f, 0xba, 0x47, 0xd0, 0x27, 0x61,
]);

/// RNS 1.3.1 `Transport.request_path` payload in its leaf (non-transport) form:
/// the requested destination hash followed by a random request id, both
/// truncated-hash sized.
pub const PATH_REQUEST_PAYLOAD_LEN: usize = TRUNCATED_HASH_BYTE_LEN * 2;

/// RNS 1.3.1 `Transport.request_path`: a broadcast plain
/// DATA packet to the well-known [`PATH_REQUEST_DESTINATION`], carrying the
/// requested `destination` and a random `id` that lets the network drop
/// duplicate requests. Any reachable peer holding a path answers by
/// (re-)announcing it.
pub fn write_path_request_wire_packet(
    destination: DestinationHash,
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
        destination: PATH_REQUEST_DESTINATION,
        context: WireContext::None,
    };
    let total_len = HEADER_MIN_LEN + PATH_REQUEST_PAYLOAD_LEN;
    if buf.len() < total_len {
        return Err(EgressSerializeError::BufferTooShort);
    }
    header
        .write(&mut buf[..HEADER_MIN_LEN])
        .map_err(|_| EgressSerializeError::BufferTooShort)?;
    let payload = &mut buf[HEADER_MIN_LEN..total_len];
    payload[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(destination.as_bytes());
    payload[TRUNCATED_HASH_BYTE_LEN..].copy_from_slice(id);
    Ok(total_len)
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum EgressDirective<'a> {
    ReemitAnnounce {
        announce: Announce<'a>,
        emit_hops: u8,
        via: TransportId,
        fire_on: &'a [InterfaceId],
    },
}

impl<'a> EgressDirective<'a> {
    pub fn to_wire(&self, buf: &mut [u8]) -> Result<usize, EgressSerializeError> {
        match self {
            Self::ReemitAnnounce {
                announce,
                emit_hops,
                via,
                ..
            } => write_retransmitted_announce_wire_packet(announce, *emit_hops, *via, buf),
        }
    }

    pub fn fire_on(&self) -> &[InterfaceId] {
        match self {
            Self::ReemitAnnounce { fire_on, .. } => fire_on,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_VIA: TransportId = TransportId::new([0x7A; 16]);

    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    // Minted from RNS 1.3.1: a leaf path request for destination [0x22; 16] with
    // id [0xAB; 16] — `08 00 <dest:6b9f…2761> 00 <requested:22…> <id:ab…>`.
    const RNS_1_3_1_PATH_REQUEST: &str = "08006b9f66014d9853faab220fba47d02761002222222222\
                                          2222222222222222222222abababababababababababababababab";

    #[test]
    fn path_request_destination_matches_rns_1_3_1() {
        assert_eq!(
            PATH_REQUEST_DESTINATION,
            DestinationHash::new(hx("6b9f66014d9853faab220fba47d02761").try_into().unwrap()),
        );
    }

    #[test]
    fn write_path_request_reproduces_the_rns_1_3_1_wire() {
        let mut buf = [0u8; HEADER_MIN_LEN + PATH_REQUEST_PAYLOAD_LEN];
        let n =
            write_path_request_wire_packet(DestinationHash::new([0x22; 16]), &[0xAB; 16], &mut buf)
                .unwrap();
        assert_eq!(&buf[..n], hx(RNS_1_3_1_PATH_REQUEST).as_slice());
    }

    #[test]
    fn write_path_request_into_a_short_buffer_is_rejected() {
        let mut tiny = [0u8; HEADER_MIN_LEN + PATH_REQUEST_PAYLOAD_LEN - 1];
        assert_eq!(
            write_path_request_wire_packet(
                DestinationHash::new([0x22; 16]),
                &[0xAB; 16],
                &mut tiny
            ),
            Err(EgressSerializeError::BufferTooShort),
        );
    }

    #[test]
    fn a_path_response_is_a_normal_announce_with_only_the_context_byte_flipped() {
        let raw = hx(RAW_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();

        let mut normal = [0u8; 500];
        let n = write_announce_wire_packet(&announce, 0, &mut normal).unwrap();
        let mut response = [0u8; 500];
        let m = write_path_response_announce_wire_packet(&announce, 0, &mut response).unwrap();
        assert_eq!(n, m);

        // The context byte is the last of a type-1 header.
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
        let raw = hx(RAW_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&orig_header, orig_payload).unwrap();
        let targets = [iface(0xAA), iface(0xBB)];

        let directive = EgressDirective::ReemitAnnounce {
            announce,
            emit_hops: orig_header.hops + 1,
            via: TEST_VIA,
            fire_on: &targets,
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
        assert_eq!(parsed_header.destination, orig_header.destination);
        assert_eq!(parsed_payload, orig_payload);
    }

    #[test]
    fn to_wire_with_buffer_too_short_returns_buffer_too_short() {
        let raw = hx(RAW_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&orig_header, orig_payload).unwrap();
        let targets = [iface(0xAB)];

        let directive = EgressDirective::ReemitAnnounce {
            announce,
            emit_hops: 1,
            via: TEST_VIA,
            fire_on: &targets,
        };

        let mut tiny_buf = [0u8; 8];
        assert!(matches!(
            directive.to_wire(&mut tiny_buf),
            Err(EgressSerializeError::BufferTooShort)
        ));
    }

    #[test]
    fn to_wire_with_exactly_sized_buffer_succeeds() {
        let raw = hx(RAW_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&orig_header, orig_payload).unwrap();
        let exact_len = HEADER_MAX_LEN + announce.wire_len();
        let targets = [iface(0xAC)];

        let directive = EgressDirective::ReemitAnnounce {
            announce,
            emit_hops: 9,
            via: TEST_VIA,
            fire_on: &targets,
        };

        let mut exact_buf = std::vec![0u8; exact_len];
        let written = directive.to_wire(&mut exact_buf).unwrap();
        assert_eq!(written, exact_len);

        let (header, payload) = WirePacketHeader::parse(&exact_buf).unwrap();
        assert_eq!(header.hops, 9);
        assert_eq!(header.destination, orig_header.destination);
        assert_eq!(payload, orig_payload);
    }

    #[test]
    fn fire_on_accessor_returns_the_engine_supplied_targets() {
        let raw = hx(RAW_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();
        let targets = [iface(0xCD), iface(0xEF)];

        let directive = EgressDirective::ReemitAnnounce {
            announce,
            emit_hops: header.hops + 1,
            via: TEST_VIA,
            fire_on: &targets,
        };

        assert_eq!(directive.fire_on(), &targets);
    }

    #[test]
    fn to_wire_output_round_trips_to_an_equivalent_announce() {
        let raw = hx(RAW_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let orig_announce = Announce::from_wire(&orig_header, orig_payload).unwrap();
        let targets = [iface(0x42)];

        let directive = EgressDirective::ReemitAnnounce {
            announce: orig_announce.clone(),
            emit_hops: 5,
            via: TEST_VIA,
            fire_on: &targets,
        };

        let mut buf = [0u8; 500];
        let n = directive.to_wire(&mut buf).unwrap();
        let (re_header, re_payload) = WirePacketHeader::parse(&buf[..n]).unwrap();
        let re_announce = Announce::from_wire(&re_header, re_payload).unwrap();

        assert_eq!(re_header.hops, 5);
        assert_eq!(re_announce, orig_announce);
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
    const EXACT_REEMIT_LEN: usize = HEADER_MIN_LEN + ANNOUNCE_WIRE_LEN;
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
            maybe_ratchet: None,
            signature: Ed25519Signature(kani::any()),
            app_data: &APP_DATA,
        }
    }

    #[kani::proof]
    fn reemit_announce_exact_buffer_serializes_header_and_payload_length() {
        let announce = arbitrary_announce();
        let emit_hops: u8 = kani::any();
        let targets = [InterfaceId::new(kani::any()), InterfaceId::new(kani::any())];
        let directive = EgressDirective::ReemitAnnounce {
            announce: announce.clone(),
            emit_hops,
            via: TEST_VIA,
            fire_on: &targets,
        };

        let mut buf = [0u8; EXACT_REEMIT_LEN];
        let written = directive.to_wire(&mut buf).unwrap();
        assert_eq!(written, EXACT_REEMIT_LEN);

        let (header, payload) = WirePacketHeader::parse(&buf).unwrap();
        assert_eq!(header.ifac_flag, IfacFlag::Open);
        assert_eq!(header.context_flag, ContextFlag::Unset);
        assert_eq!(header.propagation, PropagationType::Broadcast);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.hops, emit_hops);
        assert_eq!(header.transport_id, None);
        assert_eq!(header.destination, announce.destination);
        assert_eq!(header.context, Context::None);
        assert_eq!(payload.len(), ANNOUNCE_WIRE_LEN);
        assert_eq!(directive.fire_on(), &targets);
    }

    #[kani::proof]
    fn reemit_announce_short_buffer_rejects_before_a_full_packet_is_written() {
        let announce = arbitrary_announce();
        let targets = [InterfaceId::new(kani::any())];
        let directive = EgressDirective::ReemitAnnounce {
            announce,
            emit_hops: kani::any(),
            via: TEST_VIA,
            fire_on: &targets,
        };

        let mut buf = [0u8; EXACT_REEMIT_LEN - 1];
        assert_eq!(
            directive.to_wire(&mut buf),
            Err(EgressSerializeError::BufferTooShort)
        );
    }
}
