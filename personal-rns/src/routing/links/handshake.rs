//! The link establishment exchange: the wire frames the initiator and responder
//! trade to bring a link up, over the [`LinkId`]/[`LinkMode`] primitives. The
//! ECDH, the [`super::LinkKey`] derivation, and the state machine compose them.

use super::{LinkId, LinkMode};
use crate::crypto::{ed25519_verify, Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::IdentitySigner;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WireError, WirePacketHeader, MTU, TRUNCATED_HASH_BYTE_LEN,
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

pub fn write_link_proof(
    link_id: &LinkId,
    responder_encryption: &X25519PublicKey,
    signer: &impl IdentitySigner,
    mtu: usize,
    mode: LinkMode,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let signalling = signalling_bytes_from(mtu, mode);
    let responder_signing = signer.signing_public_key();

    let mut signed_data = [0u8; TRUNCATED_HASH_BYTE_LEN + 32 + 32 + 3];
    let mut o = 0;
    signed_data[o..o + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(link_id.as_bytes());
    o += TRUNCATED_HASH_BYTE_LEN;
    signed_data[o..o + 32].copy_from_slice(&responder_encryption.0);
    o += 32;
    signed_data[o..o + 32].copy_from_slice(responder_signing.as_bytes());
    o += 32;
    signed_data[o..o + 3].copy_from_slice(&signalling);
    let signature = signer.sign(&signed_data);

    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Proof,
        hops: 0,
        transport_id: None,
        destination: DestinationHash::new(*link_id.as_bytes()),
        context: WireContext::LinkRequestProof,
    };
    let header_len = header.write(buf)?;
    let total = header_len + signature.0.len() + responder_encryption.0.len() + signalling.len();
    if buf.len() < total {
        return Err(WireError::BufferTooShort);
    }
    let mut offset = header_len;
    buf[offset..offset + signature.0.len()].copy_from_slice(&signature.0);
    offset += signature.0.len();
    buf[offset..offset + responder_encryption.0.len()].copy_from_slice(&responder_encryption.0);
    offset += responder_encryption.0.len();
    buf[offset..offset + signalling.len()].copy_from_slice(&signalling);
    offset += signalling.len();
    Ok(offset)
}

const LINK_PROOF_BODY_LEN: usize = 96;
const SIGNALLED_LINK_PROOF_LEN: usize = LINK_PROOF_BODY_LEN + 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkProof {
    pub link_id: LinkId,
    pub responder_encryption: X25519PublicKey,
    pub mtu: usize,
    pub mode: LinkMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkProofError {
    Malformed,
    UnsupportedMode,
    InvalidSignature,
}

pub fn validate_link_proof(
    raw: &[u8],
    responder_signing: &Ed25519PublicKey,
) -> Result<LinkProof, LinkProofError> {
    let (header, payload) = WirePacketHeader::parse(raw).map_err(|_| LinkProofError::Malformed)?;

    let (body, signalling, mtu, mode): (&[u8], &[u8], usize, LinkMode) = match payload.len() {
        LINK_PROOF_BODY_LEN => (payload, &[], MTU, LinkMode::Aes256Cbc),
        SIGNALLED_LINK_PROOF_LEN => {
            let mut bytes = [0u8; 3];
            bytes.copy_from_slice(&payload[LINK_PROOF_BODY_LEN..]);
            let (mtu, mode_bits) = decode_signalling_bytes(&bytes);
            let mode = LinkMode::from_bits(mode_bits).ok_or(LinkProofError::UnsupportedMode)?;
            (
                &payload[..LINK_PROOF_BODY_LEN],
                &payload[LINK_PROOF_BODY_LEN..],
                mtu,
                mode,
            )
        }
        _ => return Err(LinkProofError::Malformed),
    };

    let link_id = LinkId::new(*header.destination.as_bytes());
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&body[..64]);
    let mut responder = [0u8; 32];
    responder.copy_from_slice(&body[64..96]);
    let responder_encryption = X25519PublicKey(responder);

    let mut signed_data = [0u8; TRUNCATED_HASH_BYTE_LEN + 32 + 32 + 3];
    let mut o = 0;
    signed_data[o..o + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(link_id.as_bytes());
    o += TRUNCATED_HASH_BYTE_LEN;
    signed_data[o..o + 32].copy_from_slice(&responder_encryption.0);
    o += 32;
    signed_data[o..o + 32].copy_from_slice(&responder_signing.0);
    o += 32;
    signed_data[o..o + signalling.len()].copy_from_slice(signalling);
    o += signalling.len();

    ed25519_verify(
        responder_signing,
        &signed_data[..o],
        &Ed25519Signature(signature),
    )
    .map_err(|_| LinkProofError::InvalidSignature)?;

    Ok(LinkProof {
        link_id,
        responder_encryption,
        mtu,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::in_memory::InMemoryNodeIdentity;

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
    const PROOF_LINK_ID: &str = "8dcf19fbdf2597e8676bf16aede3421a";
    const RESPONDER_ENCRYPTION_PUBLIC: &str =
        "bf18d33e4d3400ea2c4307296b89dd85da180ca81b1590be97f26d34d45cc26f";
    const LINK_PROOF_PACKET: &str = "0f008dcf19fbdf2597e8676bf16aede3421aff\
                                     7f06d5f969f40b53002b1e22c47db479bcd421dc7fc79ea526b06250e358bc1c\
                                     b3fb123c9e5280a5d08e5c0ebee0b02b7ea57d3f5791a99ab69f9cf102dd5002\
                                     bf18d33e4d3400ea2c4307296b89dd85da180ca81b1590be97f26d34d45cc26f\
                                     2001f4";

    fn responder_identity() -> InMemoryNodeIdentity {
        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        InMemoryNodeIdentity::from_secret_key_bytes(&secret)
    }

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

    #[test]
    fn write_link_proof_matches_the_reference_packet() {
        let mut buf = [0u8; 128];
        let n = write_link_proof(
            &LinkId::new(a16(PROOF_LINK_ID)),
            &X25519PublicKey(a32(RESPONDER_ENCRYPTION_PUBLIC)),
            &responder_identity(),
            500,
            LinkMode::Aes256Cbc,
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], &hx(LINK_PROOF_PACKET)[..]);
    }

    #[test]
    fn write_link_proof_rejects_a_buffer_too_small_for_the_proof() {
        let mut tiny = [0u8; 40];
        assert_eq!(
            write_link_proof(
                &LinkId::new(a16(PROOF_LINK_ID)),
                &X25519PublicKey(a32(RESPONDER_ENCRYPTION_PUBLIC)),
                &responder_identity(),
                500,
                LinkMode::Aes256Cbc,
                &mut tiny,
            ),
            Err(WireError::BufferTooShort),
        );
    }

    #[test]
    fn validate_link_proof_recovers_the_responders_key() {
        let proof = validate_link_proof(
            &hx(LINK_PROOF_PACKET),
            responder_identity().signing_public_key().as_ed25519(),
        )
        .unwrap();
        assert_eq!(proof.link_id, LinkId::new(a16(PROOF_LINK_ID)));
        assert_eq!(
            proof.responder_encryption,
            X25519PublicKey(a32(RESPONDER_ENCRYPTION_PUBLIC))
        );
        assert_eq!(proof.mtu, 500);
        assert_eq!(proof.mode, LinkMode::Aes256Cbc);
    }

    #[test]
    fn a_written_proof_validates_against_its_signer() {
        let mut buf = [0u8; 128];
        let n = write_link_proof(
            &LinkId::new(a16(PROOF_LINK_ID)),
            &X25519PublicKey(a32(RESPONDER_ENCRYPTION_PUBLIC)),
            &responder_identity(),
            500,
            LinkMode::Aes256Cbc,
            &mut buf,
        )
        .unwrap();
        let proof = validate_link_proof(
            &buf[..n],
            responder_identity().signing_public_key().as_ed25519(),
        )
        .unwrap();
        assert_eq!(proof.link_id, LinkId::new(a16(PROOF_LINK_ID)));
        assert_eq!(
            proof.responder_encryption,
            X25519PublicKey(a32(RESPONDER_ENCRYPTION_PUBLIC))
        );
    }

    #[test]
    fn validate_link_proof_rejects_a_tampered_signature() {
        let mut bytes = hx(LINK_PROOF_PACKET);
        bytes[20] ^= 0x01;
        assert_eq!(
            validate_link_proof(
                &bytes,
                responder_identity().signing_public_key().as_ed25519()
            ),
            Err(LinkProofError::InvalidSignature),
        );
    }

    #[test]
    fn validate_link_proof_rejects_the_wrong_signer() {
        let other = InMemoryNodeIdentity::from_secret_key_bytes(&[0x05; 64]);
        assert_eq!(
            validate_link_proof(
                &hx(LINK_PROOF_PACKET),
                other.signing_public_key().as_ed25519()
            ),
            Err(LinkProofError::InvalidSignature),
        );
    }
}
