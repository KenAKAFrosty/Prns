//! RNS 1.4.2 `Link.identify` / `Packet.LINKIDENTIFY` (0xFB).
//!
//! The initiator reveals a held identity over the encrypted link, public keys and a signature over `link_id ‖ keys sealed under the session key, so the identity is shown to the peer and no one else.
//!
//! Fire-and-forget: the reference neither proves nor acknowledges an identify.

use crate::crypto::{
    ed25519_verify, Ed25519PublicKey, Ed25519SecretKey, Ed25519Signature, X25519PublicKey,
};
use crate::engine::EngineState;
use crate::engine::{
    settle, CommandId, CommandOutcome, Directive, EngineReaction, Identify, IdentifyFailure,
    IdentifyRejection, InstantMillis, Journaled, Settlement, WakeSchedules,
};
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

/// RNS 1.4.2 `Identity.KEYSIZE//8 + Identity.SIGLENGTH//8`: the named identity's public keys (encryption ‖ signing) followed by its signature.
pub const IDENTIFY_PLAINTEXT_LEN: usize = IDENTITY_PUBLIC_KEY_LEN + Ed25519Signature::LEN;
pub const IDENTIFY_SIGNED_DATA_LEN: usize = TRUNCATED_HASH_BYTE_LEN + IDENTITY_PUBLIC_KEY_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentifyDispatch {
    pub wire_bytes: usize,
    pub fire_on: InterfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifyWriteError {
    LinkVanished,
    IdentityVanished,
    BufferTooShort,
}

/// A LINKIDENTIFY whose policy inputs are complete and whose signature may be produced outside
/// the engine. The fixed signed region and cloned secret are the whole continuation boundary;
/// link encryption and egress remain guarded by a fresh authority check in `resume_identify_sign`.
#[repr(C)]
pub struct IdentifySignOwed {
    pub command_id: CommandId,
    pub identify: Identify,
    pub iv: [u8; 16],
    pub public_keys: [u8; IDENTITY_PUBLIC_KEY_LEN],
    pub signed_data: [u8; IDENTIFY_SIGNED_DATA_LEN],
    pub signing_secret: Ed25519SecretKey,
}

/// A runtime-produced LINKIDENTIFY signature submitted as a later engine input.
#[repr(C)]
pub struct IdentifySignCompleted {
    pub owed: IdentifySignOwed,
    pub signature: Ed25519Signature,
}

/// A decrypted LINKIDENTIFY whose signature must be verified before its claimed identity may be
/// attached to the link.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkIdentityVerifyOwed {
    pub link_id: LinkId,
    pub identity: IdentityHash,
    pub signing_key: Ed25519PublicKey,
    pub signed_data: [u8; IDENTIFY_SIGNED_DATA_LEN],
    pub signature: Ed25519Signature,
    pub arrived_at: InstantMillis,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkIdentityVerification {
    Valid,
    Invalid,
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

    /// RNS 1.4.2 `Link.identify` verbatim: `signed_data = link_id ‖ keys`, payload `keys ‖ signature`, sealed, context LINKIDENTIFY.
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
        let signature = identity.sign(&identify_signed_data(&identify.link_id, &keys));

        write_identify_with_signature(identify, iv, &keys, &signature, key, buf)
            .map(|wire_bytes| IdentifyDispatch {
                wire_bytes,
                fire_on: *attached_interface,
            })
            .map_err(|_| IdentifyWriteError::BufferTooShort)
    }

    /// Materializes exactly the pure signing input authorized by `ingest_identify`.
    pub fn prepare_identify_sign(
        &self,
        command_id: CommandId,
        identify: Identify,
        iv: [u8; 16],
    ) -> Result<IdentifySignOwed, IdentifyRejection> {
        match self.links.phase_for(&identify.link_id) {
            None => return Err(IdentifyRejection::NoSuchLink),
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                return Err(IdentifyRejection::LinkNotActive);
            }
            Some(LinkPhase::Active {
                role: LinkRole::Responder { .. },
                ..
            }) => return Err(IdentifyRejection::NotInitiator),
            Some(LinkPhase::Active {
                role: LinkRole::Initiator { .. },
                ..
            }) => {}
        }
        let identity = self
            .held_identities
            .get(&identify.identity)
            .ok_or(IdentifyRejection::IdentityNotHeld)?;
        let public_keys = identity.public_key_bytes();
        Ok(IdentifySignOwed {
            command_id,
            identify,
            iv,
            public_keys,
            signed_data: identify_signed_data(&identify.link_id, &public_keys),
            signing_secret: identity.signing_secret_clone(),
        })
    }

    /// Revalidates LINKIDENTIFY authority, frames the returned signature, and settles the command.
    pub fn resume_identify_sign(
        &mut self,
        completed: IdentifySignCompleted,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let IdentifySignCompleted { owed, signature } = completed;
        let mut frame = [0u8; crate::wire::BROADCAST_MTU];
        let prepared = {
            let Some(LinkPhase::Active {
                key,
                attached_interface,
                role: LinkRole::Initiator { .. },
                ..
            }) = self.links.phase_for(&owed.identify.link_id)
            else {
                return settle_identify_resume_failure(
                    sink,
                    owed.command_id,
                    IdentifyFailure::Rejected(IdentifyRejection::NoSuchLink),
                );
            };
            let Some(identity) = self.held_identities.get(&owed.identify.identity) else {
                return settle_identify_resume_failure(
                    sink,
                    owed.command_id,
                    IdentifyFailure::Rejected(IdentifyRejection::IdentityNotHeld),
                );
            };
            if identity.public_key_bytes() != owed.public_keys {
                return settle_identify_resume_failure(
                    sink,
                    owed.command_id,
                    IdentifyFailure::Rejected(IdentifyRejection::IdentityNotHeld),
                );
            }
            match write_identify_with_signature(
                &owed.identify,
                &owed.iv,
                &owed.public_keys,
                &signature,
                key,
                &mut frame,
            ) {
                Ok(wire_bytes) => Ok((*attached_interface, wire_bytes)),
                Err(_) => Err(IdentifyFailure::WriteFailed),
            }
        };
        let (target, wire_bytes) = match prepared {
            Ok(prepared) => prepared,
            Err(failure) => {
                return settle_identify_resume_failure(sink, owed.command_id, failure);
            }
        };
        self.links.note_outbound(&owed.identify.link_id, now);
        sink(EngineReaction::Directive(Directive::Send {
            target,
            bytes: &frame[..wire_bytes],
        }));
        settle(sink, owed.command_id, Settlement::Identify(Ok(())));
        WakeSchedules {
            link_deadlines: self.link_deadlines_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    /// Applies a LINKIDENTIFY verdict only while the same responder-side link is still active.
    pub fn resume_link_identity_verify(
        &mut self,
        owed: LinkIdentityVerifyOwed,
        verification: LinkIdentityVerification,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        if verification == LinkIdentityVerification::Invalid
            || !matches!(
                self.links.phase_for(&owed.link_id),
                Some(LinkPhase::Active {
                    role: LinkRole::Responder { .. },
                    ..
                })
            )
        {
            return;
        }
        self.links.note_identified(&owed.link_id, owed.identity);
        self.links.note_inbound(&owed.link_id, owed.arrived_at);
        sink(EngineReaction::Journaled(Journaled::PeerIdentified {
            link_id: owed.link_id,
            identity: owed.identity,
        }));
    }
}

fn write_identify_with_signature(
    identify: &Identify,
    iv: &[u8; 16],
    keys: &[u8; IDENTITY_PUBLIC_KEY_LEN],
    signature: &Ed25519Signature,
    key: &crate::routing::links::LinkKey,
    buf: &mut [u8],
) -> Result<usize, crate::wire::WireError> {
    let mut plaintext = [0u8; IDENTIFY_PLAINTEXT_LEN];
    plaintext[..IDENTITY_PUBLIC_KEY_LEN].copy_from_slice(keys);
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
    let header_len = header.write(buf)?;
    let sealed = key
        .seal(iv, &plaintext, &mut buf[header_len..])
        .map_err(|_| crate::wire::WireError::BufferTooShort)?;
    Ok(header_len + sealed)
}

fn settle_identify_resume_failure(
    sink: &mut impl FnMut(EngineReaction<'_>),
    command_id: CommandId,
    failure: IdentifyFailure,
) -> WakeSchedules {
    settle(sink, command_id, Settlement::Identify(Err(failure)));
    WakeSchedules::UNCHANGED
}

fn identify_signed_data(
    link_id: &LinkId,
    keys: &[u8; IDENTITY_PUBLIC_KEY_LEN],
) -> [u8; IDENTIFY_SIGNED_DATA_LEN] {
    let mut signed_data = [0u8; IDENTIFY_SIGNED_DATA_LEN];
    signed_data[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(link_id.as_bytes());
    signed_data[TRUNCATED_HASH_BYTE_LEN..].copy_from_slice(keys);
    signed_data
}

/// RNS 1.4.2 `Link.receive`'s LINKIDENTIFY arm: exact length, then the signature must cover `link_id ‖ keys` under the named keys' own signing half.
pub fn prepare_peer_identity_verify(
    link_id: &LinkId,
    plaintext: &[u8],
    arrived_at: InstantMillis,
) -> Option<LinkIdentityVerifyOwed> {
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

    let signing_key = Ed25519PublicKey(signing);
    let remote = RemoteIdentity::from_public_keys(
        IdentityEncryptionPublicKey::new(X25519PublicKey(encryption)),
        IdentitySigningPublicKey::new(signing_key),
    );
    Some(LinkIdentityVerifyOwed {
        link_id: *link_id,
        identity: remote.identity_hash(),
        signing_key,
        signed_data: identify_signed_data(link_id, keys),
        signature: Ed25519Signature(signature),
        arrived_at,
    })
}

#[deprecated(
    note = "runtime manifolds should fulfill LinkIdentityVerifyOwed and resume the engine"
)]
pub fn peer_identity_from(link_id: &LinkId, plaintext: &[u8]) -> Option<IdentityHash> {
    let owed = prepare_peer_identity_verify(link_id, plaintext, InstantMillis(0))?;
    ed25519_verify(&owed.signing_key, &owed.signed_data, &owed.signature).ok()?;
    Some(owed.identity)
}
