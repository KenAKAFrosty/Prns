use crate::crypto::ratchets::RatchetRotation;
use crate::engine::{
    write_announce_wire_packet, write_path_response_announce_wire_packet, EgressSerializeError,
};
use crate::engine::{AnnounceAppData, AnnounceNow};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::held::{HeldIdentities, HeldIdentityRef, HeldIdentityTable};
use crate::identity::IdentitySigner;
use crate::routing::announce::ANNOUNCE_FIXED_FIELDS_LEN;
use crate::routing::announce::{
    Announce, AnnounceBuildError, AnnounceEntropy, AnnounceId, DottedNameHash, RatchetKey,
};
use crate::routing::upstream_app_destinations::UpstreamAppDestinationKind;
use crate::routing::upstream_app_destinations::{
    UpstreamAppDestinationTable, UpstreamAppDestinations,
};
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, BROADCAST_MDU, RATCHET_BYTE_LEN};
use heapless::Vec as HeaplessVec;

/// The wire maximum for our own announce's app data: the packet budget [`BROADCAST_MDU`] (worst-case header and minimum IFAC reserved, so a relayed copy still fits) minus the announce's fixed fields.
pub const MAX_ANNOUNCE_APP_DATA_LEN: usize = BROADCAST_MDU - ANNOUNCE_FIXED_FIELDS_LEN;
pub const MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN: usize = MAX_ANNOUNCE_APP_DATA_LEN - RATCHET_BYTE_LEN;

pub type AnnounceAppDataBytes = HeaplessVec<u8, MAX_ANNOUNCE_APP_DATA_LEN>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceRejection {
    NotRegistered,
    NotSingle,
    IdentityNotHeld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceWriteError {
    Build(AnnounceBuildError),
    Serialize(EgressSerializeError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceWriteFailure {
    Rejected(AnnounceRejection),
    Errored(AnnounceWriteError),
}

#[must_use]
pub enum CommandedAnnounceWriteOutcome {
    Written {
        wire_len: usize,
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
        wire_len: usize,
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
    pub fn write_commanded_announce(
        &mut self,
        commanded: &AnnounceNow,
        now: InstantMillis,
        fill_entropy: &mut impl FnMut(&mut [u8]),
        buf: &mut [u8],
    ) -> CommandedAnnounceWriteOutcome {
        use CommandedAnnounceWriteOutcome::{Failed, Rejected, Written};

        match self.write_upstream_announce(
            &commanded.destination,
            &commanded.app_data,
            now,
            fill_entropy,
            AnnounceContext::Announcement,
            buf,
        ) {
            Ok((wire_len, ratchet_rotation)) => Written {
                wire_len,
                ratchet_rotation,
            },
            Err(AnnounceWriteFailure::Rejected(rejection)) => Rejected { rejection },
            Err(AnnounceWriteFailure::Errored(failure)) => Failed { failure },
        }
    }

    /// Answer a path request for one of our own upstream destinations; RNS 1.3.5 `Destination.announce(path_response=True)`.
    /// Path responses for foreign tracked destinations re-emit the retained announce instead, over in the scheduled-announce lane.
    pub fn write_path_response_for_upstream(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
        fill_entropy: &mut impl FnMut(&mut [u8]),
        buf: &mut [u8],
    ) -> PathResponseWriteOutcome {
        use PathResponseWriteOutcome::{Failed, NotUpstream, Written};

        match self.write_upstream_announce(
            destination,
            &AnnounceAppData::Registered,
            now,
            fill_entropy,
            AnnounceContext::PathResponse,
            buf,
        ) {
            Ok((wire_len, ratchet_rotation)) => Written {
                wire_len,
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
        fill_entropy: &mut impl FnMut(&mut [u8]),
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
            .rotate_if_due(destination, now, fill_entropy);
        let ratchet = self.self_ratchets.newest_ratchet_key(destination);

        let mut announce_entropy_bytes = [0u8; AnnounceEntropy::LEN];
        fill_entropy(&mut announce_entropy_bytes);
        let wire_len = frame_announce(
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
        Ok((wire_len, ratchet_rotation))
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
                CommandedAnnounceWriteOutcome::Written { wire_len, .. } => wire_len,
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
        let PathResponseWriteOutcome::Written { wire_len, .. } = outcome else {
            panic!("expected a written path response");
        };
        assert!(buf[..wire_len].ends_with(REGISTERED_APP_DATA));
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
