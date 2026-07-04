use crate::crypto::ratchets::RatchetEntropy;
use crate::engine::{
    write_announce_wire_packet, write_path_response_announce_wire_packet, EgressSerializeError,
};
use crate::engine::{AnnounceAppData, AnnounceNow};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::held::{HeldIdentities, HeldIdentityColumns, HeldIdentityRef};
use crate::identity::IdentitySigner;
use crate::routing::announce::ANNOUNCE_FIXED_FIELDS_LEN;
use crate::routing::announce::{
    Announce, AnnounceBuildError, AnnounceEntropy, AnnounceId, DottedNameHash, RatchetKey,
};
use crate::routing::upstream_app_destinations::UpstreamAppDestinationKind;
use crate::routing::upstream_app_destinations::{
    UpstreamAppDestinationColumns, UpstreamAppDestinations,
};
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, DestinationType, MDU, RATCHET_BYTE_LEN};
use heapless::Vec as HeaplessVec;

/// The wire maximum for our own announce's app data: the packet budget [`MDU`] (worst-case
/// header and minimum IFAC reserved, so a relayed copy still fits) minus the announce's fixed fields.
pub const MAX_ANNOUNCE_APP_DATA_LEN: usize = MDU - ANNOUNCE_FIXED_FIELDS_LEN;
pub const MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN: usize = MAX_ANNOUNCE_APP_DATA_LEN - RATCHET_BYTE_LEN;

pub type AnnounceAppDataBytes = HeaplessVec<u8, MAX_ANNOUNCE_APP_DATA_LEN>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAnnounceError {
    NotRegisteredAsSingle,
    IdentityNotHeld,
    Build(AnnounceBuildError),
    Serialize(EgressSerializeError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceRejection {
    NotRegisteredAsSingle,
    IdentityNotHeld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceWriteFailure {
    Build(AnnounceBuildError),
    Serialize(EgressSerializeError),
}

impl From<AnnounceRejection> for WriteAnnounceError {
    fn from(rejection: AnnounceRejection) -> Self {
        match rejection {
            AnnounceRejection::NotRegisteredAsSingle => Self::NotRegisteredAsSingle,
            AnnounceRejection::IdentityNotHeld => Self::IdentityNotHeld,
        }
    }
}

impl From<AnnounceWriteFailure> for WriteAnnounceError {
    fn from(failure: AnnounceWriteFailure) -> Self {
        match failure {
            AnnounceWriteFailure::Build(error) => Self::Build(error),
            AnnounceWriteFailure::Serialize(error) => Self::Serialize(error),
        }
    }
}

#[must_use]
pub enum CommandedAnnounceWriteOutcome {
    Written { len: usize },
    Rejected { rejection: AnnounceRejection },
    Failed { failure: AnnounceWriteFailure },
}

#[cfg(test)]
impl CommandedAnnounceWriteOutcome {
    #[track_caller]
    pub(crate) fn written_len(self) -> usize {
        match self {
            CommandedAnnounceWriteOutcome::Written { len } => len,
            _ => panic!("expected a written commanded announce"),
        }
    }
}

#[must_use]
pub enum PathResponseWriteOutcome {
    Written { wire_len: usize },
    NotLocal,
    Failed { failure: AnnounceWriteFailure },
}

/// The only two announces we frame: identical signed bodies differing only in the wire
/// context byte. A dedicated pair keeps the other context values unrepresentable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnounceContext {
    Announcement,
    PathResponse,
}

struct AnnounceContent<'a> {
    name_hash: DottedNameHash,
    app_data: &'a [u8],
    maybe_ratchet: Option<RatchetKey>,
}

fn frame_announce(
    signer: &impl IdentitySigner,
    content: &AnnounceContent<'_>,
    now: InstantMillis,
    announce_entropy: AnnounceEntropy,
    context: AnnounceContext,
    buf: &mut [u8],
) -> Result<usize, AnnounceWriteFailure> {
    let announce = Announce::build_signed(
        signer,
        content.name_hash,
        AnnounceId::mint(announce_entropy, now),
        content.maybe_ratchet,
        content.app_data,
    )
    .map_err(AnnounceWriteFailure::Build)?;
    let framed = match context {
        AnnounceContext::Announcement => write_announce_wire_packet(&announce, 0, buf),
        AnnounceContext::PathResponse => {
            write_path_response_announce_wire_packet(&announce, 0, buf)
        }
    };
    framed.map_err(AnnounceWriteFailure::Serialize)
}

impl<S: StorageLayout> EngineState<S> {
    pub fn write_commanded_announce(
        &mut self,
        commanded: &AnnounceNow,
        now: InstantMillis,
        announce_entropy: AnnounceEntropy,
        ratchet: RatchetEntropy,
        buf: &mut [u8],
    ) -> CommandedAnnounceWriteOutcome {
        use CommandedAnnounceWriteOutcome::{Failed, Rejected, Written};

        let destination = commanded.destination;

        let (name_hash, identity) = match resolve_announce_signer(
            &self.upstream_app_destinations,
            &self.held_identities,
            &destination,
        ) {
            Ok(resolved) => resolved,
            Err(rejection) => {
                return Rejected { rejection };
            }
        };

        let app_data = match &commanded.app_data {
            AnnounceAppData::Registered => self
                .upstream_app_destinations
                .app_data_for(&destination)
                .unwrap_or(&[]),
            AnnounceAppData::Data(data) => data,
        };
        self.self_ratchets.rotate_if_due(&destination, now, ratchet);
        let maybe_ratchet = self.self_ratchets.newest_ratchet_key(&destination);
        let framed = frame_announce(
            &identity,
            &AnnounceContent {
                name_hash,
                app_data,
                maybe_ratchet,
            },
            now,
            announce_entropy,
            AnnounceContext::Announcement,
            buf,
        );
        match framed {
            Ok(len) => Written { len },
            Err(failure) => Failed { failure },
        }
    }

    /// Answer a path request for one of our own upstream destinations; RNS 1.3.5
    /// `Destination.announce(path_response=True)`.
    pub fn write_path_response_announce(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
        announce_entropy: AnnounceEntropy,
        buf: &mut [u8],
    ) -> PathResponseWriteOutcome {
        let (name_hash, identity) = match resolve_announce_signer(
            &self.upstream_app_destinations,
            &self.held_identities,
            destination,
        ) {
            Ok(resolved) => resolved,
            Err(_) => return PathResponseWriteOutcome::NotLocal,
        };

        let app_data = self
            .upstream_app_destinations
            .app_data_for(destination)
            .unwrap_or(&[]);
        let maybe_ratchet = self.self_ratchets.newest_ratchet_key(destination);
        match frame_announce(
            &identity,
            &AnnounceContent {
                name_hash,
                app_data,
                maybe_ratchet,
            },
            now,
            announce_entropy,
            AnnounceContext::PathResponse,
            buf,
        ) {
            Ok(wire_len) => PathResponseWriteOutcome::Written { wire_len },
            Err(failure) => PathResponseWriteOutcome::Failed { failure },
        }
    }
}

fn resolve_announce_signer<'held, U, H>(
    upstream_app_destinations: &UpstreamAppDestinations<U>,
    held_identities: &'held HeldIdentities<H>,
    destination: &DestinationHash,
) -> Result<(DottedNameHash, HeldIdentityRef<'held>), AnnounceRejection>
where
    U: UpstreamAppDestinationColumns,
    H: HeldIdentityColumns,
{
    let registered = upstream_app_destinations
        .lookup(destination, DestinationType::Single)
        .ok_or(AnnounceRejection::NotRegisteredAsSingle)?;
    let UpstreamAppDestinationKind::Single { identity, .. } = registered.kind else {
        return Err(AnnounceRejection::NotRegisteredAsSingle);
    };
    let identity = held_identities
        .get(&identity)
        .ok_or(AnnounceRejection::IdentityNotHeld)?;
    Ok((registered.name_hash, identity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        personal_node_announcer, personal_node_destination, TEST_ANNOUNCE_ENTROPY,
        TEST_RATCHET_ENTROPY,
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
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
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
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
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
            TEST_ANNOUNCE_ENTROPY,
            TEST_RATCHET_ENTROPY,
            &mut buf,
        );
        assert!(matches!(
            outcome,
            CommandedAnnounceWriteOutcome::Rejected {
                rejection: AnnounceRejection::NotRegisteredAsSingle,
                ..
            }
        ));
    }

    #[test]
    fn a_path_response_answers_with_the_registered_app_data() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; BROADCAST_MTU];
        let outcome = state.write_path_response_announce(
            &personal_node_destination(),
            InstantMillis(1_000),
            TEST_ANNOUNCE_ENTROPY,
            &mut buf,
        );
        let PathResponseWriteOutcome::Written { wire_len } = outcome else {
            panic!("expected a written path response");
        };
        assert!(buf[..wire_len].ends_with(REGISTERED_APP_DATA));
    }

    #[test]
    fn a_path_response_for_a_foreign_destination_is_not_local() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; BROADCAST_MTU];
        let outcome = state.write_path_response_announce(
            &DestinationHash::new([0x9e; 16]),
            InstantMillis(1_000),
            TEST_ANNOUNCE_ENTROPY,
            &mut buf,
        );
        assert!(matches!(outcome, PathResponseWriteOutcome::NotLocal));
    }
}
