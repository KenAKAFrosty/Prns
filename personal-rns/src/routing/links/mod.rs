//! The core link primitives: the [`LinkId`] both ends derive, the negotiated
//! [`LinkMode`], and the AES-256 session [`LinkKey`] — HKDF of the ECDH shared
//! secret salted by the `link_id`. The establishment frames that carry them
//! live in [`handshake`]; the per-link state the engine tracks lives in
//! [`table`].

pub mod data;
pub mod establish;
pub mod handshake;
pub mod maintenance;
pub mod table;

use crate::crypto::{
    hkdf_sha256, sha256_chunks, token_open, token_open_in_place, token_seal, CryptoError,
    Ed25519PublicKey, TokenKey, X25519PublicKey, X25519SharedSecret,
};
use crate::wire::{
    DestinationHash, DestinationType, PacketType, WireContext, TRUNCATED_HASH_BYTE_LEN,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const LINK_KEY_LEN: usize = 64;

/// The engine's one ceiling on a negotiated link MTU. Today it equals the
/// broadcast MTU because the interface seam's frame is sized to it; raising
/// fatter links means turning this knob alongside the seam's frame ceiling —
/// a deliberate, per-target memory decision, never a silent side effect.
pub const MAX_LINK_MTU: usize = crate::wire::BROADCAST_MTU;

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

/// The cipher a link negotiates. RNS 1.3.1 enables only `MODE_AES256_CBC`
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
