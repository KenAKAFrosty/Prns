//! RNS 1.3.5 `Link.identify` / `Packet.LINKIDENTIFY` (0xFB).
//!
//! The initiator reveals a held identity over the encrypted link, public keys and a signature over `link_id ‖ keys sealed under the session key, so the identity is shown to the peer and no one else.
//!
//! Fire-and-forget: the reference neither proves nor acknowledges an identify.

use crate::crypto::{ed25519_verify, Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::engine::EngineState;
use crate::engine::{CommandId, CommandOutcome, Identify, IdentifyRejection};
use crate::identity::{
    IdentityEncryptionPublicKey, IdentityHash, IdentitySigner, IdentitySigningPublicKey,
    RemoteIdentity, IDENTITY_PUBLIC_KEY_LEN,
};
use crate::interfaces::InterfaceId;
use crate::routing::links::table::{LinkPhase, LinkRole};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext,
    WirePacketHeader, TRUNCATED_HASH_BYTE_LEN,
};

/// RNS 1.3.5 `Identity.KEYSIZE//8 + Identity.SIGLENGTH//8`: the named
/// identity's public keys (encryption ‖ signing) followed by its signature.
pub const IDENTIFY_PLAINTEXT_LEN: usize = IDENTITY_PUBLIC_KEY_LEN + Ed25519Signature::LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentifyDispatch {
    pub wire_len: usize,
    pub fire_on: InterfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifyWriteError {
    LinkVanished,
    IdentityVanished,
    BufferTooShort,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_identify(&self, id: CommandId, identify: Identify) -> CommandOutcome {
        match self.links.phase_for(&identify.link_id) {
            None => CommandOutcome::IdentifyRejected {
                id,
                rejection: IdentifyRejection::NoSuchLink,
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::IdentifyRejected {
                    id,
                    rejection: IdentifyRejection::LinkNotActive,
                }
            }
            Some(LinkPhase::Active {
                role: LinkRole::Responder { .. },
                ..
            }) => CommandOutcome::IdentifyRejected {
                id,
                rejection: IdentifyRejection::NotInitiator,
            },
            Some(LinkPhase::Active {
                role: LinkRole::Initiator { .. },
                ..
            }) => {
                if self.held_identities.get(&identify.identity).is_none() {
                    CommandOutcome::IdentifyRejected {
                        id,
                        rejection: IdentifyRejection::IdentityNotHeld,
                    }
                } else {
                    CommandOutcome::OwesIdentify { id, identify }
                }
            }
        }
    }

    /// RNS 1.3.5 `Link.identify` verbatim: `signed_data = link_id ‖ keys`, payload `keys ‖ signature`, sealed, context LINKIDENTIFY.
    pub fn write_commanded_identify(
        &self,
        identify: &Identify,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<IdentifyDispatch, IdentifyWriteError> {
        let Some(LinkPhase::Active {
            key,
            attached_interface,
            ..
        }) = self.links.phase_for(&identify.link_id)
        else {
            return Err(IdentifyWriteError::LinkVanished);
        };
        let identity = self
            .held_identities
            .get(&identify.identity)
            .ok_or(IdentifyWriteError::IdentityVanished)?;

        let keys = identity.public_key_bytes();
        let signed_data = identify_signed_data(&identify.link_id, &keys);
        let signature = identity.sign(&signed_data);

        let mut plaintext = [0u8; IDENTIFY_PLAINTEXT_LEN];
        plaintext[..IDENTITY_PUBLIC_KEY_LEN].copy_from_slice(&keys);
        plaintext[IDENTITY_PUBLIC_KEY_LEN..].copy_from_slice(&signature.0);

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Link,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            address: identify.link_id.to_address(),
            context: WireContext::LinkIdentify,
        };
        let header_len = header
            .write(buf)
            .map_err(|_| IdentifyWriteError::BufferTooShort)?;
        let sealed = key
            .seal(iv, &plaintext, &mut buf[header_len..])
            .map_err(|_| IdentifyWriteError::BufferTooShort)?;
        Ok(IdentifyDispatch {
            wire_len: header_len + sealed,
            fire_on: *attached_interface,
        })
    }
}

fn identify_signed_data(
    link_id: &LinkId,
    keys: &[u8; IDENTITY_PUBLIC_KEY_LEN],
) -> [u8; TRUNCATED_HASH_BYTE_LEN + IDENTITY_PUBLIC_KEY_LEN] {
    let mut signed_data = [0u8; TRUNCATED_HASH_BYTE_LEN + IDENTITY_PUBLIC_KEY_LEN];
    signed_data[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(link_id.as_bytes());
    signed_data[TRUNCATED_HASH_BYTE_LEN..].copy_from_slice(keys);
    signed_data
}

/// The responder's read of a decrypted identify.
/// RNS 1.3.5 `Link.receive`'s LINKIDENTIFY arm: exact length, then the signature must cover `link_id ‖ keys` under the named keys' own signing half.
pub fn peer_identity_from(link_id: &LinkId, plaintext: &[u8]) -> Option<IdentityHash> {
    if plaintext.len() != IDENTIFY_PLAINTEXT_LEN {
        return None;
    }
    let keys: &[u8; IDENTITY_PUBLIC_KEY_LEN] =
        plaintext[..IDENTITY_PUBLIC_KEY_LEN].try_into().ok()?;
    let mut encryption = [0u8; X25519PublicKey::LEN];
    encryption.copy_from_slice(&keys[..X25519PublicKey::LEN]);
    let mut signing = [0u8; Ed25519PublicKey::LEN];
    signing.copy_from_slice(&keys[X25519PublicKey::LEN..]);
    let mut signature = [0u8; Ed25519Signature::LEN];
    signature.copy_from_slice(&plaintext[IDENTITY_PUBLIC_KEY_LEN..]);

    let signed_data = identify_signed_data(link_id, keys);
    ed25519_verify(
        &Ed25519PublicKey(signing),
        &signed_data,
        &Ed25519Signature(signature),
    )
    .ok()?;

    let remote = RemoteIdentity::from_public_keys(
        IdentityEncryptionPublicKey::new(X25519PublicKey(encryption)),
        IdentitySigningPublicKey::new(Ed25519PublicKey(signing)),
    );
    Some(remote.identity_hash())
}
