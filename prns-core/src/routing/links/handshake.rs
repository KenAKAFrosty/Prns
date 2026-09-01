use super::{LinkId, LinkKey, LinkMode};
use crate::crypto::{
    ed25519_verify, Ed25519PublicKey, Ed25519SecretKey, Ed25519Signature, X25519PublicKey,
    X25519SecretKey,
};
use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::identity::{IdentityHash, IdentitySigner, IdentitySigningPublicKey};
use crate::interfaces::InterfaceId;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::units::RttMillis;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WireError, WirePacketHeader, BROADCAST_MTU, TRUNCATED_HASH_BYTE_LEN,
};

const LINK_MTU_BYTEMASK: u32 = 0x1F_FFFF;

/// RNS `Link.signalling_bytes`: the negotiated link MTU (low 21 bits) and mode (top 3 bits) packed big-endian into 3 bytes.
/// `link_id` excludes these, so a relay may clamp the MTU without moving the id.
///
/// A link request that carried no signalling bytes parses to `mtu == 0`: zero is the wire's "no MTU requested", never a real MTU.
/// Resolved here once: an unsignalled request gets the broadcast default, and the interface ceiling caps either.
pub fn negotiated_link_mtu(requested: usize, ceiling: usize) -> usize {
    if requested == 0 {
        crate::wire::BROADCAST_MTU
    } else {
        requested
    }
    .min(ceiling)
}

pub fn signalling_bytes_from(mtu: usize, mode: LinkMode) -> [u8; SIGNALLING_BYTES_LEN] {
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
    write_link_request_with_signalling(
        destination,
        via,
        initiator_encryption,
        initiator_signing,
        Some((mtu, mode)),
        buf,
    )
}

pub fn write_unsignalled_link_request(
    destination: &DestinationHash,
    via: Option<TransportId>,
    initiator_encryption: &X25519PublicKey,
    initiator_signing: &Ed25519PublicKey,
    buf: &mut [u8],
) -> Result<usize, WireError> {
    write_link_request_with_signalling(
        destination,
        via,
        initiator_encryption,
        initiator_signing,
        None,
        buf,
    )
}

fn write_link_request_with_signalling(
    destination: &DestinationHash,
    via: Option<TransportId>,
    initiator_encryption: &X25519PublicKey,
    initiator_signing: &Ed25519PublicKey,
    signalling: Option<(usize, LinkMode)>,
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
        address: destination.to_address(),
        context: WireContext::None,
    };
    let header_len = header.write(buf)?;
    let body = &mut buf[header_len..];
    let (encryption_slot, body) = body
        .split_first_chunk_mut()
        .ok_or(WireError::BufferTooShort)?;
    *encryption_slot = initiator_encryption.0;
    let (signing_slot, body) = body
        .split_first_chunk_mut()
        .ok_or(WireError::BufferTooShort)?;
    *signing_slot = initiator_signing.0;
    let base_len = header_len + initiator_encryption.0.len() + initiator_signing.0.len();
    let Some((mtu, mode)) = signalling else {
        return Ok(base_len);
    };
    let signalling = signalling_bytes_from(mtu, mode);
    let (signalling_slot, _) = body
        .split_first_chunk_mut()
        .ok_or(WireError::BufferTooShort)?;
    *signalling_slot = signalling;
    Ok(base_len + signalling.len())
}

pub const SIGNALLING_BYTES_LEN: usize = 3;
pub const LINK_REQUEST_KEYS_LEN: usize = X25519PublicKey::LEN + Ed25519PublicKey::LEN;
pub const SIGNALLED_LINK_REQUEST_LEN: usize = LINK_REQUEST_KEYS_LEN + SIGNALLING_BYTES_LEN;
pub const LINK_PROOF_SIGNED_DATA_LEN: usize =
    TRUNCATED_HASH_BYTE_LEN + X25519PublicKey::LEN + Ed25519PublicKey::LEN + SIGNALLING_BYTES_LEN;

fn decode_signalling_bytes(bytes: &[u8; SIGNALLING_BYTES_LEN]) -> (usize, u8) {
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

    let mut encryption = [0u8; X25519PublicKey::LEN];
    encryption.copy_from_slice(&keys[..X25519PublicKey::LEN]);
    let mut signing = [0u8; Ed25519PublicKey::LEN];
    signing.copy_from_slice(&keys[X25519PublicKey::LEN..LINK_REQUEST_KEYS_LEN]);
    let initiator_encryption = X25519PublicKey(encryption);
    let initiator_signing = Ed25519PublicKey(signing);

    Ok(LinkRequest {
        destination: DestinationHash::from_address(header.address),
        link_id: LinkId::derive(
            &DestinationHash::from_address(header.address),
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

/// The exact bytes a LRPROOF signs: `link_id ++ responder_encryption ++ responder_signing ++ signalling`.
/// Inline and worker signers both frame identical material.
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
    signed_data[o..o + X25519PublicKey::LEN].copy_from_slice(&responder_encryption.0);
    o += X25519PublicKey::LEN;
    signed_data[o..o + Ed25519PublicKey::LEN].copy_from_slice(&responder_signing.0);
    o += Ed25519PublicKey::LEN;
    signed_data[o..o + SIGNALLING_BYTES_LEN].copy_from_slice(&signalling);
    signed_data
}

/// The assembly half of [`write_link_proof`]; worker and inline signs both share one wire path.
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
        address: link_id.to_address(),
        context: WireContext::LinkRequestProof,
    };
    let header_len = header.write(buf)?;
    let body = &mut buf[header_len..];
    let (signature_slot, body) = body
        .split_first_chunk_mut()
        .ok_or(WireError::BufferTooShort)?;
    *signature_slot = signature.0;
    let (encryption_slot, body) = body
        .split_first_chunk_mut()
        .ok_or(WireError::BufferTooShort)?;
    *encryption_slot = responder_encryption.0;
    let (signalling_slot, _) = body
        .split_first_chunk_mut()
        .ok_or(WireError::BufferTooShort)?;
    *signalling_slot = signalling;
    Ok(header_len + signature.0.len() + responder_encryption.0.len() + signalling.len())
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

pub const LINK_PROOF_BODY_LEN: usize = Ed25519Signature::LEN + X25519PublicKey::LEN;
pub const SIGNALLED_LINK_PROOF_LEN: usize = LINK_PROOF_BODY_LEN + SIGNALLING_BYTES_LEN;

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
        &LinkId::from_address(header.address),
        payload,
        responder_signing,
    )
}

pub struct LinkProofParsed {
    pub proof: LinkProof,
    pub signed_data: [u8; LINK_PROOF_SIGNED_DATA_LEN],
    pub signed_bytes: usize,
    pub signature: Ed25519Signature,
}

/// Moves through the seam (the secret is not `Copy`); the verdict rides back as the derived shared secret, so a valid proof never makes a second pool round-trip.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub struct LinkProofVerifyOwed {
    pub link_id: LinkId,
    pub source_interface: InterfaceId,
    pub received_hops: u8,
    pub responder_encryption: X25519PublicKey,
    pub responder_signing: Ed25519PublicKey,
    pub initiator_secret: X25519SecretKey,
    pub command_id: CommandId,
    pub arrived_at: InstantMillis,
    pub rtt: RttMillis,
    pub mtu: usize,
    pub signed_data: [u8; LINK_PROOF_SIGNED_DATA_LEN],
    pub signed_bytes: usize,
    pub signature: Ed25519Signature,
}

pub fn link_proof_signature_valid(owed: &LinkProofVerifyOwed) -> bool {
    ed25519_verify(
        &owed.responder_signing,
        &owed.signed_data[..owed.signed_bytes],
        &owed.signature,
    )
    .is_ok()
}

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
            let mut bytes = [0u8; SIGNALLING_BYTES_LEN];
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

    let mut signature = [0u8; Ed25519Signature::LEN];
    signature.copy_from_slice(&body[..Ed25519Signature::LEN]);
    let mut responder = [0u8; X25519PublicKey::LEN];
    responder.copy_from_slice(&body[Ed25519Signature::LEN..LINK_PROOF_BODY_LEN]);
    let responder_encryption = X25519PublicKey(responder);

    let mut signed_data = [0u8; LINK_PROOF_SIGNED_DATA_LEN];
    let mut o = 0;
    signed_data[o..o + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(link_id.as_bytes());
    o += TRUNCATED_HASH_BYTE_LEN;
    signed_data[o..o + X25519PublicKey::LEN].copy_from_slice(&responder_encryption.0);
    o += X25519PublicKey::LEN;
    signed_data[o..o + Ed25519PublicKey::LEN].copy_from_slice(&responder_signing.0);
    o += Ed25519PublicKey::LEN;
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
        signed_bytes: o,
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
        &parsed.signed_data[..parsed.signed_bytes],
        &parsed.signature,
    )
    .map_err(|_| LinkProofError::InvalidSignature)?;
    Ok(parsed.proof)
}

const MSGPACK_FLOAT32: u8 = 0xca;
const MSGPACK_FLOAT64: u8 = 0xcb;
const MSGPACK_UINT8: u8 = 0xcc;
const MSGPACK_UINT16: u8 = 0xcd;
const MSGPACK_UINT32: u8 = 0xce;
const MSGPACK_UINT64: u8 = 0xcf;
const MSGPACK_INT8: u8 = 0xd0;
const MSGPACK_INT16: u8 = 0xd1;
const MSGPACK_INT32: u8 = 0xd2;
const MSGPACK_INT64: u8 = 0xd3;
const LINK_RTT_PLAINTEXT_LEN: usize = 9;

fn pack_rtt(rtt: RttMillis) -> [u8; LINK_RTT_PLAINTEXT_LEN] {
    let mut out = [0u8; LINK_RTT_PLAINTEXT_LEN];
    out[0] = MSGPACK_FLOAT64;
    out[1..].copy_from_slice(&(rtt.millis() as f64 / 1_000.0).to_be_bytes());
    out
}

fn message_pack_numeric_body<const LENGTH: usize>(
    body: &[u8],
) -> Result<[u8; LENGTH], LinkRttError> {
    body.try_into().map_err(|_| LinkRttError::Malformed)
}

fn unpack_rtt(bytes: &[u8]) -> Result<RttMillis, LinkRttError> {
    let Some((marker, body)) = bytes.split_first() else {
        return Err(LinkRttError::Malformed);
    };
    let seconds = match *marker {
        value @ 0x00..=0x7f => {
            if !body.is_empty() {
                return Err(LinkRttError::Malformed);
            }
            f64::from(value)
        }
        value @ 0xe0..=0xff => {
            if !body.is_empty() {
                return Err(LinkRttError::Malformed);
            }
            f64::from(value as i8)
        }
        MSGPACK_FLOAT32 => f64::from(f32::from_be_bytes(message_pack_numeric_body(body)?)),
        MSGPACK_FLOAT64 => f64::from_be_bytes(message_pack_numeric_body(body)?),
        MSGPACK_UINT8 => f64::from(u8::from_be_bytes(message_pack_numeric_body(body)?)),
        MSGPACK_UINT16 => f64::from(u16::from_be_bytes(message_pack_numeric_body(body)?)),
        MSGPACK_UINT32 => f64::from(u32::from_be_bytes(message_pack_numeric_body(body)?)),
        MSGPACK_UINT64 => u64::from_be_bytes(message_pack_numeric_body(body)?) as f64,
        MSGPACK_INT8 => f64::from(i8::from_be_bytes(message_pack_numeric_body(body)?)),
        MSGPACK_INT16 => f64::from(i16::from_be_bytes(message_pack_numeric_body(body)?)),
        MSGPACK_INT32 => f64::from(i32::from_be_bytes(message_pack_numeric_body(body)?)),
        MSGPACK_INT64 => i64::from_be_bytes(message_pack_numeric_body(body)?) as f64,
        _ => return Err(LinkRttError::Malformed),
    };
    Ok(RttMillis::new((seconds * 1_000.0 + 0.5) as u64))
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
    rtt: RttMillis,
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
        address: link_id.to_address(),
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
    pub rtt: RttMillis,
}

pub fn parse_link_rtt(raw: &[u8], link_key: &LinkKey) -> Result<LinkRtt, LinkRttError> {
    let (header, payload) = WirePacketHeader::parse(raw).map_err(|_| LinkRttError::Malformed)?;
    link_rtt_from(&LinkId::from_address(header.address), payload, link_key)
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
    let rtt = unpack_rtt(&out[..n])?;
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

    fn bytes_from_hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }
    fn a16(s: &str) -> [u8; 16] {
        bytes_from_hex(s).try_into().expect("16 bytes")
    }
    fn a32(s: &str) -> [u8; 32] {
        bytes_from_hex(s).try_into().expect("32 bytes")
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
    // Minted from RNS 1.3.5 and revalidated with 1.4.2: the pre-signalling LRPROOF form (signature ‖ encryption key, no signalling bytes) that `Link.validate_proof` still accepts from older peers. A reference `Identity` signed `link_id ‖ pub ‖ sig_pub`, and `Identity.validate` self-checked it before pinning.
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
        assert_eq!(&buf[..n], &bytes_from_hex(REQUEST_PACKET)[..]);
    }

    #[test]
    fn an_unsignalled_link_request_contains_only_the_two_ephemeral_keys() {
        let mut buf = [0u8; 128];
        let n = write_unsignalled_link_request(
            &DestinationHash::new(a16(LINK_DEST)),
            None,
            &X25519PublicKey(a32(INITIATOR_ENCRYPTION_PUBLIC)),
            &Ed25519PublicKey(a32(INITIATOR_SIGNING_PUBLIC)),
            &mut buf,
        )
        .unwrap();
        let (_, payload) = WirePacketHeader::parse(&buf[..n]).unwrap();
        assert_eq!(payload.len(), LINK_REQUEST_KEYS_LEN);
        let parsed = parse_link_request(&buf[..n]).unwrap();
        assert!(!parsed.signalled);
        assert_eq!(parsed.mtu, BROADCAST_MTU);
        assert_eq!(negotiated_link_mtu(0, 131_072), BROADCAST_MTU);
    }

    #[test]
    fn write_link_request_fills_an_exact_buffer_and_rejects_one_byte_short() {
        let exact = bytes_from_hex(REQUEST_PACKET).len();
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
        let parsed = parse_link_request(&bytes_from_hex(REQUEST_PACKET)).unwrap();
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
        let bytes = bytes_from_hex(REQUEST_PACKET);
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
        let bytes = bytes_from_hex(REQUEST_PACKET);
        assert_eq!(
            parse_link_request(&bytes[..50]),
            Err(LinkRequestError::Malformed)
        );
    }

    #[test]
    fn parse_link_request_rejects_an_unsupported_mode() {
        let mut bytes = bytes_from_hex(REQUEST_PACKET);
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
        assert_eq!(&buf[..n], &bytes_from_hex(LINK_PROOF_PACKET)[..]);
    }

    #[test]
    fn write_link_proof_fills_an_exact_buffer_and_rejects_one_byte_short() {
        let exact = bytes_from_hex(LINK_PROOF_PACKET).len();
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
            &bytes_from_hex(LINK_PROOF_PACKET),
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
        let proof =
            validate_link_proof(&bytes_from_hex(UNSIGNALLED_LINK_PROOF_PACKET), &signing).unwrap();
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
        let mut bytes = bytes_from_hex(LINK_PROOF_PACKET);
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
                &bytes_from_hex(LINK_PROOF_PACKET),
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
            RttMillis::new(RTT_VALUE_MS),
            &a16(RTT_IV),
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], &bytes_from_hex(LRRTT_PACKET)[..]);
    }

    #[test]
    fn parse_link_rtt_recovers_the_reference_rtt() {
        let parsed = parse_link_rtt(&bytes_from_hex(LRRTT_PACKET), &rtt_link_key()).unwrap();
        assert_eq!(parsed.link_id, LinkId::new(a16(RTT_LINK_ID)));
        assert_eq!(parsed.rtt, RttMillis::new(RTT_VALUE_MS));
    }

    #[test]
    fn write_then_parse_round_trips_an_inexactly_packable_rtt() {
        let key = rtt_link_key();
        let mut buf = [0u8; 128];
        let n = write_link_rtt(
            &LinkId::new(a16(RTT_LINK_ID)),
            &key,
            RttMillis::new(73_115),
            &a16(RTT_IV),
            &mut buf,
        )
        .unwrap();
        let parsed = parse_link_rtt(&buf[..n], &key).unwrap();
        assert_eq!(parsed.rtt, RttMillis::new(73_115));
    }

    struct NumericRttCase {
        intent: &'static str,
        encoded: &'static [u8],
        expected_millis: u64,
    }

    const POSITIVE_FIXINT_ZERO_SECONDS: NumericRttCase = NumericRttCase {
        intent: "positive fixint zero",
        encoded: &[0x00],
        expected_millis: 0,
    };
    const POSITIVE_FIXINT_MAXIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "positive fixint maximum",
        encoded: &[0x7f],
        expected_millis: 127_000,
    };
    const NEGATIVE_FIXINT_MINUS_ONE_SECOND: NumericRttCase = NumericRttCase {
        intent: "negative fixint minus one saturates to zero",
        encoded: &[0xff],
        expected_millis: 0,
    };
    const NEGATIVE_FIXINT_MINIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "negative fixint minimum saturates to zero",
        encoded: &[0xe0],
        expected_millis: 0,
    };
    const FLOAT32_ONE_EIGHTH_SECOND: NumericRttCase = NumericRttCase {
        intent: "float32 one eighth second",
        encoded: &[MSGPACK_FLOAT32, 0x3e, 0x00, 0x00, 0x00],
        expected_millis: 125,
    };
    const FLOAT64_ONE_EIGHTH_SECOND: NumericRttCase = NumericRttCase {
        intent: "float64 one eighth second",
        encoded: &[
            MSGPACK_FLOAT64,
            0x3f,
            0xc0,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
        ],
        expected_millis: 125,
    };
    const UINT8_MAXIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "uint8 maximum",
        encoded: &[MSGPACK_UINT8, 0xff],
        expected_millis: 255_000,
    };
    const UINT16_MAXIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "uint16 maximum",
        encoded: &[MSGPACK_UINT16, 0xff, 0xff],
        expected_millis: 65_535_000,
    };
    const UINT32_MAXIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "uint32 maximum",
        encoded: &[MSGPACK_UINT32, 0xff, 0xff, 0xff, 0xff],
        expected_millis: 4_294_967_295_000,
    };
    const UINT64_MAXIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "uint64 maximum saturates milliseconds",
        encoded: &[
            MSGPACK_UINT64,
            0xff,
            0xff,
            0xff,
            0xff,
            0xff,
            0xff,
            0xff,
            0xff,
        ],
        expected_millis: u64::MAX,
    };
    const INT8_MAXIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "int8 maximum",
        encoded: &[MSGPACK_INT8, 0x7f],
        expected_millis: 127_000,
    };
    const INT8_MINIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "int8 minimum saturates to zero",
        encoded: &[MSGPACK_INT8, 0x80],
        expected_millis: 0,
    };
    const INT16_MAXIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "int16 maximum",
        encoded: &[MSGPACK_INT16, 0x7f, 0xff],
        expected_millis: 32_767_000,
    };
    const INT16_MINIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "int16 minimum saturates to zero",
        encoded: &[MSGPACK_INT16, 0x80, 0x00],
        expected_millis: 0,
    };
    const INT32_MAXIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "int32 maximum",
        encoded: &[MSGPACK_INT32, 0x7f, 0xff, 0xff, 0xff],
        expected_millis: 2_147_483_647_000,
    };
    const INT32_MINIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "int32 minimum saturates to zero",
        encoded: &[MSGPACK_INT32, 0x80, 0x00, 0x00, 0x00],
        expected_millis: 0,
    };
    const INT64_MAXIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "int64 maximum saturates milliseconds",
        encoded: &[
            MSGPACK_INT64,
            0x7f,
            0xff,
            0xff,
            0xff,
            0xff,
            0xff,
            0xff,
            0xff,
        ],
        expected_millis: u64::MAX,
    };
    const INT64_MINIMUM_SECONDS: NumericRttCase = NumericRttCase {
        intent: "int64 minimum saturates to zero",
        encoded: &[
            MSGPACK_INT64,
            0x80,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
        ],
        expected_millis: 0,
    };
    const NUMERIC_RTT_CASES: &[NumericRttCase] = &[
        POSITIVE_FIXINT_ZERO_SECONDS,
        POSITIVE_FIXINT_MAXIMUM_SECONDS,
        NEGATIVE_FIXINT_MINUS_ONE_SECOND,
        NEGATIVE_FIXINT_MINIMUM_SECONDS,
        FLOAT32_ONE_EIGHTH_SECOND,
        FLOAT64_ONE_EIGHTH_SECOND,
        UINT8_MAXIMUM_SECONDS,
        UINT16_MAXIMUM_SECONDS,
        UINT32_MAXIMUM_SECONDS,
        UINT64_MAXIMUM_SECONDS,
        INT8_MAXIMUM_SECONDS,
        INT8_MINIMUM_SECONDS,
        INT16_MAXIMUM_SECONDS,
        INT16_MINIMUM_SECONDS,
        INT32_MAXIMUM_SECONDS,
        INT32_MINIMUM_SECONDS,
        INT64_MAXIMUM_SECONDS,
        INT64_MINIMUM_SECONDS,
    ];

    #[test]
    fn unpack_link_rtt_accepts_every_msgpack_numeric_scalar_encoding() {
        for case in NUMERIC_RTT_CASES {
            assert_eq!(
                unpack_rtt(case.encoded),
                Ok(RttMillis::new(case.expected_millis)),
                "{}: {:02x?}",
                case.intent,
                case.encoded,
            );
        }
    }

    #[test]
    fn unpack_link_rtt_saturates_hostile_float32_values() {
        let parse = |value: f32| {
            let mut encoded = [0u8; 5];
            encoded[0] = MSGPACK_FLOAT32;
            encoded[1..].copy_from_slice(&value.to_be_bytes());
            unpack_rtt(&encoded).unwrap().millis()
        };

        assert_eq!(parse(f32::NAN), 0);
        assert_eq!(parse(-5.0), 0);
        assert_eq!(parse(f32::INFINITY), u64::MAX);
    }

    struct MalformedRttCase {
        intent: &'static str,
        encoded: &'static [u8],
    }

    const MALFORMED_RTT_CASES: &[MalformedRttCase] = &[
        MalformedRttCase {
            intent: "empty plaintext has no MessagePack value",
            encoded: &[],
        },
        MalformedRttCase {
            intent: "nil is not numeric",
            encoded: &[0xc0],
        },
        MalformedRttCase {
            intent: "the reserved marker is invalid MessagePack",
            encoded: &[0xc1],
        },
        MalformedRttCase {
            intent: "false is not numeric",
            encoded: &[0xc2],
        },
        MalformedRttCase {
            intent: "true is not numeric",
            encoded: &[0xc3],
        },
        MalformedRttCase {
            intent: "an empty array is not numeric",
            encoded: &[0x90],
        },
        MalformedRttCase {
            intent: "an empty string is not numeric",
            encoded: &[0xa0],
        },
        MalformedRttCase {
            intent: "an empty binary value is not numeric",
            encoded: &[0xc4, 0x00],
        },
        MalformedRttCase {
            intent: "float32 without a body is incomplete",
            encoded: &[MSGPACK_FLOAT32],
        },
        MalformedRttCase {
            intent: "float32 with a three-byte body is incomplete",
            encoded: &[MSGPACK_FLOAT32, 0x3e, 0x00, 0x00],
        },
        MalformedRttCase {
            intent: "float64 without a body is incomplete",
            encoded: &[MSGPACK_FLOAT64],
        },
        MalformedRttCase {
            intent: "float64 with a seven-byte body is incomplete",
            encoded: &[MSGPACK_FLOAT64, 0x3f, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00],
        },
        MalformedRttCase {
            intent: "uint8 without a body is incomplete",
            encoded: &[MSGPACK_UINT8],
        },
        MalformedRttCase {
            intent: "uint16 with a one-byte body is incomplete",
            encoded: &[MSGPACK_UINT16, 0x01],
        },
        MalformedRttCase {
            intent: "uint32 with a three-byte body is incomplete",
            encoded: &[MSGPACK_UINT32, 0x00, 0x00, 0x01],
        },
        MalformedRttCase {
            intent: "uint64 with a seven-byte body is incomplete",
            encoded: &[MSGPACK_UINT64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01],
        },
        MalformedRttCase {
            intent: "int8 without a body is incomplete",
            encoded: &[MSGPACK_INT8],
        },
        MalformedRttCase {
            intent: "int16 with a one-byte body is incomplete",
            encoded: &[MSGPACK_INT16, 0x01],
        },
        MalformedRttCase {
            intent: "int32 with a three-byte body is incomplete",
            encoded: &[MSGPACK_INT32, 0x00, 0x00, 0x01],
        },
        MalformedRttCase {
            intent: "int64 with a seven-byte body is incomplete",
            encoded: &[MSGPACK_INT64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01],
        },
        MalformedRttCase {
            intent: "a fixint followed by another value has trailing data",
            encoded: &[0x01, 0x00],
        },
        MalformedRttCase {
            intent: "a complete float32 followed by another byte has trailing data",
            encoded: &[MSGPACK_FLOAT32, 0x3e, 0x00, 0x00, 0x00, 0x00],
        },
    ];

    #[test]
    fn unpack_link_rtt_rejects_nonnumeric_incomplete_and_trailing_data() {
        for case in MALFORMED_RTT_CASES {
            assert_eq!(
                unpack_rtt(case.encoded),
                Err(LinkRttError::Malformed),
                "{}: {:02x?}",
                case.intent,
                case.encoded,
            );
        }
    }

    fn sealed_rtt_packet_of(hostile: f64) -> Vec<u8> {
        let mut packet = bytes_from_hex(LRRTT_PACKET);
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
        let mut bytes = bytes_from_hex(LRRTT_PACKET);
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
                RttMillis::new(RTT_VALUE_MS),
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
