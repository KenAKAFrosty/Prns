//! The validated announce, only constructible once its signature and destination binding both check out.

pub mod acceptance;
mod id;

pub use acceptance::{
    AcceptReason, AnnounceAcceptanceDecision, AnnounceAcceptanceInput, RejectReason,
};
pub use id::{AnnounceId, AnnounceNonce, MonotonicTimebase, ANNOUNCE_ID_WIRE_LEN};

use crate::crypto::{ed25519_verify, sha256, Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, PacketType, WirePacketHeader,
    ANNOUNCE_PUBLIC_KEY_LEN, DOTTED_NAME_HASH_LEN, MTU, RATCHET_LEN, SIGNATURE_LEN,
    TRUNCATED_HASH_BYTE_LEN,
};

/// The 64-byte announced public key, split by role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityPublicKeys {
    pub encryption: X25519PublicKey,
    pub signing: Ed25519PublicKey,
}

/// Hash of the destination's dotted app/aspect name (`app.aspect…`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DottedNameHash([u8; DOTTED_NAME_HASH_LEN]);

impl DottedNameHash {
    pub const fn new(bytes: [u8; DOTTED_NAME_HASH_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; DOTTED_NAME_HASH_LEN] {
        &self.0
    }
}

/// An X25519 forward-secrecy ratchet public key (distinct role from the
/// identity's static X25519 encryption key, despite using the same primitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatchetKey([u8; RATCHET_LEN]);

impl RatchetKey {
    pub const fn new(bytes: [u8; RATCHET_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; RATCHET_LEN] {
        &self.0
    }
}

/// A validated announce. Holds the full announce content — every field needed to
/// reproduce the exact wire payload via [`Announce::to_wire`], so the same value
/// drives both retention (for re-emission) and origination (for our own
/// announces). Routing (hops, transport id) stays on the [`WirePacketHeader`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announce<'a> {
    pub destination: DestinationHash,
    pub public_keys: IdentityPublicKeys,
    pub dotted_name_hash: DottedNameHash,
    pub announce_id: AnnounceId,
    pub ratchet: Option<RatchetKey>,
    pub signature: Ed25519Signature,
    pub app_data: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceValidationError {
    NotAnnounce,
    NotSingleDestination,
    PayloadTooSmall,
    PayloadTooBig,
    InvalidSignature,
    /// The destination hash does not match the announced identity + name hash.
    DestinationMismatch,
}

impl<'a> Announce<'a> {
    pub fn from_wire(
        header: &WirePacketHeader,
        payload: &'a [u8],
    ) -> Result<Announce<'a>, AnnounceValidationError> {
        if header.packet_type != PacketType::Announce {
            return Err(AnnounceValidationError::NotAnnounce);
        }
        if header.destination_type != DestinationType::Single {
            return Err(AnnounceValidationError::NotSingleDestination);
        }
        if payload.len() > MTU {
            return Err(AnnounceValidationError::PayloadTooBig);
        }

        let has_ratchet = header.context_flag == ContextFlag::Set;
        let ratchet_len = if has_ratchet { RATCHET_LEN } else { 0 };
        let fixed_len = ANNOUNCE_PUBLIC_KEY_LEN
            + DOTTED_NAME_HASH_LEN
            + ANNOUNCE_ID_WIRE_LEN
            + ratchet_len
            + SIGNATURE_LEN;

        if payload.len() < fixed_len {
            return Err(AnnounceValidationError::PayloadTooSmall);
        }

        // Slice the payload ( pubkey ‖ name_hash ‖ announce_id ‖ [ratchet] ‖ sig ‖ app_data )
        let mut offset = 0;
        let public_key = &payload[offset..offset + ANNOUNCE_PUBLIC_KEY_LEN];
        offset += ANNOUNCE_PUBLIC_KEY_LEN;
        let name_hash = &payload[offset..offset + DOTTED_NAME_HASH_LEN];
        offset += DOTTED_NAME_HASH_LEN;
        let announce_id = &payload[offset..offset + ANNOUNCE_ID_WIRE_LEN];
        offset += ANNOUNCE_ID_WIRE_LEN;
        let ratchet = if has_ratchet {
            let r = &payload[offset..offset + RATCHET_LEN];
            offset += RATCHET_LEN;
            Some(r)
        } else {
            None
        };
        let signature = &payload[offset..offset + SIGNATURE_LEN];
        let signed_end = offset; // everything before the signature is signed
        offset += SIGNATURE_LEN;
        let app_data = &payload[offset..];

        // Reassemble signed data ( `dest ‖ payload-before-signature ‖ app_data` )
        // The signature sits between, and is excluded.
        let mut scratch = [0u8; TRUNCATED_HASH_BYTE_LEN + MTU];
        let mut len = 0;
        scratch[len..len + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(header.destination.as_bytes());
        len += TRUNCATED_HASH_BYTE_LEN;
        scratch[len..len + signed_end].copy_from_slice(&payload[..signed_end]);
        len += signed_end;
        scratch[len..len + app_data.len()].copy_from_slice(app_data);
        len += app_data.len();

        let mut signing = [0u8; 32];
        signing.copy_from_slice(&public_key[32..]);
        let mut sig = [0u8; SIGNATURE_LEN];
        sig.copy_from_slice(signature);

        ed25519_verify(
            &Ed25519PublicKey(signing),
            &scratch[..len],
            &Ed25519Signature(sig),
        )
        .map_err(|_| AnnounceValidationError::InvalidSignature)?;

        // Destination binding ( dest == sha256(name_hash ‖ sha256(pubkey)[:16])[:16] )
        let identity_hash = sha256(public_key);
        let mut binding_input = [0u8; DOTTED_NAME_HASH_LEN + TRUNCATED_HASH_BYTE_LEN];
        binding_input[..DOTTED_NAME_HASH_LEN].copy_from_slice(name_hash);
        binding_input[DOTTED_NAME_HASH_LEN..]
            .copy_from_slice(&identity_hash[..TRUNCATED_HASH_BYTE_LEN]);
        if sha256(&binding_input)[..TRUNCATED_HASH_BYTE_LEN] != *header.destination.as_bytes() {
            return Err(AnnounceValidationError::DestinationMismatch);
        }

        let mut encryption = [0u8; 32];
        encryption.copy_from_slice(&public_key[..32]);
        let mut name = [0u8; DOTTED_NAME_HASH_LEN];
        name.copy_from_slice(name_hash);
        let mut id = [0u8; ANNOUNCE_ID_WIRE_LEN];
        id.copy_from_slice(announce_id);
        let ratchet = ratchet.map(|r| {
            let mut bytes = [0u8; RATCHET_LEN];
            bytes.copy_from_slice(r);
            RatchetKey(bytes)
        });

        Ok(Announce {
            destination: header.destination,
            public_keys: IdentityPublicKeys {
                encryption: X25519PublicKey(encryption),
                signing: Ed25519PublicKey(signing),
            },
            dotted_name_hash: DottedNameHash(name),
            announce_id: AnnounceId::from_wire(id),
            ratchet,
            signature: Ed25519Signature(sig),
            app_data,
        })
    }

    /// Lightweight check of the wire length the announce will produce when serialized (without serializing)
    pub fn wire_len(&self) -> usize {
        let ratchet_len = if self.ratchet.is_some() {
            RATCHET_LEN
        } else {
            0
        };
        ANNOUNCE_PUBLIC_KEY_LEN
            + DOTTED_NAME_HASH_LEN
            + ANNOUNCE_ID_WIRE_LEN
            + ratchet_len
            + SIGNATURE_LEN
            + self.app_data.len()
    }

    /// Serialize the announce back to a matching RNS layout
    /// ( `pubkey ‖ name_hash ‖ announce_id ‖ [ratchet] ‖ signature ‖ app_data`)
    ///
    /// Returns the number of bytes written. Used both for
    /// re-emitting a retained announce and for emitting our own.
    pub fn to_wire(&self, buf: &mut [u8]) -> Result<usize, AnnounceSerializeError> {
        let total = self.wire_len();
        if buf.len() < total {
            return Err(AnnounceSerializeError::BufferTooShort);
        }
        let mut offset = 0;
        buf[offset..offset + 32].copy_from_slice(&self.public_keys.encryption.0);
        offset += 32;
        buf[offset..offset + 32].copy_from_slice(&self.public_keys.signing.0);
        offset += 32;
        buf[offset..offset + DOTTED_NAME_HASH_LEN]
            .copy_from_slice(self.dotted_name_hash.as_bytes());
        offset += DOTTED_NAME_HASH_LEN;
        buf[offset..offset + ANNOUNCE_ID_WIRE_LEN]
            .copy_from_slice(&self.announce_id.to_wire_bytes());
        offset += ANNOUNCE_ID_WIRE_LEN;
        if let Some(ratchet) = &self.ratchet {
            buf[offset..offset + RATCHET_LEN].copy_from_slice(ratchet.as_bytes());
            offset += RATCHET_LEN;
        }
        buf[offset..offset + SIGNATURE_LEN].copy_from_slice(&self.signature.0);
        offset += SIGNATURE_LEN;
        buf[offset..offset + self.app_data.len()].copy_from_slice(self.app_data);
        offset += self.app_data.len();
        Ok(offset)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceSerializeError {
    BufferTooShort,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{ed25519_public_key, ed25519_sign, sha256, Ed25519SecretKey};

    // A genuine RNS 1.3.1 announce (SINGLE dest, app_data b"hello-personal",
    // no ratchet), generated offline; RNS confirms it validates + binds.
    const RAW: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                       59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                       0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                       7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                       4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }
    fn a<const N: usize>(s: &str) -> [u8; N] {
        hx(s).try_into().expect("expected length")
    }

    #[test]
    fn from_wire_validates_real_rns_announce() {
        let raw = hx(RAW);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();

        assert_eq!(
            announce.destination,
            DestinationHash::new(a("16f8a6d3f7d7c5b6f106d293804d7314")),
        );
        assert_eq!(
            announce.public_keys.encryption,
            X25519PublicKey(a(
                "02281f6d21232cbba9d12e516183197f08e59b7afba27e99e4fe39f01b0d4d25"
            )),
        );
        assert_eq!(
            announce.public_keys.signing,
            Ed25519PublicKey(a(
                "83a5920220253970a16861e82e52e955a05ee39e2b6d20a2331f515512f66700"
            )),
        );
        assert_eq!(
            announce.dotted_name_hash,
            DottedNameHash::new(a("9618ccc8f5ebce060084"))
        );
        assert_eq!(
            announce.announce_id,
            AnnounceId::from_wire(a("5468d9b829006a172e83"))
        );
        assert_eq!(announce.ratchet, None);
        assert_eq!(announce.app_data, b"hello-personal");
    }

    #[test]
    fn to_wire_reproduces_the_real_payload_exactly() {
        // The serializer is byte-identical with the parsed input — the round-trip
        // that lets us re-emit a retained announce with its signature intact.
        let raw = hx(RAW);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();

        let mut buf = [0u8; MTU];
        let n = announce.to_wire(&mut buf).unwrap();
        assert_eq!(n, payload.len());
        assert_eq!(&buf[..n], payload);
        assert_eq!(n, announce.wire_len());
    }

    #[test]
    fn rejects_tampered_signature() {
        let mut raw = hx(RAW);
        raw[103] ^= 1; // first signature byte: header(19) + payload sig offset(84)
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(
            Announce::from_wire(&header, payload),
            Err(AnnounceValidationError::InvalidSignature),
        );
    }

    #[test]
    fn rejects_non_single_destination() {
        let mut raw = hx(RAW);
        raw[0] |= 0b0000_0100; // flip destination_type Single -> Group
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(
            Announce::from_wire(&header, payload),
            Err(AnnounceValidationError::NotSingleDestination),
        );
    }

    #[test]
    fn rejects_truncated_payload() {
        let raw = hx(RAW);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(
            Announce::from_wire(&header, &payload[..100]),
            Err(AnnounceValidationError::PayloadTooSmall),
        );
    }

    #[test]
    fn rejects_oversized_payload() {
        let raw = hx(RAW);
        let (header, _) = WirePacketHeader::parse(&raw).unwrap();
        let oversized = [0u8; 600]; // beyond the announce MTU bound
        assert_eq!(
            Announce::from_wire(&header, &oversized),
            Err(AnnounceValidationError::PayloadTooBig),
        );
    }

    // Build an announce with our own crypto, signed over `signed_destination`.
    fn synthetic_announce(signed_destination: [u8; 16], app_data: &[u8]) -> Vec<u8> {
        let secret = Ed25519SecretKey::new([0x11u8; 32]);
        let signing = ed25519_public_key(&secret).0;
        let mut pubkey = [0u8; 64];
        pubkey[..32].copy_from_slice(&[0x22u8; 32]);
        pubkey[32..].copy_from_slice(&signing);
        let name_hash = [0x33u8; 10];
        let announce_id = [0x44u8; 10];

        let mut signed = Vec::new();
        signed.extend_from_slice(&signed_destination);
        signed.extend_from_slice(&pubkey);
        signed.extend_from_slice(&name_hash);
        signed.extend_from_slice(&announce_id);
        signed.extend_from_slice(app_data);
        let sig = ed25519_sign(&secret, &signed).0;

        let mut payload = Vec::new();
        payload.extend_from_slice(&pubkey);
        payload.extend_from_slice(&name_hash);
        payload.extend_from_slice(&announce_id);
        payload.extend_from_slice(&sig);
        payload.extend_from_slice(app_data);

        let mut raw = vec![0x01u8, 0x00]; // Announce|Single|type1, hops 0
        raw.extend_from_slice(&signed_destination); // header destination
        raw.push(0x00); // context
        raw.extend_from_slice(&payload);
        raw
    }

    fn bound_destination() -> [u8; 16] {
        let signing = ed25519_public_key(&Ed25519SecretKey::new([0x11u8; 32])).0;
        let mut pubkey = [0u8; 64];
        pubkey[..32].copy_from_slice(&[0x22u8; 32]);
        pubkey[32..].copy_from_slice(&signing);
        let idh = sha256(&pubkey);
        let mut input = [0u8; 26];
        input[..10].copy_from_slice(&[0x33u8; 10]);
        input[10..].copy_from_slice(&idh[..16]);
        sha256(&input)[..16].try_into().unwrap()
    }

    #[test]
    fn synthetic_announce_with_correct_binding_validates() {
        let raw = synthetic_announce(bound_destination(), b"app");
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();
        assert_eq!(announce.app_data, b"app");
        assert_eq!(
            announce.destination,
            DestinationHash::new(bound_destination())
        );
    }

    #[test]
    fn rejects_destination_mismatch() {
        // Signature is valid (signed over this dest), but the dest doesn't match
        // the pubkey+name binding, so it must still be rejected.
        let raw = synthetic_announce([0x99u8; 16], b"app");
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(
            Announce::from_wire(&header, payload),
            Err(AnnounceValidationError::DestinationMismatch),
        );
    }
}
