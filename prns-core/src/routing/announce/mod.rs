pub mod acceptance;
pub mod defaults;
pub mod destination_announce_limit;
pub mod emit;
pub mod held;
mod id;
pub mod interface_announce_limit;
pub mod schedule;
pub mod stored;

pub use acceptance::{
    AcceptReason, AnnounceAcceptanceDecision, AnnounceAcceptanceInput, RejectReason,
};
pub use id::{AnnounceEntropy, AnnounceId, AnnounceNonce, MonotonicTimebase, ANNOUNCE_ID_WIRE_LEN};

use crate::crypto::{ed25519_verify, sha256, Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{
    derive_identity_hash, IdentityEncryptionPublicKey, IdentityHash, IdentitySigner,
    IdentitySigningPublicKey,
};
use crate::interfaces::InterfaceId;
use crate::routing::NextHop;
use crate::units::InstantMillis;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, PacketType, WirePacketHeader,
    ANNOUNCE_PUBLIC_KEY_BYTE_LEN, BROADCAST_MTU, DOTTED_NAME_HASH_BYTE_LEN, RATCHET_BYTE_LEN,
    SIGNATURE_BYTE_LEN, TRUNCATED_HASH_BYTE_LEN,
};
use heapless::Vec as HeaplessVec;

pub const ANNOUNCE_FIXED_FIELDS_LEN: usize = ANNOUNCE_PUBLIC_KEY_BYTE_LEN
    + DOTTED_NAME_HASH_BYTE_LEN
    + ANNOUNCE_ID_WIRE_LEN
    + SIGNATURE_BYTE_LEN;
const _: () = assert!(ANNOUNCE_PUBLIC_KEY_BYTE_LEN == X25519PublicKey::LEN + Ed25519PublicKey::LEN);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityPublicKeys {
    pub encryption: IdentityEncryptionPublicKey,
    pub signing: IdentitySigningPublicKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DottedNameHash([u8; DOTTED_NAME_HASH_BYTE_LEN]);

impl DottedNameHash {
    pub const fn new(bytes: [u8; DOTTED_NAME_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; DOTTED_NAME_HASH_BYTE_LEN] {
        &self.0
    }
}

pub const MAX_DOTTED_NAME_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpandNameError {
    DotInComponent,
    NameTooLong,
}

/// RNS 1.3.5 `Destination.hash`'s name-hash step: `sha256("app.aspect1.aspect2".utf8)` truncated to [`DOTTED_NAME_HASH_BYTE_LEN`] bytes; feed [`derive_destination_hash`] to address it.
pub fn expand_name(app_name: &str, aspects: &[&str]) -> Result<DottedNameHash, ExpandNameError> {
    if app_name.contains('.') {
        return Err(ExpandNameError::DotInComponent);
    }
    let mut name: HeaplessVec<u8, MAX_DOTTED_NAME_LEN> = HeaplessVec::new();
    name.extend_from_slice(app_name.as_bytes())
        .map_err(|_| ExpandNameError::NameTooLong)?;
    for aspect in aspects {
        if aspect.contains('.') {
            return Err(ExpandNameError::DotInComponent);
        }
        name.push(b'.').map_err(|_| ExpandNameError::NameTooLong)?;
        name.extend_from_slice(aspect.as_bytes())
            .map_err(|_| ExpandNameError::NameTooLong)?;
    }

    let mut name_hash = [0u8; DOTTED_NAME_HASH_BYTE_LEN];
    name_hash.copy_from_slice(&sha256(&name)[..DOTTED_NAME_HASH_BYTE_LEN]);
    Ok(DottedNameHash::new(name_hash))
}

/// `sha256(name_hash ‖ identity_hash)[..16]`: the final step of RNS 1.3.5 `Destination.hash`.
/// Both directions run through this one derivation, so a validated announce and one we emit can never disagree on how a destination is addressed.
pub fn derive_destination_hash(
    identity_hash: &IdentityHash,
    dotted_name_hash: &DottedNameHash,
) -> DestinationHash {
    let mut material = [0u8; DOTTED_NAME_HASH_BYTE_LEN + TRUNCATED_HASH_BYTE_LEN];
    material[..DOTTED_NAME_HASH_BYTE_LEN].copy_from_slice(dotted_name_hash.as_bytes());
    material[DOTTED_NAME_HASH_BYTE_LEN..].copy_from_slice(identity_hash.as_bytes());

    let mut truncated = [0u8; TRUNCATED_HASH_BYTE_LEN];
    truncated.copy_from_slice(&sha256(&material)[..TRUNCATED_HASH_BYTE_LEN]);
    DestinationHash::new(truncated)
}

/// `sha256(name_hash)[..16]`: the identity-less arm of RNS 1.3.5 `Destination.hash`.
/// A plain destination is owned by no identity, so its address binds to the name alone.
pub fn derive_plain_destination_hash(dotted_name_hash: &DottedNameHash) -> DestinationHash {
    let mut truncated = [0u8; TRUNCATED_HASH_BYTE_LEN];
    truncated.copy_from_slice(&sha256(dotted_name_hash.as_bytes())[..TRUNCATED_HASH_BYTE_LEN]);
    DestinationHash::new(truncated)
}

pub fn derive_single_destination_hash(
    identity_hash: &IdentityHash,
    app_name: &str,
    aspects: &[&str],
) -> Result<DestinationHash, ExpandNameError> {
    Ok(derive_destination_hash(
        identity_hash,
        &expand_name(app_name, aspects)?,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatchetKey([u8; RATCHET_BYTE_LEN]);

impl RatchetKey {
    pub const fn new(bytes: [u8; RATCHET_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; RATCHET_BYTE_LEN] {
        &self.0
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceArrival<'a> {
    pub announce: Announce<'a>,
    pub hops: u8,
    pub arrived_at: InstantMillis,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
    pub is_path_response: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceValidationError {
    NotAnnounce,
    NotSingleDestination,
    PayloadTooSmall,
    PayloadTooBig,
    InvalidSignature,
    DestinationMismatch,
}

impl<'a> Announce<'a> {
    pub fn from_wire(
        header: &WirePacketHeader,
        payload: &'a [u8],
    ) -> Result<Announce<'a>, AnnounceValidationError> {
        let announce = Self::from_wire_unverified(header, payload)?;
        if !announce.signature_is_valid() {
            return Err(AnnounceValidationError::InvalidSignature);
        }
        Ok(announce)
    }

    /// Splits out the Ed25519 verify, the one heavy step, so it can run inline or off the reactor on the crypto pool.
    pub fn from_wire_unverified(
        header: &WirePacketHeader,
        payload: &'a [u8],
    ) -> Result<Announce<'a>, AnnounceValidationError> {
        if header.packet_type != PacketType::Announce {
            return Err(AnnounceValidationError::NotAnnounce);
        }
        if header.destination_type != DestinationType::Single {
            return Err(AnnounceValidationError::NotSingleDestination);
        }
        if payload.len() > BROADCAST_MTU {
            return Err(AnnounceValidationError::PayloadTooBig);
        }

        let has_ratchet = header.context_flag == ContextFlag::Set;

        let (encryption, rest) = payload
            .split_first_chunk()
            .ok_or(AnnounceValidationError::PayloadTooSmall)?;

        let (signing, rest) = rest
            .split_first_chunk()
            .ok_or(AnnounceValidationError::PayloadTooSmall)?;

        let (name_hash, rest) = rest
            .split_first_chunk()
            .ok_or(AnnounceValidationError::PayloadTooSmall)?;

        let (announce_id, rest) = rest
            .split_first_chunk()
            .ok_or(AnnounceValidationError::PayloadTooSmall)?;

        let (ratchet, rest) = if has_ratchet {
            let (ratchet, rest) = rest
                .split_first_chunk()
                .ok_or(AnnounceValidationError::PayloadTooSmall)?;
            (Some(RatchetKey(*ratchet)), rest)
        } else {
            (None, rest)
        };

        let (signature, app_data) = rest
            .split_first_chunk()
            .ok_or(AnnounceValidationError::PayloadTooSmall)?;

        let announce = Announce {
            destination: DestinationHash::from_address(header.address),
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey(*encryption)),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey(*signing)),
            },
            dotted_name_hash: DottedNameHash::new(*name_hash),
            announce_id: AnnounceId::from_wire(*announce_id),
            ratchet,
            signature: Ed25519Signature(*signature),
            app_data,
        };

        let identity_hash = derive_identity_hash(
            &announce.public_keys.encryption,
            &announce.public_keys.signing,
        );
        if derive_destination_hash(&identity_hash, &announce.dotted_name_hash)
            != announce.destination
        {
            return Err(AnnounceValidationError::DestinationMismatch);
        }

        Ok(announce)
    }

    /// The heavy step, separated. When available, the crypto pool runs it off the reactor on a [`Self::from_wire_unverified`]-parsed announce, otherwise [`Self::from_wire`] runs both steps inline.
    pub fn signature_is_valid(&self) -> bool {
        // The scratch (16 + BROADCAST_MTU) always fits: the source payload is <= BROADCAST_MTU.
        let mut scratch = [0u8; TRUNCATED_HASH_BYTE_LEN + BROADCAST_MTU];
        let Ok(signed_len) = self.write_signed_material(&mut scratch) else {
            return false;
        };
        ed25519_verify(
            self.public_keys.signing.as_ed25519(),
            &scratch[..signed_len],
            &self.signature,
        )
        .is_ok()
    }

    pub fn build_signed(
        signer: &impl IdentitySigner,
        dotted_name_hash: DottedNameHash,
        announce_id: AnnounceId,
        ratchet: Option<RatchetKey>,
        app_data: &'a [u8],
    ) -> Result<Announce<'a>, AnnounceBuildError> {
        // A signature cannot sign itself, so write_signed_material never reads the signature field.
        // The zeroed placeholder just lets the struct exist first: build, sign what it serializes, then fill in the real signature.
        let mut announce = Announce {
            destination: derive_destination_hash(&signer.identity_hash(), &dotted_name_hash),
            public_keys: IdentityPublicKeys {
                encryption: signer.encryption_public_key(),
                signing: signer.signing_public_key(),
            },
            dotted_name_hash,
            announce_id,
            ratchet,
            signature: Ed25519Signature([0u8; SIGNATURE_BYTE_LEN]),
            app_data,
        };

        let mut scratch = [0u8; TRUNCATED_HASH_BYTE_LEN + BROADCAST_MTU];
        let signed_len = announce
            .write_signed_material(&mut scratch)
            .map_err(|BufferTooShort| AnnounceBuildError::AnnounceTooLarge)?;
        announce.signature = signer.sign(&scratch[..signed_len]);
        Ok(announce)
    }

    pub fn wire_len(&self) -> usize {
        let ratchet_len = if self.ratchet.is_some() {
            RATCHET_BYTE_LEN
        } else {
            0
        };
        ANNOUNCE_PUBLIC_KEY_BYTE_LEN
            + DOTTED_NAME_HASH_BYTE_LEN
            + ANNOUNCE_ID_WIRE_LEN
            + ratchet_len
            + SIGNATURE_BYTE_LEN
            + self.app_data.len()
    }

    fn write_fields_before_signature(&self, buf: &mut [u8], mut offset: usize) -> usize {
        buf[offset..offset + X25519PublicKey::LEN]
            .copy_from_slice(self.public_keys.encryption.as_bytes());
        offset += X25519PublicKey::LEN;

        buf[offset..offset + Ed25519PublicKey::LEN]
            .copy_from_slice(self.public_keys.signing.as_bytes());
        offset += Ed25519PublicKey::LEN;

        buf[offset..offset + DOTTED_NAME_HASH_BYTE_LEN]
            .copy_from_slice(self.dotted_name_hash.as_bytes());
        offset += DOTTED_NAME_HASH_BYTE_LEN;

        buf[offset..offset + ANNOUNCE_ID_WIRE_LEN]
            .copy_from_slice(&self.announce_id.to_wire_bytes());
        offset += ANNOUNCE_ID_WIRE_LEN;

        if let Some(ratchet) = &self.ratchet {
            buf[offset..offset + RATCHET_BYTE_LEN].copy_from_slice(ratchet.as_bytes());
            offset += RATCHET_BYTE_LEN;
        }
        offset
    }

    /// Mirrors RNS `Destination.announce`'s `signed_data`.
    fn write_signed_material(&self, buf: &mut [u8]) -> Result<usize, BufferTooShort> {
        let total = TRUNCATED_HASH_BYTE_LEN + self.wire_len() - SIGNATURE_BYTE_LEN;
        if buf.len() < total {
            return Err(BufferTooShort);
        }
        let mut offset = 0;

        buf[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(self.destination.as_bytes());
        offset += TRUNCATED_HASH_BYTE_LEN;

        offset = self.write_fields_before_signature(buf, offset);

        buf[offset..offset + self.app_data.len()].copy_from_slice(self.app_data);
        offset += self.app_data.len();

        Ok(offset)
    }

    pub fn to_wire(&self, buf: &mut [u8]) -> Result<usize, BufferTooShort> {
        let total = self.wire_len();
        if buf.len() < total {
            return Err(BufferTooShort);
        }

        let mut offset = self.write_fields_before_signature(buf, 0);

        buf[offset..offset + SIGNATURE_BYTE_LEN].copy_from_slice(&self.signature.0);
        offset += SIGNATURE_BYTE_LEN;

        buf[offset..offset + self.app_data.len()].copy_from_slice(self.app_data);
        offset += self.app_data.len();

        Ok(offset)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferTooShort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceBuildError {
    AnnounceTooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{ed25519_public_key, ed25519_sign, sha256, Ed25519SecretKey};

    const RAW: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                       59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                       0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                       7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                       4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn bytes_from_hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }
    fn a<const N: usize>(s: &str) -> [u8; N] {
        bytes_from_hex(s).try_into().expect("expected length")
    }

    #[test]
    fn from_wire_validates_real_rns_announce() {
        let raw = bytes_from_hex(RAW);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();

        assert_eq!(
            announce.destination,
            DestinationHash::new(a("16f8a6d3f7d7c5b6f106d293804d7314")),
        );
        assert_eq!(
            announce.public_keys.encryption,
            IdentityEncryptionPublicKey::new(X25519PublicKey(a(
                "02281f6d21232cbba9d12e516183197f08e59b7afba27e99e4fe39f01b0d4d25"
            ))),
        );
        assert_eq!(
            announce.public_keys.signing,
            IdentitySigningPublicKey::new(Ed25519PublicKey(a(
                "83a5920220253970a16861e82e52e955a05ee39e2b6d20a2331f515512f66700"
            ))),
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
        let raw = bytes_from_hex(RAW);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();

        let mut buf = [0u8; BROADCAST_MTU];
        let n = announce.to_wire(&mut buf).unwrap();
        assert_eq!(n, payload.len());
        assert_eq!(&buf[..n], payload);
        assert_eq!(n, announce.wire_len());
    }

    #[test]
    fn rejects_tampered_signature() {
        let mut raw = bytes_from_hex(RAW);
        raw[103] ^= 1;
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(
            Announce::from_wire(&header, payload),
            Err(AnnounceValidationError::InvalidSignature),
        );
    }

    #[test]
    fn rejects_non_single_destination() {
        let mut raw = bytes_from_hex(RAW);
        raw[0] |= 0b0000_0100;
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(
            Announce::from_wire(&header, payload),
            Err(AnnounceValidationError::NotSingleDestination),
        );
    }

    #[test]
    fn rejects_truncated_payload() {
        let raw = bytes_from_hex(RAW);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(
            Announce::from_wire(&header, &payload[..100]),
            Err(AnnounceValidationError::PayloadTooSmall),
        );
    }

    #[test]
    fn rejects_oversized_payload() {
        let raw = bytes_from_hex(RAW);
        let (header, _) = WirePacketHeader::parse(&raw).unwrap();
        let oversized = [0u8; 600];
        assert_eq!(
            Announce::from_wire(&header, &oversized),
            Err(AnnounceValidationError::PayloadTooBig),
        );
    }

    fn synthetic_announce(
        signed_destination: [u8; 16],
        ratchet: Option<[u8; RATCHET_BYTE_LEN]>,
        app_data: &[u8],
    ) -> Vec<u8> {
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
        if let Some(ratchet) = ratchet {
            signed.extend_from_slice(&ratchet);
        }
        signed.extend_from_slice(app_data);
        let sig = ed25519_sign(&secret, &signed).0;

        let mut payload = Vec::new();
        payload.extend_from_slice(&pubkey);
        payload.extend_from_slice(&name_hash);
        payload.extend_from_slice(&announce_id);
        if let Some(ratchet) = ratchet {
            payload.extend_from_slice(&ratchet);
        }
        payload.extend_from_slice(&sig);
        payload.extend_from_slice(app_data);

        let flags = if ratchet.is_some() { 0x21 } else { 0x01 };
        let mut raw = vec![flags, 0x00];
        raw.extend_from_slice(&signed_destination);
        raw.push(0x00);
        raw.extend_from_slice(&payload);
        raw
    }

    fn bound_destination() -> [u8; 16] {
        let signing = ed25519_public_key(&Ed25519SecretKey::new([0x11u8; 32])).0;
        let mut pubkey = [0u8; 64];
        pubkey[..32].copy_from_slice(&[0x22u8; 32]);
        pubkey[32..].copy_from_slice(&signing);
        let mut idh = [0u8; 16];
        idh.copy_from_slice(&sha256(&pubkey)[..16]);
        *derive_destination_hash(&IdentityHash::new(idh), &DottedNameHash::new([0x33u8; 10]))
            .as_bytes()
    }

    #[test]
    fn synthetic_announce_with_correct_binding_validates() {
        let raw = synthetic_announce(bound_destination(), None, b"app");
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();
        assert_eq!(announce.app_data, b"app");
        assert_eq!(
            announce.destination,
            DestinationHash::new(bound_destination())
        );
    }

    #[test]
    fn synthetic_ratchet_announce_validates_and_round_trips() {
        let ratchet = [0x55u8; RATCHET_BYTE_LEN];
        let raw = synthetic_announce(bound_destination(), Some(ratchet), b"ratchet-app");
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(header.context_flag, ContextFlag::Set);

        let announce = Announce::from_wire(&header, payload).unwrap();
        assert_eq!(announce.ratchet, Some(RatchetKey::new(ratchet)));
        assert_eq!(announce.app_data, b"ratchet-app");

        let mut buf = [0u8; BROADCAST_MTU];
        let n = announce.to_wire(&mut buf).unwrap();
        assert_eq!(n, payload.len());
        assert_eq!(&buf[..n], payload);
    }

    #[test]
    fn rejects_destination_mismatch() {
        let raw = synthetic_announce([0x99u8; 16], None, b"app");
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(
            Announce::from_wire(&header, payload),
            Err(AnnounceValidationError::DestinationMismatch),
        );
    }

    #[test]
    fn derive_destination_hash_matches_rns_1_3_5() {
        let identity_hash = IdentityHash::new(a("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let dotted_name_hash = DottedNameHash::new(a("8794b70072dbf251144b"));
        assert_eq!(
            derive_destination_hash(&identity_hash, &dotted_name_hash),
            DestinationHash::new(a("33d610d1d6a7f4f809ebfe62c0ce7d43")),
        );

        let real_pubkey = a::<64>(
            "02281f6d21232cbba9d12e516183197f08e59b7afba27e99e4fe39f01b0d4d25\
             83a5920220253970a16861e82e52e955a05ee39e2b6d20a2331f515512f66700",
        );
        let mut real_identity_hash = [0u8; 16];
        real_identity_hash.copy_from_slice(&sha256(&real_pubkey)[..16]);
        assert_eq!(
            derive_destination_hash(
                &IdentityHash::new(real_identity_hash),
                &DottedNameHash::new(a("9618ccc8f5ebce060084")),
            ),
            DestinationHash::new(a("16f8a6d3f7d7c5b6f106d293804d7314")),
        );
    }

    #[test]
    fn derive_plain_destination_hash_matches_rns_1_3_5() {
        // rnstransport.path.request derives from its name alone: the plain-destination arm.
        let name = expand_name("rnstransport", &["path", "request"]).unwrap();
        assert_eq!(name, DottedNameHash::new(a("7926bbe7dd7f9aba88b0")));
        assert_eq!(
            derive_plain_destination_hash(&name),
            DestinationHash::new(a("6b9f66014d9853faab220fba47d02761")),
        );
    }

    #[test]
    fn derive_single_destination_hash_composes_the_rns_1_3_5_address_from_name_parts() {
        let identity_hash = IdentityHash::new(a("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        assert_eq!(
            derive_single_destination_hash(&identity_hash, "personal", &["node"]),
            Ok(DestinationHash::new(a("c3cfae69b36bb6e3bbfd96a3b5867a59"))),
        );
        assert_eq!(
            derive_single_destination_hash(&identity_hash, "per.sonal", &["node"]),
            Err(ExpandNameError::DotInComponent),
        );
    }

    #[test]
    fn expand_name_matches_rns_1_3_5() {
        assert_eq!(
            expand_name("personal", &["announce"]).unwrap(),
            DottedNameHash::new(a("8794b70072dbf251144b")),
        );
        assert_eq!(
            expand_name("personal", &["node"]).unwrap(),
            DottedNameHash::new(a("ab49baa826f122c1437f")),
        );
        assert_eq!(
            expand_name("personal", &[]).unwrap(),
            DottedNameHash::new(a("4a0a339b0c6d05538977")),
        );
    }

    #[test]
    fn expand_name_rejects_dots_in_components_like_rns() {
        assert_eq!(
            expand_name("per.sonal", &["node"]),
            Err(ExpandNameError::DotInComponent),
        );
        assert_eq!(
            expand_name("personal", &["no.de"]),
            Err(ExpandNameError::DotInComponent),
        );
    }

    #[test]
    fn expand_name_rejects_names_past_the_bound() {
        let overlong = "x".repeat(MAX_DOTTED_NAME_LEN + 1);
        assert_eq!(
            expand_name(&overlong, &[]),
            Err(ExpandNameError::NameTooLong),
        );
    }

    #[test]
    fn build_signed_matches_rns_1_3_5() {
        use crate::identity::in_memory::InMemoryNodeIdentity;
        let mut secret_key_bytes = [0u8; 64];
        secret_key_bytes[..32].fill(0x22);
        secret_key_bytes[32..].fill(0x11);
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key_bytes);

        let announce = Announce::build_signed(
            &identity,
            DottedNameHash::new(a("8794b70072dbf251144b")),
            AnnounceId::from_wire([0x44; ANNOUNCE_ID_WIRE_LEN]),
            None,
            b"hello-personal",
        )
        .unwrap();

        assert_eq!(
            announce.destination,
            DestinationHash::new(a("33d610d1d6a7f4f809ebfe62c0ce7d43")),
        );
        let mut buf = [0u8; BROADCAST_MTU];
        let n = announce.to_wire(&mut buf).unwrap();
        assert_eq!(
            &buf[..n],
            bytes_from_hex(
                "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
                d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737\
                8794b70072dbf251144b\
                44444444444444444444\
                77000516a77f83f26b6fd0abc4b9b8a0de0fd8bc51f82fe55e14b75628b41955\
                c895395870fe4cd0b69afc85e4969cc3b70dbeb14d8c3c7ddc08692e0968010e\
                68656c6c6f2d706572736f6e616c"
            )
            .as_slice(),
        );
    }

    #[test]
    fn build_signed_round_trips_through_the_validator() {
        use crate::identity::in_memory::InMemoryNodeIdentity;
        let mut secret_key_bytes = [0u8; 64];
        secret_key_bytes[..32].fill(0x07);
        secret_key_bytes[32..].fill(0x09);
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key_bytes);

        let built = Announce::build_signed(
            &identity,
            DottedNameHash::new([0xAB; DOTTED_NAME_HASH_BYTE_LEN]),
            AnnounceId::from_wire([0x01; ANNOUNCE_ID_WIRE_LEN]),
            Some(RatchetKey::new([0x55; RATCHET_BYTE_LEN])),
            b"round-trip",
        )
        .unwrap();

        let mut payload = [0u8; BROADCAST_MTU];
        let n = built.to_wire(&mut payload).unwrap();
        let mut raw = vec![0x21u8, 0x00];
        raw.extend_from_slice(built.destination.as_bytes());
        raw.push(0x00);
        raw.extend_from_slice(&payload[..n]);

        let (header, parsed_payload) = WirePacketHeader::parse(&raw).unwrap();
        let parsed = Announce::from_wire(&header, parsed_payload).unwrap();
        assert_eq!(parsed, built);
    }
}
