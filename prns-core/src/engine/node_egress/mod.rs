mod announce_selection;
mod fanout;

pub(super) use announce_selection::{
    allows_announce_rebroadcast, fleet_announce_fan_target, fleet_fan_target_reaches_any_member,
};
pub(super) use fanout::{fan_announce, fan_frame};

use crate::interfaces::InterfaceId;
use crate::routing::announce::{
    write_relayed_path_response_wire_packet, write_retransmitted_announce_wire_packet, Announce,
};
use crate::wire::{TransportId, WireError as EgressSerializeError};

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
    use crate::wire::{
        DestinationType, PacketType, PropagationType, WireContext, WirePacketHeader, HEADER_MAX_LEN,
    };

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
    use crate::wire::{
        ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
        WireContext, WirePacketHeader, HEADER_MAX_LEN,
    };

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
