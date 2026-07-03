use super::{LinkId, LinkKey, LinkMode};
use crate::crypto::{
    ed25519_verify, Ed25519PublicKey, Ed25519SecretKey, Ed25519Signature, X25519PublicKey,
    X25519SecretKey,
};
use crate::engine::commands::CommandId;
use crate::engine::InstantMillis;
use crate::identity::{IdentityHash, IdentitySigner, IdentitySigningPublicKey};
use crate::interfaces::InterfaceId;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::units::Rtt;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WireError, WirePacketHeader, BROADCAST_MTU, TRUNCATED_HASH_BYTE_LEN,
};

const LINK_MTU_BYTEMASK: u32 = 0x1F_FFFF;

/// RNS `Link.signalling_bytes`: the negotiated link MTU (low 21 bits) and mode
/// (top 3 bits) packed big-endian into 3 bytes. `link_id` excludes these, so a
/// relay may clamp the MTU without moving the id.
pub fn signalling_bytes_from(mtu: usize, mode: LinkMode) -> [u8; 3] {
    let value = ((mtu as u32) & LINK_MTU_BYTEMASK) | ((mode.to_bits() as u32) << 21);
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

pub fn write_link_request(
    destination: &DestinationHash,
    via: Option<TransportId>,
    initiator_encryption: &X25519PublicKey,
    initiator_signing: &Ed25519PublicKey,
    mtu: usize,
    mode: LinkMode,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let propagation = if via.is_some() {
        PropagationType::Transport
    } else {
        PropagationType::Broadcast
    };
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation,
        destination_type: DestinationType::Single,
        packet_type: PacketType::LinkRequest,
        hops: 0,
        transport_id: via,
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

pub const LINK_REQUEST_KEYS_LEN: usize = 64;
pub const SIGNALLED_LINK_REQUEST_LEN: usize = LINK_REQUEST_KEYS_LEN + 3;
pub const LINK_PROOF_SIGNED_DATA_LEN: usize = TRUNCATED_HASH_BYTE_LEN + 32 + 32 + 3;

fn decode_signalling_bytes(bytes: &[u8; 3]) -> (usize, u8) {
    let value = ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | (bytes[2] as u32);
    let mtu = (value & LINK_MTU_BYTEMASK) as usize;
    let mode_bits = ((value >> 21) & 0x07) as u8;
    (mtu, mode_bits)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedLinkRequest {
    pub request: LinkRequest,
    pub identity: IdentityHash,
    pub proof_strategy: ProofStrategy,
    pub received_hops: u8,
    pub arrived_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRequest {
    pub destination: DestinationHash,
    pub link_id: LinkId,
    pub initiator_encryption: X25519PublicKey,
    pub initiator_signing: Ed25519PublicKey,
    pub mtu: usize,
    pub mode: LinkMode,
    pub signalled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRequestError {
    Malformed,
    UnsupportedMode,
}

pub fn parse_link_request(raw: &[u8]) -> Result<LinkRequest, LinkRequestError> {
    let (header, payload) =
        WirePacketHeader::parse(raw).map_err(|_| LinkRequestError::Malformed)?;
    link_request_from(&header, payload)
}

pub fn link_request_from(
    header: &WirePacketHeader,
    payload: &[u8],
) -> Result<LinkRequest, LinkRequestError> {
    let (keys, mtu, mode, signalled): (&[u8], usize, LinkMode, bool) = match payload.len() {
        LINK_REQUEST_KEYS_LEN => (payload, BROADCAST_MTU, LinkMode::Aes256Cbc, false),
        SIGNALLED_LINK_REQUEST_LEN => {
            let mut signalling = [0u8; 3];
            signalling.copy_from_slice(&payload[LINK_REQUEST_KEYS_LEN..]);
            let (mtu, mode_bits) = decode_signalling_bytes(&signalling);
            let mode = LinkMode::from_bits(mode_bits).ok_or(LinkRequestError::UnsupportedMode)?;
            (&payload[..LINK_REQUEST_KEYS_LEN], mtu, mode, true)
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
        signalled,
    })
}

/// The exact bytes a LRPROOF signs: `link_id ++ responder_encryption ++
/// responder_signing ++ signalling`; the inline signer and the pool's deferred
/// sign frame identical material.
pub fn link_proof_signed_data(
    link_id: &LinkId,
    responder_encryption: &X25519PublicKey,
    responder_signing: &Ed25519PublicKey,
    mtu: usize,
    mode: LinkMode,
) -> [u8; LINK_PROOF_SIGNED_DATA_LEN] {
    let signalling = signalling_bytes_from(mtu, mode);
    let mut signed_data = [0u8; LINK_PROOF_SIGNED_DATA_LEN];
    let mut o = 0;
    signed_data[o..o + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(link_id.as_bytes());
    o += TRUNCATED_HASH_BYTE_LEN;
    signed_data[o..o + 32].copy_from_slice(&responder_encryption.0);
    o += 32;
    signed_data[o..o + 32].copy_from_slice(&responder_signing.0);
    o += 32;
    signed_data[o..o + 3].copy_from_slice(&signalling);
    signed_data
}

/// The assembly half of [`write_link_proof`]; deferred and inline signs share one
/// wire path.
pub fn write_link_proof_from_parts(
    link_id: &LinkId,
    responder_encryption: &X25519PublicKey,
    signature: &Ed25519Signature,
    mtu: usize,
    mode: LinkMode,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let signalling = signalling_bytes_from(mtu, mode);
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

pub fn write_link_proof(
    link_id: &LinkId,
    responder_encryption: &X25519PublicKey,
    signer: &impl IdentitySigner,
    mtu: usize,
    mode: LinkMode,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let signed_data = link_proof_signed_data(
        link_id,
        responder_encryption,
        signer.signing_public_key().as_ed25519(),
        mtu,
        mode,
    );
    let signature = signer.sign(&signed_data);
    write_link_proof_from_parts(link_id, responder_encryption, &signature, mtu, mode, buf)
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
    link_proof_from(
        &LinkId::new(*header.destination.as_bytes()),
        payload,
        responder_signing,
    )
}

pub struct LinkProofParsed {
    pub proof: LinkProof,
    pub signed_data: [u8; LINK_PROOF_SIGNED_DATA_LEN],
    pub signed_len: usize,
    pub signature: Ed25519Signature,
}

/// Moves through the seam (the secret is not `Copy`); the verdict rides back as
/// the derived shared secret, so a valid proof never makes a second pool round-trip.
pub struct LinkProofVerifyOwed {
    pub link_id: LinkId,
    pub source_interface: InterfaceId,
    pub responder_encryption: X25519PublicKey,
    pub responder_signing: Ed25519PublicKey,
    pub initiator_secret: X25519SecretKey,
    pub command_id: CommandId,
    pub rtt: Rtt,
    pub mtu: usize,
    pub signed_data: [u8; LINK_PROOF_SIGNED_DATA_LEN],
    pub signed_len: usize,
    pub signature: Ed25519Signature,
}

pub fn link_proof_signature_valid(owed: &LinkProofVerifyOwed) -> bool {
    ed25519_verify(
        &owed.responder_signing,
        &owed.signed_data[..owed.signed_len],
        &owed.signature,
    )
    .is_ok()
}

/// Moves through the seam (the two secrets are not `Copy`); the pool hands back
/// the encryption pubkey, shared secret, and signature, so the reactor only
/// assembles and tracks.
pub struct LinkProofSignOwed {
    pub request: LinkRequest,
    pub identity: IdentityHash,
    pub proof_strategy: ProofStrategy,
    pub received_hops: u8,
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub mtu: usize,
    pub ephemeral_secret: X25519SecretKey,
    pub signing_secret: Ed25519SecretKey,
    pub responder_signing: IdentitySigningPublicKey,
}

pub fn link_proof_parse(
    link_id: &LinkId,
    payload: &[u8],
    responder_signing: &Ed25519PublicKey,
) -> Result<LinkProofParsed, LinkProofError> {
    let (body, signalling, mtu, mode): (&[u8], &[u8], usize, LinkMode) = match payload.len() {
        LINK_PROOF_BODY_LEN => (payload, &[], BROADCAST_MTU, LinkMode::Aes256Cbc),
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

    let mut signature = [0u8; 64];
    signature.copy_from_slice(&body[..64]);
    let mut responder = [0u8; 32];
    responder.copy_from_slice(&body[64..96]);
    let responder_encryption = X25519PublicKey(responder);

    let mut signed_data = [0u8; LINK_PROOF_SIGNED_DATA_LEN];
    let mut o = 0;
    signed_data[o..o + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(link_id.as_bytes());
    o += TRUNCATED_HASH_BYTE_LEN;
    signed_data[o..o + 32].copy_from_slice(&responder_encryption.0);
    o += 32;
    signed_data[o..o + 32].copy_from_slice(&responder_signing.0);
    o += 32;
    signed_data[o..o + signalling.len()].copy_from_slice(signalling);
    o += signalling.len();

    Ok(LinkProofParsed {
        proof: LinkProof {
            link_id: *link_id,
            responder_encryption,
            mtu,
            mode,
        },
        signed_data,
        signed_len: o,
        signature: Ed25519Signature(signature),
    })
}

pub fn link_proof_from(
    link_id: &LinkId,
    payload: &[u8],
    responder_signing: &Ed25519PublicKey,
) -> Result<LinkProof, LinkProofError> {
    let parsed = link_proof_parse(link_id, payload, responder_signing)?;
    ed25519_verify(
        responder_signing,
        &parsed.signed_data[..parsed.signed_len],
        &parsed.signature,
    )
    .map_err(|_| LinkProofError::InvalidSignature)?;
    Ok(parsed.proof)
}

const MSGPACK_FLOAT64: u8 = 0xcb;
const LINK_RTT_PLAINTEXT_LEN: usize = 9;

fn pack_rtt(rtt: Rtt) -> [u8; LINK_RTT_PLAINTEXT_LEN] {
    let mut out = [0u8; LINK_RTT_PLAINTEXT_LEN];
    out[0] = MSGPACK_FLOAT64;
    out[1..].copy_from_slice(&(rtt.millis() as f64 / 1_000.0).to_be_bytes());
    out
}

fn unpack_rtt(bytes: &[u8]) -> Option<Rtt> {
    if bytes.len() != LINK_RTT_PLAINTEXT_LEN || bytes[0] != MSGPACK_FLOAT64 {
        return None;
    }
    let mut be = [0u8; 8];
    be.copy_from_slice(&bytes[1..]);
    Some(Rtt::from_millis(
        (f64::from_be_bytes(be) * 1_000.0 + 0.5) as u64,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRttError {
    Malformed,
    InvalidToken,
    BufferTooShort,
}

pub fn write_link_rtt(
    link_id: &LinkId,
    link_key: &LinkKey,
    rtt: Rtt,
    iv: &[u8; 16],
    buf: &mut [u8],
) -> Result<usize, LinkRttError> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        destination: DestinationHash::new(*link_id.as_bytes()),
        context: WireContext::LinkRtt,
    };
    let header_len = header
        .write(buf)
        .map_err(|_| LinkRttError::BufferTooShort)?;
    let plaintext = pack_rtt(rtt);
    let sealed = link_key
        .seal(iv, &plaintext, &mut buf[header_len..])
        .map_err(|_| LinkRttError::BufferTooShort)?;
    Ok(header_len + sealed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRtt {
    pub link_id: LinkId,
    pub rtt: Rtt,
}

pub fn parse_link_rtt(raw: &[u8], link_key: &LinkKey) -> Result<LinkRtt, LinkRttError> {
    let (header, payload) = WirePacketHeader::parse(raw).map_err(|_| LinkRttError::Malformed)?;
    link_rtt_from(
        &LinkId::new(*header.destination.as_bytes()),
        payload,
        link_key,
    )
}

pub fn link_rtt_from(
    link_id: &LinkId,
    payload: &[u8],
    link_key: &LinkKey,
) -> Result<LinkRtt, LinkRttError> {
    let mut out = [0u8; 16];
    let n = link_key
        .open(payload, &mut out)
        .map_err(|_| LinkRttError::InvalidToken)?;
    let rtt = unpack_rtt(&out[..n]).ok_or(LinkRttError::Malformed)?;
    Ok(LinkRtt {
        link_id: *link_id,
        rtt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{x25519_diffie_hellman, X25519SecretKey};
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
    // Minted from RNS 1.3.5: the pre-signalling LRPROOF form (signature ‖
    // encryption key, no signalling bytes) that `Link.validate_proof` still
    // accepts from older peers — signed over link_id ‖ pub ‖ sig_pub by a
    // reference `Identity` and self-checked through `Identity.validate`
    // before pinning.
    const UNSIGNALLED_PROOF_LINK_ID: &str = "4242aa55c3e1d20f8badf00d5ca1ab1e";
    const UNSIGNALLED_RESPONDER_ENCRYPTION_PUBLIC: &str =
        "07a37cbc142093c8b755dc1b10e86cb426374ad16aa853ed0bdfc0b2b86d1c7c";
    const UNSIGNALLED_RESPONDER_SIGNING_PUBLIC: &str =
        "da29e95b02e00ffa15645775fb1d2ba222a1943395eea06b94e2c057b7be69d0";
    const UNSIGNALLED_LINK_PROOF_PACKET: &str = "0f004242aa55c3e1d20f8badf00d5ca1ab1eff\
        5b80243ce3c437a59e25ac2de5ee0c99857a83cd17548e7261f86da4511189d0\
        b536064c8a9db3f83718d0a402ead809cb4af90869607d6cc1dc822caf37990c\
        07a37cbc142093c8b755dc1b10e86cb426374ad16aa853ed0bdfc0b2b86d1c7c";
    const RTT_LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    const RTT_INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    const RTT_RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";
    const RTT_IV: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0";
    const RTT_VALUE_MS: u64 = 125;
    const LRRTT_PACKET: &str = "0c00000102030405060708090a0b0c0d0e0ffe\
                                a1a2a3a4a5a6a7a8a9aaabacadaeafb0\
                                dc2a04eab8c13d78dc9d02510d587a56\
                                b7134599d34b153468f2618d9b4893ca759fef9170eee3908949ad759ecd380a";

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
            None,
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
    fn write_link_request_fills_an_exact_buffer_and_rejects_one_byte_short() {
        let exact = hx(REQUEST_PACKET).len();
        let request = |buf: &mut [u8]| {
            write_link_request(
                &DestinationHash::new(a16(LINK_DEST)),
                None,
                &X25519PublicKey(a32(INITIATOR_ENCRYPTION_PUBLIC)),
                &Ed25519PublicKey(a32(INITIATOR_SIGNING_PUBLIC)),
                500,
                LinkMode::Aes256Cbc,
                buf,
            )
        };
        let mut fits = std::vec![0u8; exact];
        assert_eq!(request(&mut fits), Ok(exact));
        let mut short = std::vec![0u8; exact - 1];
        assert_eq!(request(&mut short), Err(WireError::BufferTooShort));
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
        assert_eq!(parsed.mtu, BROADCAST_MTU);
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
    fn write_link_proof_fills_an_exact_buffer_and_rejects_one_byte_short() {
        let exact = hx(LINK_PROOF_PACKET).len();
        let proof = |buf: &mut [u8]| {
            write_link_proof(
                &LinkId::new(a16(PROOF_LINK_ID)),
                &X25519PublicKey(a32(RESPONDER_ENCRYPTION_PUBLIC)),
                &responder_identity(),
                500,
                LinkMode::Aes256Cbc,
                buf,
            )
        };
        let mut fits = std::vec![0u8; exact];
        assert_eq!(proof(&mut fits), Ok(exact));
        let mut short = std::vec![0u8; exact - 1];
        assert_eq!(proof(&mut short), Err(WireError::BufferTooShort));
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
    fn validate_link_proof_accepts_the_reference_unsignalled_form() {
        let signing = Ed25519PublicKey(a32(UNSIGNALLED_RESPONDER_SIGNING_PUBLIC));
        let proof = validate_link_proof(&hx(UNSIGNALLED_LINK_PROOF_PACKET), &signing).unwrap();
        assert_eq!(proof.link_id, LinkId::new(a16(UNSIGNALLED_PROOF_LINK_ID)));
        assert_eq!(
            proof.responder_encryption,
            X25519PublicKey(a32(UNSIGNALLED_RESPONDER_ENCRYPTION_PUBLIC))
        );
        assert_eq!(proof.mtu, BROADCAST_MTU);
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

    fn rtt_link_key() -> LinkKey {
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new(a32(RTT_INITIATOR_SCALAR)),
            &X25519PublicKey(a32(RTT_RESPONDER_PUBLIC)),
        );
        LinkKey::derive(&LinkId::new(a16(RTT_LINK_ID)), &shared)
    }

    #[test]
    fn write_link_rtt_matches_the_reference_packet() {
        let mut buf = [0u8; 128];
        let n = write_link_rtt(
            &LinkId::new(a16(RTT_LINK_ID)),
            &rtt_link_key(),
            Rtt(RTT_VALUE_MS),
            &a16(RTT_IV),
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], &hx(LRRTT_PACKET)[..]);
    }

    #[test]
    fn parse_link_rtt_recovers_the_reference_rtt() {
        let parsed = parse_link_rtt(&hx(LRRTT_PACKET), &rtt_link_key()).unwrap();
        assert_eq!(parsed.link_id, LinkId::new(a16(RTT_LINK_ID)));
        assert_eq!(parsed.rtt, Rtt(RTT_VALUE_MS));
    }

    #[test]
    fn write_then_parse_round_trips_an_inexactly_packable_rtt() {
        let key = rtt_link_key();
        let mut buf = [0u8; 128];
        let n = write_link_rtt(
            &LinkId::new(a16(RTT_LINK_ID)),
            &key,
            Rtt(73_115),
            &a16(RTT_IV),
            &mut buf,
        )
        .unwrap();
        let parsed = parse_link_rtt(&buf[..n], &key).unwrap();
        assert_eq!(parsed.rtt, Rtt(73_115));
    }

    fn sealed_rtt_packet_of(hostile: f64) -> Vec<u8> {
        let mut packet = hx(LRRTT_PACKET);
        packet.truncate(19);
        let mut plaintext = [0u8; LINK_RTT_PLAINTEXT_LEN];
        plaintext[0] = MSGPACK_FLOAT64;
        plaintext[1..].copy_from_slice(&hostile.to_be_bytes());
        let mut sealed = [0u8; 64];
        let n = rtt_link_key()
            .seal(&a16(RTT_IV), &plaintext, &mut sealed)
            .unwrap();
        packet.extend_from_slice(&sealed[..n]);
        packet
    }

    #[test]
    fn parse_link_rtt_saturates_a_hostile_float() {
        let key = rtt_link_key();
        let parse = |packet: &[u8]| parse_link_rtt(packet, &key).unwrap().rtt.millis();
        assert_eq!(parse(&sealed_rtt_packet_of(f64::NAN)), 0);
        assert_eq!(parse(&sealed_rtt_packet_of(-5.0)), 0);
        assert_eq!(parse(&sealed_rtt_packet_of(f64::INFINITY)), u64::MAX);
    }

    #[test]
    fn parse_link_rtt_rejects_a_tampered_token() {
        let mut bytes = hx(LRRTT_PACKET);
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert_eq!(
            parse_link_rtt(&bytes, &rtt_link_key()),
            Err(LinkRttError::InvalidToken),
        );
    }

    #[test]
    fn write_link_rtt_rejects_a_buffer_too_small_for_the_frame() {
        let mut tiny = [0u8; 40];
        assert_eq!(
            write_link_rtt(
                &LinkId::new(a16(RTT_LINK_ID)),
                &rtt_link_key(),
                Rtt(RTT_VALUE_MS),
                &a16(RTT_IV),
                &mut tiny,
            ),
            Err(LinkRttError::BufferTooShort),
        );
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn decoded_signalling_bytes_always_land_in_range() {
        let bytes: [u8; 3] = kani::any();
        let (mtu, mode_bits) = decode_signalling_bytes(&bytes);
        assert!(mtu <= LINK_MTU_BYTEMASK as usize);
        assert!(mode_bits <= 0x07);
    }

    #[kani::proof]
    fn signalling_bytes_round_trip_for_every_in_range_mtu_and_mode() {
        let mtu: u32 = kani::any();
        kani::assume(mtu <= LINK_MTU_BYTEMASK);
        let mode_bits: u8 = kani::any();
        let Some(mode) = LinkMode::from_bits(mode_bits) else {
            return;
        };
        let bytes = signalling_bytes_from(mtu as usize, mode);
        let (decoded_mtu, decoded_bits) = decode_signalling_bytes(&bytes);
        assert_eq!(decoded_mtu, mtu as usize);
        assert_eq!(decoded_bits, mode.to_bits());
    }
}
