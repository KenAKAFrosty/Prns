//! The link establishment exchange: the wire frames the initiator and responder
//! trade to bring a link up, over the [`LinkId`]/[`LinkMode`] primitives. The
//! ECDH, the [`super::LinkKey`] derivation, and the state machine compose them.

use super::{LinkId, LinkMode};
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WireError, WirePacketHeader, MTU,
};

const LINK_MTU_BYTEMASK: u32 = 0x1F_FFFF;

/// RNS `Link.signalling_bytes`: the negotiated MTU (low 21 bits) and mode (top
/// 3 bits) packed big-endian into 3 bytes. `link_id` excludes these, so a relay
/// may clamp the MTU without moving the id.
pub fn signalling_bytes_from(mtu: usize, mode: LinkMode) -> [u8; 3] {
    let value = ((mtu as u32) & LINK_MTU_BYTEMASK) | ((mode.to_bits() as u32) << 21);
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

pub fn write_link_request(
    destination: &DestinationHash,
    initiator_encryption: &X25519PublicKey,
    initiator_signing: &Ed25519PublicKey,
    mtu: usize,
    mode: LinkMode,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::LinkRequest,
        hops: 0,
        transport_id: None,
        destination: *destination,
        context: WireContext::None,
    };
    let header_len = header.write(buf)?;
    let encryption = &initiator_encryption.0;
    let signing = &initiator_signing.0;
    let signalling = signalling_bytes_from(mtu, mode);
    let total = header_len + encryption.len() + signing.len() + signalling.len();
    if buf.len() < total {
        return Err(WireError::BufferTooShort);
    }
    let mut offset = header_len;
    buf[offset..offset + encryption.len()].copy_from_slice(encryption);
    offset += encryption.len();
    buf[offset..offset + signing.len()].copy_from_slice(signing);
    offset += signing.len();
    buf[offset..offset + signalling.len()].copy_from_slice(&signalling);
    offset += signalling.len();
    Ok(offset)
}

const LINK_REQUEST_KEYS_LEN: usize = 64;
const SIGNALLED_LINK_REQUEST_LEN: usize = LINK_REQUEST_KEYS_LEN + 3;

fn decode_signalling_bytes(bytes: &[u8; 3]) -> (usize, u8) {
    let value = ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | (bytes[2] as u32);
    let mtu = (value & LINK_MTU_BYTEMASK) as usize;
    let mode_bits = ((value >> 21) & 0x07) as u8;
    (mtu, mode_bits)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRequest {
    pub destination: DestinationHash,
    pub link_id: LinkId,
    pub initiator_encryption: X25519PublicKey,
    pub initiator_signing: Ed25519PublicKey,
    pub mtu: usize,
    pub mode: LinkMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRequestError {
    Malformed,
    UnsupportedMode,
}

pub fn parse_link_request(raw: &[u8]) -> Result<LinkRequest, LinkRequestError> {
    let (header, payload) =
        WirePacketHeader::parse(raw).map_err(|_| LinkRequestError::Malformed)?;

    let (keys, mtu, mode): (&[u8], usize, LinkMode) = match payload.len() {
        LINK_REQUEST_KEYS_LEN => (payload, MTU, LinkMode::Aes256Cbc),
        SIGNALLED_LINK_REQUEST_LEN => {
            let mut signalling = [0u8; 3];
            signalling.copy_from_slice(&payload[LINK_REQUEST_KEYS_LEN..]);
            let (mtu, mode_bits) = decode_signalling_bytes(&signalling);
            let mode = LinkMode::from_bits(mode_bits).ok_or(LinkRequestError::UnsupportedMode)?;
            (&payload[..LINK_REQUEST_KEYS_LEN], mtu, mode)
        }
        _ => return Err(LinkRequestError::Malformed),
    };

    let mut encryption = [0u8; 32];
    encryption.copy_from_slice(&keys[..32]);
    let mut signing = [0u8; 32];
    signing.copy_from_slice(&keys[32..64]);
    let initiator_encryption = X25519PublicKey(encryption);
    let initiator_signing = Ed25519PublicKey(signing);

    Ok(LinkRequest {
        destination: header.destination,
        link_id: LinkId::derive(
            &header.destination,
            &initiator_encryption,
            &initiator_signing,
        ),
        initiator_encryption,
        initiator_signing,
        mtu,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }
    fn a16(s: &str) -> [u8; 16] {
        hx(s).try_into().expect("16 bytes")
    }
    fn a32(s: &str) -> [u8; 32] {
        hx(s).try_into().expect("32 bytes")
    }

    const LINK_DEST: &str = "50de0d856ad9ed3541af6d506e14d26f";
    const INITIATOR_ENCRYPTION_PUBLIC: &str =
        "a0a1a2a3a4a5a6a7a8a9aaabacadaeafa0a1a2a3a4a5a6a7a8a9aaabacadaeaf";
    const INITIATOR_SIGNING_PUBLIC: &str =
        "505152535455565758595a5b5c5d5e5f505152535455565758595a5b5c5d5e5f";
    const REQUEST_LINK_ID: &str = "6923ae567bd1dba8db3f4b8d34f894e5";
    const REQUEST_PACKET: &str = "020050de0d856ad9ed3541af6d506e14d26f00\
                                  a0a1a2a3a4a5a6a7a8a9aaabacadaeafa0a1a2a3a4a5a6a7a8a9aaabacadaeaf\
                                  505152535455565758595a5b5c5d5e5f505152535455565758595a5b5c5d5e5f\
                                  2001f4";

    #[test]
    fn link_id_matches_the_reference_request_derivation() {
        let id = LinkId::derive(
            &DestinationHash::new(a16(LINK_DEST)),
            &X25519PublicKey(a32(INITIATOR_ENCRYPTION_PUBLIC)),
            &Ed25519PublicKey(a32(INITIATOR_SIGNING_PUBLIC)),
        );
        assert_eq!(id, LinkId::new(a16(REQUEST_LINK_ID)));
    }

    #[test]
    fn signalling_bytes_match_the_reference_codec() {
        assert_eq!(
            signalling_bytes_from(500, LinkMode::Aes256Cbc),
            [0x20, 0x01, 0xf4]
        );
        assert_eq!(
            signalling_bytes_from(1064, LinkMode::Aes256Cbc),
            [0x20, 0x04, 0x28]
        );
        assert_eq!(
            signalling_bytes_from(262143, LinkMode::Aes256Cbc),
            [0x23, 0xff, 0xff]
        );
        assert_eq!(
            signalling_bytes_from(1, LinkMode::Aes256Cbc),
            [0x20, 0x00, 0x01]
        );
    }

    #[test]
    fn write_link_request_matches_a_reference_packet() {
        let mut buf = [0u8; 128];
        let n = write_link_request(
            &DestinationHash::new(a16(LINK_DEST)),
            &X25519PublicKey(a32(INITIATOR_ENCRYPTION_PUBLIC)),
            &Ed25519PublicKey(a32(INITIATOR_SIGNING_PUBLIC)),
            500,
            LinkMode::Aes256Cbc,
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], &hx(REQUEST_PACKET)[..]);
    }

    #[test]
    fn write_link_request_rejects_a_buffer_too_small_for_the_payload() {
        let mut tiny = [0u8; 40];
        assert_eq!(
            write_link_request(
                &DestinationHash::new(a16(LINK_DEST)),
                &X25519PublicKey(a32(INITIATOR_ENCRYPTION_PUBLIC)),
                &Ed25519PublicKey(a32(INITIATOR_SIGNING_PUBLIC)),
                500,
                LinkMode::Aes256Cbc,
                &mut tiny,
            ),
            Err(WireError::BufferTooShort),
        );
    }

    #[test]
    fn parse_link_request_recovers_the_initiators_request() {
        let parsed = parse_link_request(&hx(REQUEST_PACKET)).unwrap();
        assert_eq!(parsed.destination, DestinationHash::new(a16(LINK_DEST)));
        assert_eq!(parsed.link_id, LinkId::new(a16(REQUEST_LINK_ID)));
        assert_eq!(
            parsed.initiator_encryption,
            X25519PublicKey(a32(INITIATOR_ENCRYPTION_PUBLIC))
        );
        assert_eq!(
            parsed.initiator_signing,
            Ed25519PublicKey(a32(INITIATOR_SIGNING_PUBLIC))
        );
        assert_eq!(parsed.mtu, 500);
        assert_eq!(parsed.mode, LinkMode::Aes256Cbc);
    }

    #[test]
    fn parse_link_request_without_signalling_defaults_mtu_and_mode() {
        let bytes = hx(REQUEST_PACKET);
        let parsed = parse_link_request(&bytes[..bytes.len() - 3]).unwrap();
        assert_eq!(parsed.mtu, MTU);
        assert_eq!(parsed.mode, LinkMode::Aes256Cbc);
        assert_eq!(
            parsed.link_id,
            LinkId::new(a16(REQUEST_LINK_ID)),
            "the link_id excludes signalling, so it is identical with or without it",
        );
    }

    #[test]
    fn parse_link_request_rejects_a_wrong_length_payload() {
        let bytes = hx(REQUEST_PACKET);
        assert_eq!(
            parse_link_request(&bytes[..50]),
            Err(LinkRequestError::Malformed)
        );
    }

    #[test]
    fn parse_link_request_rejects_an_unsupported_mode() {
        let mut bytes = hx(REQUEST_PACKET);
        let n = bytes.len();
        bytes[n - 3..].copy_from_slice(&[0x40, 0x01, 0xf4]);
        assert_eq!(
            parse_link_request(&bytes),
            Err(LinkRequestError::UnsupportedMode)
        );
    }

    #[test]
    fn signalling_round_trips_through_decode() {
        for mtu in [1usize, 500, 1064, 262143] {
            let (decoded_mtu, mode_bits) =
                decode_signalling_bytes(&signalling_bytes_from(mtu, LinkMode::Aes256Cbc));
            assert_eq!(decoded_mtu, mtu);
            assert_eq!(LinkMode::from_bits(mode_bits), Some(LinkMode::Aes256Cbc));
        }
    }
}
