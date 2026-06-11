use crate::routing::links::{LinkId, LinkKey};
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader, BROADCAST_MTU, HEADER_MIN_LEN, IFAC_MIN_LEN,
};

/// RNS 1.3.1 `Identity.TOKEN_OVERHEAD`: the 16-byte IV and 32-byte HMAC around
/// every sealed link payload.
pub const LINK_TOKEN_OVERHEAD: usize = 48;

/// RNS 1.3.1 `Link.update_mdu`: the most plaintext one link data packet can
/// carry: the link MTU less the type-1 header, minimum IFAC, and token
/// overhead, floored to a whole AES block, minus one pad byte.
pub const fn link_mdu(mtu: usize) -> usize {
    ((mtu - IFAC_MIN_LEN - HEADER_MIN_LEN - LINK_TOKEN_OVERHEAD) / 16) * 16 - 1
}

pub const LINK_MDU: usize = link_mdu(BROADCAST_MTU);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkDataError {
    PayloadTooLong,
    BufferTooShort,
}

pub fn write_link_data(
    link_id: &LinkId,
    link_key: &LinkKey,
    plaintext: &[u8],
    iv: &[u8; 16],
    buf: &mut [u8],
) -> Result<usize, LinkDataError> {
    if plaintext.len() > LINK_MDU {
        return Err(LinkDataError::PayloadTooLong);
    }
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        destination: DestinationHash::new(*link_id.as_bytes()),
        context: WireContext::None,
    };
    let header_len = header
        .write(buf)
        .map_err(|_| LinkDataError::BufferTooShort)?;
    let sealed = link_key
        .seal(iv, plaintext, &mut buf[header_len..])
        .map_err(|_| LinkDataError::BufferTooShort)?;
    Ok(header_len + sealed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};

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

    const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    const INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    const RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";
    const CIPHER_IV: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0";
    const PLAINTEXT: &[u8] = b"link layer rides the same token!";
    const LINK_DATA_PACKET: &str = "0c00000102030405060708090a0b0c0d0e0f00\
                                    a1a2a3a4a5a6a7a8a9aaabacadaeafb012a31f7217fde987fbb8bab1ef73d3b3\
                                    b63557757d0c3adea6b0e94e9d27f23ba732763cc4ed566de7c915bafe3e5467\
                                    99a834e0e6579c62ccb6da661641040a56430127964af6eafdae462cd79e8ff0";

    fn link_key() -> LinkKey {
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new(a32(INITIATOR_SCALAR)),
            &X25519PublicKey(a32(RESPONDER_PUBLIC)),
        );
        LinkKey::derive(&LinkId::new(a16(LINK_ID)), &shared)
    }

    #[test]
    fn the_link_mdu_matches_the_reference_arithmetic() {
        assert_eq!(LINK_MDU, 431);
        assert_eq!(link_mdu(1_064), 991);
    }

    #[test]
    fn write_link_data_frames_the_reference_token_behind_the_data_header() {
        let mut buf = [0u8; BROADCAST_MTU];
        let n = write_link_data(
            &LinkId::new(a16(LINK_ID)),
            &link_key(),
            PLAINTEXT,
            &a16(CIPHER_IV),
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], &hx(LINK_DATA_PACKET)[..]);
    }

    #[test]
    fn a_sealed_frame_opens_in_place_to_the_plaintext() {
        let key = link_key();
        let mut buf = [0u8; BROADCAST_MTU];
        let n = write_link_data(
            &LinkId::new(a16(LINK_ID)),
            &key,
            PLAINTEXT,
            &a16(CIPHER_IV),
            &mut buf,
        )
        .unwrap();
        let (header, _) = WirePacketHeader::parse(&buf[..n]).unwrap();
        assert_eq!(header.destination, DestinationHash::new(a16(LINK_ID)));
        assert_eq!(header.context, WireContext::None);

        let opened = key.open_in_place(&mut buf[HEADER_MIN_LEN..n]).unwrap();
        assert_eq!(opened, PLAINTEXT);
    }

    #[test]
    fn a_payload_past_the_link_mdu_is_refused() {
        let mut buf = [0u8; 1_024];
        assert_eq!(
            write_link_data(
                &LinkId::new(a16(LINK_ID)),
                &link_key(),
                &[0u8; LINK_MDU + 1],
                &a16(CIPHER_IV),
                &mut buf,
            ),
            Err(LinkDataError::PayloadTooLong),
        );
        assert!(write_link_data(
            &LinkId::new(a16(LINK_ID)),
            &link_key(),
            &[0u8; LINK_MDU],
            &a16(CIPHER_IV),
            &mut buf,
        )
        .is_ok());
    }
}
