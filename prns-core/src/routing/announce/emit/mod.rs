use crate::crypto::ratchets::RatchetRotation;
use crate::crypto::{ed25519_sign, Ed25519SecretKey, Ed25519Signature};
use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::held::{HeldIdentities, HeldIdentityRef, HeldIdentityTable};
use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
use crate::interfaces::InterfaceId;
use crate::routing::announce::wire::write_originated_announce_from_signed_material;
use crate::routing::announce::ANNOUNCE_FIXED_FIELDS_LEN;
use crate::routing::announce::{
    write_announce_wire_packet, write_path_response_announce_wire_packet, Announce,
    AnnounceBuildError, AnnounceEntropy, AnnounceId, DottedNameHash, RatchetKey,
};
use crate::routing::upstream_app_destinations::UpstreamAppDestinationKind;
use crate::routing::upstream_app_destinations::{
    UpstreamAppDestinationTable, UpstreamAppDestinations,
};
use crate::storage::StorageLayout;
use crate::wire::{
    DestinationHash, WireError, BROADCAST_MDU, BROADCAST_MTU, RATCHET_BYTE_LEN, SIGNATURE_BYTE_LEN,
    TRUNCATED_HASH_BYTE_LEN,
};
use heapless::Vec as HeaplessVec;

/// The wire maximum for our own announce's app data: the packet budget [`BROADCAST_MDU`] (worst-case header and minimum IFAC reserved, so a relayed copy still fits) minus the announce's fixed fields.
pub const MAX_ANNOUNCE_APP_DATA_LEN: usize = BROADCAST_MDU - ANNOUNCE_FIXED_FIELDS_LEN;
pub const MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN: usize = MAX_ANNOUNCE_APP_DATA_LEN - RATCHET_BYTE_LEN;

pub type AnnounceAppDataBytes = HeaplessVec<u8, MAX_ANNOUNCE_APP_DATA_LEN>;
pub const MAX_ANNOUNCE_SIGNED_MATERIAL_LEN: usize = TRUNCATED_HASH_BYTE_LEN + BROADCAST_MTU;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceSignPurpose {
    Command {
        command_id: CommandId,
        target: AnnounceTarget,
    },
    PathResponse {
        target: InterfaceId,
    },
}

#[repr(C)]
pub struct AnnounceSignOwed {
    pub purpose: AnnounceSignPurpose,
    pub destination: DestinationHash,
    pub identity: IdentityHash,
    pub public_keys: IdentityPublicKeys,
    pub dotted_name_hash: DottedNameHash,
    pub ratchet_rotation: RatchetRotation,
    pub has_ratchet: bool,
    pub fields_before_signature: usize,
    pub signed_material_len: usize,
    pub signed_material: [u8; MAX_ANNOUNCE_SIGNED_MATERIAL_LEN],
    pub signing_secret: Ed25519SecretKey,
}

#[repr(C)]
pub struct AnnounceSignCompleted {
    pub owed: AnnounceSignOwed,
    pub signature: Ed25519Signature,
}

impl AnnounceSignOwed {
    #[must_use]
    pub fn fulfill(self) -> AnnounceSignCompleted {
        let signature = ed25519_sign(
            &self.signing_secret,
            &self.signed_material[..self.signed_material_len],
        );
        AnnounceSignCompleted {
            owed: self,
            signature,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OriginatedAnnounceDispatch {
    pub purpose: AnnounceSignPurpose,
    pub destination: DestinationHash,
    pub ratchet_rotation: RatchetRotation,
    pub wire_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceRejection {
    NotRegistered,
    NotSingle,
    IdentityNotHeld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceWriteError {
    Build(AnnounceBuildError),
    Serialize(WireError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceWriteFailure {
    Rejected(AnnounceRejection),
    Errored(AnnounceWriteError),
}

#[must_use]
pub enum CommandedAnnounceWriteOutcome {
    Written {
        wire_bytes: usize,
        ratchet_rotation: RatchetRotation,
    },
    Rejected {
        rejection: AnnounceRejection,
    },
    Failed {
        failure: AnnounceWriteError,
    },
}

#[must_use]
pub enum PathResponseWriteOutcome {
    Written {
        wire_bytes: usize,
        ratchet_rotation: RatchetRotation,
    },
    NotUpstream,
    Failed {
        failure: AnnounceWriteError,
    },
}

/// The only two announces we frame. Identical signed bodies differing only in the wire context byte.
/// A dedicated pair keeps the other context values unrepresentable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnounceContext {
    Announcement,
    PathResponse,
}

struct AnnounceContent<'a> {
    name_hash: DottedNameHash,
    app_data: &'a [u8],
    ratchet: Option<RatchetKey>,
}

fn frame_announce(
    signer: &impl IdentitySigner,
    content: &AnnounceContent<'_>,
    now: InstantMillis,
    announce_entropy: AnnounceEntropy,
    context: AnnounceContext,
    buf: &mut [u8],
) -> Result<usize, AnnounceWriteError> {
    let announce = Announce::build_signed(
        signer,
        content.name_hash,
        AnnounceId::mint(announce_entropy, now),
        content.ratchet,
        content.app_data,
    )
    .map_err(AnnounceWriteError::Build)?;

    let framed = match context {
        AnnounceContext::Announcement => write_announce_wire_packet(&announce, 0, buf),
        AnnounceContext::PathResponse => {
            write_path_response_announce_wire_packet(&announce, 0, buf)
        }
    };
    framed.map_err(AnnounceWriteError::Serialize)
}

impl<S: StorageLayout> EngineState<S> {
    pub fn prepare_commanded_announce_sign(
        &mut self,
        command_id: CommandId,
        commanded: &AnnounceNow,
        now: InstantMillis,
        fill_random: &mut impl FnMut(&mut [u8]),
    ) -> Result<AnnounceSignOwed, AnnounceWriteFailure> {
        self.prepare_upstream_announce_sign(
            &commanded.destination,
            &commanded.app_data,
            now,
            fill_random,
            AnnounceSignPurpose::Command {
                command_id,
                target: commanded.target,
            },
        )
    }

    pub fn prepare_path_response_announce_sign(
        &mut self,
        destination: &DestinationHash,
        target: InterfaceId,
        now: InstantMillis,
        fill_random: &mut impl FnMut(&mut [u8]),
    ) -> Result<AnnounceSignOwed, AnnounceWriteFailure> {
        self.prepare_upstream_announce_sign(
            destination,
            &AnnounceAppData::Registered,
            now,
            fill_random,
            AnnounceSignPurpose::PathResponse { target },
        )
    }

    fn prepare_upstream_announce_sign(
        &mut self,
        destination: &DestinationHash,
        app_data: &AnnounceAppData,
        now: InstantMillis,
        fill_random: &mut impl FnMut(&mut [u8]),
        purpose: AnnounceSignPurpose,
    ) -> Result<AnnounceSignOwed, AnnounceWriteFailure> {
        let (name_hash, identity, registered_app_data) = resolve_announce_signer(
            &self.upstream_app_destinations,
            &self.held_identities,
            destination,
        )
        .map_err(AnnounceWriteFailure::Rejected)?;
        let app_data = match app_data {
            AnnounceAppData::Registered => registered_app_data,
            AnnounceAppData::Data(data) => data,
        };
        let ratchet_rotation = self
            .self_ratchets
            .rotate_if_due(destination, now, fill_random);
        let ratchet = self.self_ratchets.newest_ratchet_key(destination);
        let mut announce_entropy = [0u8; AnnounceEntropy::LEN];
        fill_random(&mut announce_entropy);
        let public_keys = IdentityPublicKeys {
            encryption: identity.encryption_public_key(),
            signing: identity.signing_public_key(),
        };
        let unsigned = Announce {
            destination: *destination,
            public_keys,
            dotted_name_hash: name_hash,
            announce_id: AnnounceId::mint(AnnounceEntropy::new(announce_entropy), now),
            ratchet,
            signature: Ed25519Signature([0; SIGNATURE_BYTE_LEN]),
            app_data,
        };
        let mut signed_material = [0u8; MAX_ANNOUNCE_SIGNED_MATERIAL_LEN];
        let signed_material_len = unsigned
            .write_signed_material(&mut signed_material)
            .map_err(|_| {
                AnnounceWriteFailure::Errored(AnnounceWriteError::Build(
                    AnnounceBuildError::AnnounceTooLarge,
                ))
            })?;
        Ok(AnnounceSignOwed {
            purpose,
            destination: *destination,
            identity: identity.identity_hash(),
            public_keys,
            dotted_name_hash: name_hash,
            ratchet_rotation,
            has_ratchet: ratchet.is_some(),
            fields_before_signature: unsigned.wire_bytes() - SIGNATURE_BYTE_LEN - app_data.len(),
            signed_material_len,
            signed_material,
            signing_secret: identity.signing_secret_clone(),
        })
    }

    pub fn finish_announce_sign(
        &self,
        completed: AnnounceSignCompleted,
        buf: &mut [u8],
    ) -> Result<OriginatedAnnounceDispatch, AnnounceWriteFailure> {
        let AnnounceSignCompleted { owed, signature } = completed;
        let (name_hash, identity, _) = resolve_announce_signer(
            &self.upstream_app_destinations,
            &self.held_identities,
            &owed.destination,
        )
        .map_err(AnnounceWriteFailure::Rejected)?;
        if identity.identity_hash() != owed.identity
            || identity.public_key_bytes() != owed.public_keys.public_key_bytes()
            || name_hash != owed.dotted_name_hash
        {
            return Err(AnnounceWriteFailure::Rejected(
                AnnounceRejection::IdentityNotHeld,
            ));
        }
        let wire_bytes = write_originated_announce_from_signed_material(
            owed.destination,
            owed.has_ratchet,
            matches!(owed.purpose, AnnounceSignPurpose::PathResponse { .. }),
            &owed.signed_material[..owed.signed_material_len],
            owed.fields_before_signature,
            &signature,
            buf,
        )
        .map_err(|error| AnnounceWriteFailure::Errored(AnnounceWriteError::Serialize(error)))?;
        Ok(OriginatedAnnounceDispatch {
            purpose: owed.purpose,
            destination: owed.destination,
            ratchet_rotation: owed.ratchet_rotation,
            wire_bytes,
        })
    }

    pub fn write_commanded_announce(
        &mut self,
        commanded: &AnnounceNow,
        now: InstantMillis,
        fill_random: &mut impl FnMut(&mut [u8]),
        buf: &mut [u8],
    ) -> CommandedAnnounceWriteOutcome {
        use CommandedAnnounceWriteOutcome::{Failed, Rejected, Written};

        match self.write_upstream_announce(
            &commanded.destination,
            &commanded.app_data,
            now,
            fill_random,
            AnnounceContext::Announcement,
            buf,
        ) {
            Ok((wire_bytes, ratchet_rotation)) => Written {
                wire_bytes,
                ratchet_rotation,
            },
            Err(AnnounceWriteFailure::Rejected(rejection)) => Rejected { rejection },
            Err(AnnounceWriteFailure::Errored(failure)) => Failed { failure },
        }
    }

    /// Answer a path request for one of our own upstream destinations; RNS 1.4.2 `Destination.announce(path_response=True)`.
    /// Path responses for foreign tracked destinations re-emit the retained announce instead, over in the scheduled-announce lane.
    pub fn write_path_response_for_upstream(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
        fill_random: &mut impl FnMut(&mut [u8]),
        buf: &mut [u8],
    ) -> PathResponseWriteOutcome {
        use PathResponseWriteOutcome::{Failed, NotUpstream, Written};

        match self.write_upstream_announce(
            destination,
            &AnnounceAppData::Registered,
            now,
            fill_random,
            AnnounceContext::PathResponse,
            buf,
        ) {
            Ok((wire_bytes, ratchet_rotation)) => Written {
                wire_bytes,
                ratchet_rotation,
            },
            Err(AnnounceWriteFailure::Rejected(_)) => NotUpstream,
            Err(AnnounceWriteFailure::Errored(failure)) => Failed { failure },
        }
    }

    fn write_upstream_announce(
        &mut self,
        destination: &DestinationHash,
        app_data: &AnnounceAppData,
        now: InstantMillis,
        fill_random: &mut impl FnMut(&mut [u8]),
        context: AnnounceContext,
        buf: &mut [u8],
    ) -> Result<(usize, RatchetRotation), AnnounceWriteFailure> {
        let (name_hash, identity, registered_app_data) = resolve_announce_signer(
            &self.upstream_app_destinations,
            &self.held_identities,
            destination,
        )
        .map_err(AnnounceWriteFailure::Rejected)?;

        let app_data = match app_data {
            AnnounceAppData::Registered => registered_app_data,
            AnnounceAppData::Data(data) => data,
        };

        let ratchet_rotation = self
            .self_ratchets
            .rotate_if_due(destination, now, fill_random);
        let ratchet = self.self_ratchets.newest_ratchet_key(destination);

        let mut announce_entropy_bytes = [0u8; AnnounceEntropy::LEN];
        fill_random(&mut announce_entropy_bytes);
        let wire_bytes = frame_announce(
            &identity,
            &AnnounceContent {
                name_hash,
                app_data,
                ratchet,
            },
            now,
            AnnounceEntropy::new(announce_entropy_bytes),
            context,
            buf,
        )
        .map_err(AnnounceWriteFailure::Errored)?;
        Ok((wire_bytes, ratchet_rotation))
    }
}

fn resolve_announce_signer<'held, 'reg, U, H>(
    upstream_app_destinations: &'reg UpstreamAppDestinations<U>,
    held_identities: &'held HeldIdentities<H>,
    destination: &DestinationHash,
) -> Result<(DottedNameHash, HeldIdentityRef<'held>, &'reg [u8]), AnnounceRejection>
where
    U: UpstreamAppDestinationTable,
    H: HeldIdentityTable,
{
    let (registered, app_data) = upstream_app_destinations
        .registration_for(destination)
        .ok_or(AnnounceRejection::NotRegistered)?;

    let UpstreamAppDestinationKind::Single { identity, .. } = registered.kind else {
        return Err(AnnounceRejection::NotSingle);
    };

    let identity = held_identities
        .get(&identity)
        .ok_or(AnnounceRejection::IdentityNotHeld)?;

    Ok((registered.name_hash, identity, app_data))
}

#[cfg(test)]
mod tests {
    impl CommandedAnnounceWriteOutcome {
        #[track_caller]
        pub(crate) fn written_len(self) -> usize {
            match self {
                CommandedAnnounceWriteOutcome::Written { wire_bytes, .. } => wire_bytes,
                _ => panic!("expected a written commanded announce"),
            }
        }
    }
    use super::*;
    use crate::engine::test_support::{
        personal_node_announcer, personal_node_destination, test_fill_entropy,
    };
    use crate::engine::AnnounceTarget;
    use crate::wire::{DestinationHash, BROADCAST_MTU};

    const REGISTERED_APP_DATA: &[u8] = b"hello-personal";

    fn commanded(destination: DestinationHash, app_data: AnnounceAppData) -> AnnounceNow {
        AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data,
        }
    }

    #[test]
    fn continued_announce_signing_is_byte_identical_to_the_inline_writer() {
        let command = commanded(
            personal_node_destination(),
            AnnounceAppData::Data(AnnounceAppDataBytes::from_slice(b"continued").unwrap()),
        );
        let now = InstantMillis(4_200);
        let mut inline = personal_node_announcer();
        let mut continued = personal_node_announcer();
        let mut inline_wire = [0u8; BROADCAST_MTU];
        let mut continued_wire = [0u8; BROADCAST_MTU];
        let inline_outcome = inline.write_commanded_announce(
            &command,
            now,
            &mut |bytes| bytes.fill(0x7A),
            &mut inline_wire,
        );
        let CommandedAnnounceWriteOutcome::Written {
            wire_bytes: inline_bytes,
            ratchet_rotation: inline_rotation,
        } = inline_outcome
        else {
            panic!("inline announce must write");
        };
        let completed = continued
            .prepare_commanded_announce_sign(CommandId(17), &command, now, &mut |bytes| {
                bytes.fill(0x7A)
            })
            .unwrap()
            .fulfill();
        let dispatch = continued
            .finish_announce_sign(completed, &mut continued_wire)
            .unwrap();

        assert_eq!(dispatch.wire_bytes, inline_bytes);
        assert_eq!(dispatch.ratchet_rotation, inline_rotation);
        assert_eq!(
            continued_wire[..dispatch.wire_bytes],
            inline_wire[..inline_bytes]
        );
    }

    #[test]
    fn a_commanded_announce_carries_the_registered_app_data() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; BROADCAST_MTU];
        let len = state
            .write_commanded_announce(
                &commanded(personal_node_destination(), AnnounceAppData::Registered),
                InstantMillis(1_000),
                &mut test_fill_entropy,
                &mut buf,
            )
            .written_len();
        assert!(buf[..len].ends_with(REGISTERED_APP_DATA));
    }

    #[test]
    fn a_commanded_data_payload_overrides_the_registered_app_data() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; BROADCAST_MTU];
        let override_data = AnnounceAppDataBytes::from_slice(b"override-data").unwrap();
        let len = state
            .write_commanded_announce(
                &commanded(
                    personal_node_destination(),
                    AnnounceAppData::Data(override_data),
                ),
                InstantMillis(1_000),
                &mut test_fill_entropy,
                &mut buf,
            )
            .written_len();
        assert!(buf[..len].ends_with(b"override-data"));
        assert!(!buf[..len].ends_with(REGISTERED_APP_DATA));
    }

    #[test]
    fn a_commanded_announce_for_an_unregistered_destination_is_rejected() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; BROADCAST_MTU];
        let outcome = state.write_commanded_announce(
            &commanded(
                DestinationHash::new([0x9e; 16]),
                AnnounceAppData::Registered,
            ),
            InstantMillis(1_000),
            &mut test_fill_entropy,
            &mut buf,
        );
        assert!(matches!(
            outcome,
            CommandedAnnounceWriteOutcome::Rejected {
                rejection: AnnounceRejection::NotRegistered,
                ..
            }
        ));
    }

    #[test]
    fn a_path_response_answers_with_the_registered_app_data() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; BROADCAST_MTU];
        let outcome = state.write_path_response_for_upstream(
            &personal_node_destination(),
            InstantMillis(1_000),
            &mut test_fill_entropy,
            &mut buf,
        );
        let PathResponseWriteOutcome::Written { wire_bytes, .. } = outcome else {
            panic!("expected a written path response");
        };
        assert!(buf[..wire_bytes].ends_with(REGISTERED_APP_DATA));
    }

    #[test]
    fn a_path_response_for_a_foreign_destination_is_not_upstream() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; BROADCAST_MTU];
        let outcome = state.write_path_response_for_upstream(
            &DestinationHash::new([0x9e; 16]),
            InstantMillis(1_000),
            &mut test_fill_entropy,
            &mut buf,
        );
        assert!(matches!(outcome, PathResponseWriteOutcome::NotUpstream));
    }

    #[test]
    fn a_path_response_rotates_the_ratchet_exactly_like_a_commanded_announce() {
        use crate::engine::test_support::personal_node_announcer_with;
        use crate::engine::RatchetPolicy;

        let mut state = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let destination = personal_node_destination();
        assert_eq!(state.self_ratchets.newest_ratchet_key(&destination), None);

        let mut buf = [0u8; BROADCAST_MTU];
        let outcome = state.write_path_response_for_upstream(
            &destination,
            InstantMillis(1_000),
            &mut test_fill_entropy,
            &mut buf,
        );
        assert!(matches!(outcome, PathResponseWriteOutcome::Written { .. }));
        assert!(
            state
                .self_ratchets
                .newest_ratchet_key(&destination)
                .is_some(),
            "a due rotation must ride the path response, exactly as the reference rotates inside announce()",
        );
    }
}
