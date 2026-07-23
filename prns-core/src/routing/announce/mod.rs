pub mod acceptance;
pub mod defaults;
pub mod destination_announce_limit;
pub mod emit;
pub mod held;
mod id;
pub mod interface_announce_limit;
pub mod schedule;
pub mod stored;
mod wire;

pub use acceptance::{
    determine_acceptance, AcceptReason, AnnounceAcceptanceDecision, AnnounceAcceptanceInput,
    RejectReason,
};
pub use id::{AnnounceEntropy, AnnounceId, AnnounceNonce, MonotonicTimebase, ANNOUNCE_ID_WIRE_LEN};
pub use wire::{
    write_announce_wire_packet, write_path_response_announce_wire_packet,
    write_relayed_path_response_wire_packet, write_retransmitted_announce_wire_packet,
};

use crate::crypto::{ed25519_verify, sha256, Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
pub use crate::identity::IdentityPublicKeys;
use crate::identity::{
    IdentityEncryptionPublicKey, IdentityHash, IdentitySigner, IdentitySigningPublicKey,
};
use crate::interfaces::InterfaceId;
use crate::routing::NextHop;
use crate::units::HopCount;
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

/// RNS 1.4.0 `Destination.hash`'s name-hash step: `sha256("app.aspect1.aspect2".utf8)` truncated to [`DOTTED_NAME_HASH_BYTE_LEN`] bytes; feed [`derive_destination_hash`] to address it.
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

/// `sha256(name_hash ‖ identity_hash)[..16]`: the final step of RNS 1.4.0 `Destination.hash`.
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

/// `sha256(name_hash)[..16]`: the identity-less arm of RNS 1.4.0 `Destination.hash`.
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
pub struct AnnounceObservation<'a> {
    pub destination: DestinationHash,
    pub announced_identity: IdentityHash,
    pub hops: HopCount,
    pub source_interface: InterfaceId,
    pub arrived_at: InstantMillis,
    pub app_data: &'a [u8],
    pub is_path_response: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceRateAccounting {
    NotApplied,
    Started,
    Continued,
}

impl<'a> AnnounceObservation<'a> {
    pub const fn from_arrival(
        announced_identity: IdentityHash,
        arrival: &AnnounceArrival<'a>,
    ) -> Self {
        Self {
            destination: arrival.announce.destination,
            announced_identity,
            hops: HopCount(arrival.hops),
            source_interface: arrival.receiving_interface,
            arrived_at: arrival.arrived_at,
            app_data: arrival.announce.app_data,
            is_path_response: arrival.is_path_response,
        }
    }
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

    /// Splits out the Ed25519 verify, the one heavy step, so it can run inline or off the manifold on the crypto pool.
    pub fn from_wire_unverified(
        header: &WirePacketHeader,
        payload: &'a [u8],
    ) -> Result<Announce<'a>, AnnounceValidationError> {
        Self::from_wire_unverified_with_identity(header, payload).map(|(announce, _)| announce)
    }

    pub(crate) fn from_wire_unverified_with_identity(
        header: &WirePacketHeader,
        payload: &'a [u8],
    ) -> Result<(Announce<'a>, IdentityHash), AnnounceValidationError> {
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

        let identity_hash = announce.public_keys.identity_hash();
        if derive_destination_hash(&identity_hash, &announce.dotted_name_hash)
            != announce.destination
        {
            return Err(AnnounceValidationError::DestinationMismatch);
        }

        Ok((announce, identity_hash))
    }

    /// The heavy step, separated. When available, the crypto pool runs it off the manifold on a [`Self::from_wire_unverified`]-parsed announce, otherwise [`Self::from_wire`] runs both steps inline.
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
mod tests;
