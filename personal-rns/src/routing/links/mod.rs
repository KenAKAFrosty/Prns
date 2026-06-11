//! The per-link session key derived during establishment (RNS 1.3.1 `Link`).
//! After the X25519 handshake both ends HKDF the ECDH shared secret with the
//! `link_id` as salt (context none) into one 64-byte AES-256 Token key, then
//! protect every link data packet with it (`iv ‖ AES-256-CBC ‖ HMAC-SHA256`).

use crate::crypto::{
    hkdf_sha256, sha256_chunks, token_open, token_open_in_place, token_seal, CryptoError,
    Ed25519PublicKey, TokenKey, X25519PublicKey, X25519SharedSecret,
};
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WireError, WirePacketHeader, MTU, TRUNCATED_HASH_BYTE_LEN,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const LINK_KEY_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkId([u8; TRUNCATED_HASH_BYTE_LEN]);

impl LinkId {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub fn derive(
        destination: &DestinationHash,
        initiator_encryption: &X25519PublicKey,
        initiator_signing: &Ed25519PublicKey,
    ) -> Self {
        const FLAGS_NIBBLE: u8 =
            ((DestinationType::Single as u8) << 2) | (PacketType::LinkRequest as u8);
        let digest = sha256_chunks(&[
            &[FLAGS_NIBBLE],
            destination.as_bytes(),
            &[WireContext::None.to_byte()],
            &initiator_encryption.0,
            &initiator_signing.0,
        ]);
        let mut id = [0u8; TRUNCATED_HASH_BYTE_LEN];
        id.copy_from_slice(&digest[..TRUNCATED_HASH_BYTE_LEN]);
        Self(id)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}

/// A link's derived session key: 64-byte AES-256 Token key (32-byte signing
/// half ‖ 32-byte encryption half)
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct LinkKey {
    material: [u8; LINK_KEY_LEN],
}

impl LinkKey {
    pub fn derive(link_id: &LinkId, shared: &X25519SharedSecret) -> Self {
        Self {
            material: hkdf_sha256::<LINK_KEY_LEN>(shared.as_bytes(), link_id.as_bytes(), &[]),
        }
    }

    pub fn seal(
        &self,
        iv: &[u8; 16],
        plaintext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, CryptoError> {
        token_seal(&TokenKey::from_derived(&self.material)?, iv, plaintext, out)
    }

    pub fn open(&self, token: &[u8], out: &mut [u8]) -> Result<usize, CryptoError> {
        token_open(&TokenKey::from_derived(&self.material)?, token, out)
    }

    pub fn open_in_place<'t>(&self, token: &'t mut [u8]) -> Result<&'t [u8], CryptoError> {
        token_open_in_place(&TokenKey::from_derived(&self.material)?, token)
    }
}

impl core::fmt::Debug for LinkKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LinkKey").finish_non_exhaustive()
    }
}

/// The cipher a link negotiates. RNS  1.3.1 enables only `MODE_AES256_CBC`
/// (`ENABLED_MODES = [0x01]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    Aes256Cbc,
}

impl LinkMode {
    const fn to_bits(self) -> u8 {
        match self {
            Self::Aes256Cbc => 0x01,
        }
    }

    const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0x01 => Some(Self::Aes256Cbc),
            _ => None,
        }
    }
}

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
    use crate::crypto::{x25519_diffie_hellman, X25519SecretKey};

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
    fn a64(s: &str) -> [u8; 64] {
        hx(s).try_into().expect("64 bytes")
    }

    const INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    const RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";
    const SHARED_SECRET: &str = "3c528e9fd39731b15d10de8feb5f71d3f65b73c993581dedb03315a9ed177730";
    const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    const DERIVED_KEY: &str = "c44718017ed8c8dd932f6e3fc65c00edda249daeaaf006a6920ad02905b3d766\
                               40ea59958b62b1f452f00d2762ca217f45f2028886e79c8cf4e09eb18d37b83a";
    const PLAINTEXT: &[u8] = b"link layer rides the same token!";
    const CIPHER_IV: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0";
    const LINK_TOKEN: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb012a31f7217fde987fbb8bab1ef73d3b3\
                              b63557757d0c3adea6b0e94e9d27f23ba732763cc4ed566de7c915bafe3e5467\
                              99a834e0e6579c62ccb6da661641040a56430127964af6eafdae462cd79e8ff0";
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

    fn derived_link_key() -> LinkKey {
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new(a32(INITIATOR_SCALAR)),
            &X25519PublicKey(a32(RESPONDER_PUBLIC)),
        );
        assert_eq!(
            shared.as_bytes(),
            &a32(SHARED_SECRET),
            "the ECDH leg must reproduce the reference shared secret",
        );
        LinkKey::derive(&LinkId::new(a16(LINK_ID)), &shared)
    }

    #[test]
    fn derive_matches_the_reference_handshake() {
        assert_eq!(derived_link_key().material, a64(DERIVED_KEY));
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
    fn the_link_cipher_seals_and_opens_against_the_reference_token() {
        let key = derived_link_key();

        let mut sealed = [0u8; 128];
        let n = key.seal(&a16(CIPHER_IV), PLAINTEXT, &mut sealed).unwrap();
        assert_eq!(&sealed[..n], &hx(LINK_TOKEN)[..], "seal matches RNS Token");

        let mut out = [0u8; 128];
        let m = key.open(&hx(LINK_TOKEN), &mut out).unwrap();
        assert_eq!(&out[..m], PLAINTEXT, "open recovers the plaintext");

        let mut token = hx(LINK_TOKEN);
        assert_eq!(key.open_in_place(&mut token).unwrap(), PLAINTEXT);
    }
}
