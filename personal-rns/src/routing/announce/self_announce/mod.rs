use crate::crypto::ratchets::{RatchetEntropy, RatchetRotation};
use crate::engine::commands::{AnnounceAppData, AnnounceNow};
use crate::engine::egress::{
    write_announce_wire_packet, write_path_response_announce_wire_packet, EgressSerializeError,
};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::held::{HeldIdentities, HeldIdentityColumns, HeldIdentityRef};
use crate::identity::IdentitySigner;
use crate::routing::announce::ANNOUNCE_FIXED_FIELDS_LEN;
use crate::routing::announce::{
    Announce, AnnounceBuildError, AnnounceId, DottedNameHash, RatchetKey, SelfAnnounceEntropy,
};
use crate::routing::storage::EngineStorage;
use crate::routing::upstream_app_destinations::UpstreamAppDestinationKind;
use crate::routing::upstream_app_destinations::{
    UpstreamAppDestinationColumns, UpstreamAppDestinations,
};
use crate::wire::{DestinationHash, DestinationType, MDU, RATCHET_LEN};
use heapless::Vec as HeaplessVec;

/// The actual wire maximum for our own announce's app data: the packet budget
/// ([`MDU`] — worst-case header and minimum IFAC already reserved, so a relayed
/// copy still fits) minus the announce's fixed fields.
pub const MAX_SELF_ANNOUNCE_APP_DATA_LEN: usize = MDU - ANNOUNCE_FIXED_FIELDS_LEN;
pub const MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN: usize =
    MAX_SELF_ANNOUNCE_APP_DATA_LEN - RATCHET_LEN;

pub type SelfAnnounceAppData = HeaplessVec<u8, MAX_SELF_ANNOUNCE_APP_DATA_LEN>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteSelfAnnounceError {
    NotRegisteredAsSingle,
    IdentityNotHeld,
    Build(AnnounceBuildError),
    Serialize(EgressSerializeError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfAnnounceRejection {
    NotRegisteredAsSingle,
    IdentityNotHeld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfAnnounceWriteFailure {
    Build(AnnounceBuildError),
    Serialize(EgressSerializeError),
}

impl From<SelfAnnounceRejection> for WriteSelfAnnounceError {
    fn from(rejection: SelfAnnounceRejection) -> Self {
        match rejection {
            SelfAnnounceRejection::NotRegisteredAsSingle => Self::NotRegisteredAsSingle,
            SelfAnnounceRejection::IdentityNotHeld => Self::IdentityNotHeld,
        }
    }
}

impl From<SelfAnnounceWriteFailure> for WriteSelfAnnounceError {
    fn from(failure: SelfAnnounceWriteFailure) -> Self {
        match failure {
            SelfAnnounceWriteFailure::Build(error) => Self::Build(error),
            SelfAnnounceWriteFailure::Serialize(error) => Self::Serialize(error),
        }
    }
}

#[must_use]
pub enum CommandedAnnounceWriteOutcome {
    Written {
        len: usize,
        rotation: RatchetRotation,
    },
    Rejected {
        rejection: SelfAnnounceRejection,
        unspent_self_announce: SelfAnnounceEntropy,
        unspent_ratchet: RatchetEntropy,
    },
    Failed {
        failure: SelfAnnounceWriteFailure,
        rotation: RatchetRotation,
    },
}

#[cfg(test)]
impl CommandedAnnounceWriteOutcome {
    #[track_caller]
    pub(crate) fn written_len(self) -> usize {
        match self {
            CommandedAnnounceWriteOutcome::Written { len, .. } => len,
            _ => panic!("expected a written commanded announce"),
        }
    }
}

#[must_use]
pub enum PathResponseWriteOutcome {
    Written { wire_len: usize },
    NotLocal,
    Failed { failure: SelfAnnounceWriteFailure },
}

/// The only two announces we frame: a normal announcement, and a path response
/// answering a request. Identical signed bodies; they differ only in the wire
/// context byte. A dedicated pair keeps the other context values unrepresentable
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnounceContext {
    Announcement,
    PathResponse,
}

#[allow(clippy::too_many_arguments)]
fn frame_announce(
    signer: &impl IdentitySigner,
    name_hash: DottedNameHash,
    app_data: &[u8],
    now: InstantMillis,
    self_announce_entropy: SelfAnnounceEntropy,
    maybe_ratchet: Option<RatchetKey>,
    context: AnnounceContext,
    buf: &mut [u8],
) -> Result<usize, SelfAnnounceWriteFailure> {
    let announce = Announce::build_signed(
        signer,
        name_hash,
        AnnounceId::mint(self_announce_entropy, now),
        maybe_ratchet,
        app_data,
    )
    .map_err(SelfAnnounceWriteFailure::Build)?;
    let framed = match context {
        AnnounceContext::Announcement => write_announce_wire_packet(&announce, 0, buf),
        AnnounceContext::PathResponse => {
            write_path_response_announce_wire_packet(&announce, 0, buf)
        }
    };
    framed.map_err(SelfAnnounceWriteFailure::Serialize)
}

impl<S: EngineStorage> EngineState<S> {
    pub fn write_commanded_announce(
        &mut self,
        commanded: &AnnounceNow,
        now: InstantMillis,
        self_announce_entropy: SelfAnnounceEntropy,
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
                return Rejected {
                    rejection,
                    unspent_self_announce: self_announce_entropy,
                    unspent_ratchet: ratchet,
                };
            }
        };

        let app_data = match &commanded.app_data {
            AnnounceAppData::Registered => self
                .upstream_app_destinations
                .app_data_for(&destination)
                .unwrap_or(&[]),
            AnnounceAppData::Data(data) => data,
        };
        let rotation = self.self_ratchets.rotate_if_due(&destination, now, ratchet);
        let maybe_ratchet = self.self_ratchets.newest_ratchet_key(&destination);
        let framed = frame_announce(
            &identity,
            name_hash,
            app_data,
            now,
            self_announce_entropy,
            maybe_ratchet,
            AnnounceContext::Announcement,
            buf,
        );
        match framed {
            Ok(len) => Written { len, rotation },
            Err(failure) => Failed { failure, rotation },
        }
    }

    /// Answer a path request for one of our own self-or-upstream destinations; RNS 1.3.1
    /// `Destination.announce(path_response=True)`.
    pub fn write_path_response_announce(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
        self_announce_entropy: SelfAnnounceEntropy,
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
            name_hash,
            app_data,
            now,
            self_announce_entropy,
            maybe_ratchet,
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
) -> Result<(DottedNameHash, HeldIdentityRef<'held>), SelfAnnounceRejection>
where
    U: UpstreamAppDestinationColumns,
    H: HeldIdentityColumns,
{
    let registered = upstream_app_destinations
        .lookup(destination, DestinationType::Single)
        .ok_or(SelfAnnounceRejection::NotRegisteredAsSingle)?;
    let UpstreamAppDestinationKind::Single { identity, .. } = registered.kind else {
        return Err(SelfAnnounceRejection::NotRegisteredAsSingle);
    };
    let identity = held_identities
        .get(&identity)
        .ok_or(SelfAnnounceRejection::IdentityNotHeld)?;
    Ok((registered.name_hash, identity))
}
