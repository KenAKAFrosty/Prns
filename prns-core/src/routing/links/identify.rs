//! RNS 1.3.1 `Link.identify` / `Packet.LINKIDENTIFY` (0xFB): the initiator reveals a held
//! identity over the encrypted link, public keys and a signature over `link_id ‖ keys`
//! sealed under the session key, so the identity is shown to the peer and no one else.
//! Fire-and-forget: the reference neither proves nor acknowledges an identify.

use crate::crypto::{ed25519_verify, Ed25519PublicKey, Ed25519Signature};
use crate::engine::commands::{CommandId, CommandOutcome, Identify, IdentifyError};
use crate::engine::EngineState;
use crate::identity::{
    IdentityEncryptionPublicKey, IdentityHash, IdentitySigner, IdentitySigningPublicKey,
    RemoteIdentity,
};
use crate::interfaces::InterfaceId;
use crate::routing::links::table::{LinkPhase, LinkRole};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader, TRUNCATED_HASH_BYTE_LEN,
};

/// RNS 1.3.1 `Identity.KEYSIZE//8 + Identity.SIGLENGTH//8`: the named
/// identity's public keys (encryption ‖ signing) followed by its signature.
pub const IDENTIFY_PLAINTEXT_LEN: usize = 64 + 64;

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
                error: IdentifyError::NoSuchLink,
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::IdentifyRejected {
                    id,
                    error: IdentifyError::LinkNotActive,
                }
            }
            Some(LinkPhase::Active {
                role: LinkRole::Responder { .. },
                ..
            }) => CommandOutcome::IdentifyRejected {
                id,
                error: IdentifyError::NotInitiator,
            },
            Some(LinkPhase::Active {
                role: LinkRole::Initiator { .. },
                ..
            }) => {
                if self.held_identities.get(&identify.identity).is_none() {
                    CommandOutcome::IdentifyRejected {
                        id,
                        error: IdentifyError::IdentityNotHeld,
                    }
                } else {
                    CommandOutcome::OwesIdentify { id, identify }
                }
            }
        }
    }

    /// RNS 1.3.1 `Link.identify` verbatim: `signed_data = link_id ‖ keys`,
    /// payload `keys ‖ signature`, sealed, context LINKIDENTIFY.
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

        let mut plaintext = [0u8; IDENTIFY_PLAINTEXT_LEN];
        plaintext[..32].copy_from_slice(identity.encryption_public_key().as_bytes());
        plaintext[32..64].copy_from_slice(identity.signing_public_key().as_bytes());
        let mut signed_data = [0u8; TRUNCATED_HASH_BYTE_LEN + 64];
        signed_data[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(identify.link_id.as_bytes());
        signed_data[TRUNCATED_HASH_BYTE_LEN..].copy_from_slice(&plaintext[..64]);
        let signature = identity.sign(&signed_data);
        plaintext[64..].copy_from_slice(&signature.0);

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Link,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination: DestinationHash::new(*identify.link_id.as_bytes()),
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

/// The responder's read of a decrypted identify — RNS 1.3.1 `Link.receive`'s
/// LINKIDENTIFY arm: exact length, then the signature must cover
/// `link_id ‖ keys` under the named keys' own signing half.
pub fn peer_identity_from(link_id: &LinkId, plaintext: &[u8]) -> Option<IdentityHash> {
    if plaintext.len() != IDENTIFY_PLAINTEXT_LEN {
        return None;
    }
    let mut encryption = [0u8; 32];
    encryption.copy_from_slice(&plaintext[..32]);
    let mut signing = [0u8; 32];
    signing.copy_from_slice(&plaintext[32..64]);
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&plaintext[64..]);

    let mut signed_data = [0u8; TRUNCATED_HASH_BYTE_LEN + 64];
    signed_data[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(link_id.as_bytes());
    signed_data[TRUNCATED_HASH_BYTE_LEN..].copy_from_slice(&plaintext[..64]);
    ed25519_verify(
        &Ed25519PublicKey(signing),
        &signed_data,
        &Ed25519Signature(signature),
    )
    .ok()?;

    let remote = RemoteIdentity::from_public_keys(
        IdentityEncryptionPublicKey::new(crate::crypto::X25519PublicKey(encryption)),
        IdentitySigningPublicKey::new(Ed25519PublicKey(signing)),
    );
    Some(remote.identity_hash())
}
