mod impls;

pub use impls::*;

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

pub const DEFAULT_REANNOUNCE_INTERVAL_MS: u64 = 6 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReannounceSchedule {
    interval_millis: u64,
}

impl ReannounceSchedule {
    pub const fn every(interval_millis: u64) -> Self {
        Self { interval_millis }
    }

    pub const fn interval_millis(&self) -> u64 {
        self.interval_millis
    }
}

impl Default for ReannounceSchedule {
    fn default() -> Self {
        Self::every(DEFAULT_REANNOUNCE_INTERVAL_MS)
    }
}

pub struct AnnounceConfig<'a> {
    pub app_data: &'a [u8],
    pub schedule: ReannounceSchedule,
}

pub type SelfAnnounceAppData = HeaplessVec<u8, MAX_SELF_ANNOUNCE_APP_DATA_LEN>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleAnnounceError {
    UnknownDestination,
    NotASingleDestination,
    AppDataTooLong,
    TableFull,
}

pub trait SelfAnnounceColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn schedules(&self) -> &[ReannounceSchedule];
    fn last_announced(&self) -> &[Option<InstantMillis>];
    fn app_data_at(&self, index: usize) -> Option<&[u8]>;

    fn set_row(
        &mut self,
        index: usize,
        schedule: ReannounceSchedule,
        app_data: SelfAnnounceAppData,
    );
    fn set_last_announced(&mut self, index: usize, at: InstantMillis);

    fn push(
        &mut self,
        destination: DestinationHash,
        schedule: ReannounceSchedule,
        app_data: SelfAnnounceAppData,
    ) -> Result<usize, ScheduleAnnounceError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueAnnounce<'a> {
    pub destination: DestinationHash,
    pub app_data: &'a [u8],
}

#[derive(Debug, Default)]
pub struct SelfAnnounces<C: SelfAnnounceColumns> {
    columns: C,
}

impl<C: SelfAnnounceColumns> SelfAnnounces<C> {
    pub fn destinations(&self) -> &[DestinationHash] {
        self.columns.destinations()
    }

    pub fn schedule(
        &mut self,
        destination: DestinationHash,
        config: AnnounceConfig<'_>,
    ) -> Result<(), ScheduleAnnounceError> {
        let mut app_data = SelfAnnounceAppData::new();
        app_data
            .extend_from_slice(config.app_data)
            .map_err(|_| ScheduleAnnounceError::AppDataTooLong)?;
        match self
            .columns
            .destinations()
            .iter()
            .position(|candidate| candidate == &destination)
        {
            Some(index) => {
                self.columns.set_row(index, config.schedule, app_data);
                Ok(())
            }
            None => self
                .columns
                .push(destination, config.schedule, app_data)
                .map(|_| ()),
        }
    }

    pub fn due_announce(&self, now: InstantMillis) -> Option<DueAnnounce<'_>> {
        let index = self.due_index(now)?;
        Some(DueAnnounce {
            destination: *self.columns.destinations().get(index)?,
            app_data: self.columns.app_data_at(index)?,
        })
    }

    pub fn scheduled_app_data(&self, destination: &DestinationHash) -> Option<&[u8]> {
        let index = self
            .columns
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)?;
        self.columns.app_data_at(index)
    }

    pub fn mark_announced(&mut self, destination: &DestinationHash, now: InstantMillis) {
        if let Some(index) = self
            .columns
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
        {
            self.columns.set_last_announced(index, now);
        }
    }

    pub fn next_due_at(&self) -> Option<InstantMillis> {
        self.columns
            .last_announced()
            .iter()
            .zip(self.columns.schedules())
            .filter_map(|(last, schedule)| {
                last.map(|last| InstantMillis(last.0.saturating_add(schedule.interval_millis())))
            })
            .min()
    }

    pub fn contains(&self, destination: &DestinationHash) -> bool {
        self.columns.destinations().contains(destination)
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    fn due_index(&self, now: InstantMillis) -> Option<usize> {
        self.columns
            .last_announced()
            .iter()
            .zip(self.columns.schedules())
            .position(|(last, schedule)| match last {
                None => true,
                Some(last) => now.0.saturating_sub(last.0) >= schedule.interval_millis(),
            })
    }
}

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
pub enum DueSelfAnnounceWriteOutcome {
    NothingDue {
        unspent_self_announce: SelfAnnounceEntropy,
        unspent_ratchet: RatchetEntropy,
    },
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
    pub fn schedule_announce(
        &mut self,
        destination: &DestinationHash,
        config: AnnounceConfig<'_>,
    ) -> Result<(), ScheduleAnnounceError> {
        if self
            .upstream_app_destinations
            .lookup(destination, DestinationType::Single)
            .is_none()
        {
            return Err(
                if self
                    .upstream_app_destinations
                    .lookup(destination, DestinationType::Plain)
                    .is_some()
                {
                    ScheduleAnnounceError::NotASingleDestination
                } else {
                    ScheduleAnnounceError::UnknownDestination
                },
            );
        }
        if self.self_ratchets.is_tracked(destination)
            && config.app_data.len() > MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN
        {
            return Err(ScheduleAnnounceError::AppDataTooLong);
        }
        self.self_announces.schedule(*destination, config)
    }

    pub fn self_announced_destinations(&self) -> &[DestinationHash] {
        self.self_announces.destinations()
    }

    /// `NothingDue` is the common case, not a failure. An attempt at a due
    /// announce consumes its due-ness whatever the arm: a persistently failing
    /// announce retries next interval instead of spinning the engine's
    /// `Immediate` wakeup forever.
    ///
    /// A ratcheted destination rotates here, before the announce is framed
    /// (RNS 1.3.1 `Destination.announce` calls `rotate_ratchets` first), so
    /// the announce always carries the newest ratchet. Every arm hands back
    /// exactly the entropy whose bytes were never read: a rejected announce
    /// happens before rotation and the id mint, so both units come home; a
    /// framing failure happens after both, so only the rotation's verdict does.
    pub fn write_due_self_announce(
        &mut self,
        now: InstantMillis,
        self_announce_entropy: SelfAnnounceEntropy,
        ratchet: RatchetEntropy,
        buf: &mut [u8],
    ) -> DueSelfAnnounceWriteOutcome {
        use DueSelfAnnounceWriteOutcome::{Failed, NothingDue, Rejected, Written};

        let Some(destination) = self
            .self_announces
            .due_announce(now)
            .map(|due| due.destination)
        else {
            return NothingDue {
                unspent_self_announce: self_announce_entropy,
                unspent_ratchet: ratchet,
            };
        };
        self.self_announces.mark_announced(&destination, now);

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

        let app_data = self
            .self_announces
            .scheduled_app_data(&destination)
            .unwrap_or(&[]);
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
        self.self_announces.mark_announced(&destination, now);

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
            AnnounceAppData::Scheduled => self
                .self_announces
                .scheduled_app_data(&destination)
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
            .self_announces
            .scheduled_app_data(destination)
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

#[cfg(test)]
mod tests {
    use super::*;

    impl DueSelfAnnounceWriteOutcome {
        #[track_caller]
        pub fn written_len(self) -> usize {
            match self {
                Self::Written { len, .. } => len,
                Self::NothingDue { .. } => panic!("expected Written, got NothingDue"),
                Self::Rejected { rejection, .. } => {
                    panic!("expected Written, got Rejected({rejection:?})")
                }
                Self::Failed { failure, .. } => panic!("expected Written, got Failed({failure:?})"),
            }
        }

        #[track_caller]
        pub fn nothing_due(self) -> (SelfAnnounceEntropy, RatchetEntropy) {
            match self {
                Self::NothingDue {
                    unspent_self_announce,
                    unspent_ratchet,
                } => (unspent_self_announce, unspent_ratchet),
                Self::Written { len, .. } => panic!("expected NothingDue, got Written({len}B)"),
                Self::Rejected { rejection, .. } => {
                    panic!("expected NothingDue, got Rejected({rejection:?})")
                }
                Self::Failed { failure, .. } => {
                    panic!("expected NothingDue, got Failed({failure:?})")
                }
            }
        }

        #[track_caller]
        pub fn rejection(self) -> (SelfAnnounceRejection, SelfAnnounceEntropy, RatchetEntropy) {
            match self {
                Self::Rejected {
                    rejection,
                    unspent_self_announce,
                    unspent_ratchet,
                } => (rejection, unspent_self_announce, unspent_ratchet),
                Self::NothingDue { .. } => panic!("expected Rejected, got NothingDue"),
                Self::Written { len, .. } => panic!("expected Rejected, got Written({len}B)"),
                Self::Failed { failure, .. } => {
                    panic!("expected Rejected, got Failed({failure:?})")
                }
            }
        }

        #[track_caller]
        pub fn failure(self) -> (SelfAnnounceWriteFailure, RatchetRotation) {
            match self {
                Self::Failed { failure, rotation } => (failure, rotation),
                Self::NothingDue { .. } => panic!("expected Failed, got NothingDue"),
                Self::Written { len, .. } => panic!("expected Failed, got Written({len}B)"),
                Self::Rejected { rejection, .. } => {
                    panic!("expected Failed, got Rejected({rejection:?})")
                }
            }
        }
    }

    impl CommandedAnnounceWriteOutcome {
        #[track_caller]
        pub fn written_len(self) -> usize {
            match self {
                Self::Written { len, .. } => len,
                Self::Rejected { rejection, .. } => {
                    panic!("expected Written, got Rejected({rejection:?})")
                }
                Self::Failed { failure, .. } => panic!("expected Written, got Failed({failure:?})"),
            }
        }

        #[track_caller]
        pub fn rejection(self) -> (SelfAnnounceRejection, SelfAnnounceEntropy, RatchetEntropy) {
            match self {
                Self::Rejected {
                    rejection,
                    unspent_self_announce,
                    unspent_ratchet,
                } => (rejection, unspent_self_announce, unspent_ratchet),
                Self::Written { len, .. } => panic!("expected Rejected, got Written({len}B)"),
                Self::Failed { failure, .. } => {
                    panic!("expected Rejected, got Failed({failure:?})")
                }
            }
        }

        #[track_caller]
        pub fn failure(self) -> (SelfAnnounceWriteFailure, RatchetRotation) {
            match self {
                Self::Failed { failure, rotation } => (failure, rotation),
                Self::Written { len, .. } => panic!("expected Failed, got Written({len}B)"),
                Self::Rejected { rejection, .. } => {
                    panic!("expected Failed, got Rejected({rejection:?})")
                }
            }
        }
    }

    type TestAnnounces = SelfAnnounces<FixedSelfAnnounceColumns<2>>;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    #[test]
    fn never_announced_rows_are_due_and_marking_walks_the_table_dry() {
        let mut announces = TestAnnounces::default();
        announces
            .schedule(
                dest(1),
                AnnounceConfig {
                    app_data: b"one",
                    schedule: ReannounceSchedule::every(1_000),
                },
            )
            .unwrap();
        announces
            .schedule(
                dest(2),
                AnnounceConfig {
                    app_data: b"two",
                    schedule: ReannounceSchedule::every(2_000),
                },
            )
            .unwrap();

        assert_eq!(
            announces.due_announce(InstantMillis(0)),
            Some(DueAnnounce {
                destination: dest(1),
                app_data: b"one",
            }),
        );
        announces.mark_announced(&dest(1), InstantMillis(0));

        assert_eq!(
            announces.due_announce(InstantMillis(0)),
            Some(DueAnnounce {
                destination: dest(2),
                app_data: b"two",
            }),
        );
        announces.mark_announced(&dest(2), InstantMillis(0));

        assert_eq!(announces.due_announce(InstantMillis(0)), None);
        assert_eq!(announces.next_due_at(), Some(InstantMillis(1_000)));
        assert_eq!(
            announces.due_announce(InstantMillis(1_000)),
            Some(DueAnnounce {
                destination: dest(1),
                app_data: b"one",
            }),
        );
    }

    #[test]
    fn rescheduling_replaces_the_config_and_announces_the_new_data_promptly() {
        let mut announces = TestAnnounces::default();
        announces
            .schedule(
                dest(1),
                AnnounceConfig {
                    app_data: b"old-name",
                    schedule: ReannounceSchedule::default(),
                },
            )
            .unwrap();
        announces.mark_announced(&dest(1), InstantMillis(500));
        assert_eq!(announces.due_announce(InstantMillis(501)), None);

        announces
            .schedule(
                dest(1),
                AnnounceConfig {
                    app_data: b"new-name",
                    schedule: ReannounceSchedule::default(),
                },
            )
            .unwrap();
        assert_eq!(announces.len(), 1);
        assert_eq!(
            announces.due_announce(InstantMillis(501)),
            Some(DueAnnounce {
                destination: dest(1),
                app_data: b"new-name",
            }),
        );
    }

    #[test]
    fn overlong_app_data_and_a_full_table_report_themselves() {
        let mut announces = SelfAnnounces::<FixedSelfAnnounceColumns<1>>::default();
        let too_long = [0u8; MAX_SELF_ANNOUNCE_APP_DATA_LEN + 1];
        assert_eq!(
            announces.schedule(
                dest(1),
                AnnounceConfig {
                    app_data: &too_long,
                    schedule: ReannounceSchedule::default(),
                },
            ),
            Err(ScheduleAnnounceError::AppDataTooLong),
        );
        assert!(announces.is_empty());

        announces
            .schedule(
                dest(1),
                AnnounceConfig {
                    app_data: b"fits",
                    schedule: ReannounceSchedule::default(),
                },
            )
            .unwrap();
        assert_eq!(
            announces.schedule(
                dest(2),
                AnnounceConfig {
                    app_data: b"overflow",
                    schedule: ReannounceSchedule::default(),
                },
            ),
            Err(ScheduleAnnounceError::TableFull),
        );
        assert!(announces.contains(&dest(1)));
        assert!(!announces.contains(&dest(2)));
    }

    #[test]
    fn default_schedule_is_six_hours() {
        assert_eq!(
            ReannounceSchedule::default().interval_millis(),
            6 * 60 * 60 * 1000,
        );
    }

    use crate::engine::commands::AnnounceTarget;
    use crate::engine::test_support::*;
    use crate::engine::RatchetPolicy;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::wire::{DestinationType, PacketType, PropagationType, WirePacketHeader, MTU};

    const SELF_ANNOUNCE_RNS_ANNOUNCE_DATA: &str =
        "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
         d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737\
         ab49baa826f122c1437f44444444444444444444\
         3dba22d6ca6544a5cc056182536b9c42077e769ebd4398fea328a66424fa8972\
         0d8639c7ad031b59ed698508eddf96dc0a130a21af65b2022ae0a118e497660f\
         68656c6c6f2d706572736f6e616c";

    #[test]
    fn self_announce_originates_the_rns_1_3_1_vector() {
        let mut state = personal_node_announcer();
        let now = InstantMillis(0x44_4444_4444 * 1000);
        let self_announce_entropy = SelfAnnounceEntropy::new([0x44; SelfAnnounceEntropy::LEN]);

        let mut buf = [0u8; MTU];
        let n = state
            .write_due_self_announce(now, self_announce_entropy, TEST_RATCHET_ENTROPY, &mut buf)
            .written_len();

        let (header, payload) = WirePacketHeader::parse(&buf[..n]).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.propagation, PropagationType::Broadcast);
        assert_eq!(header.hops, 0, "we originate at hop count 0");
        assert_eq!(
            header.destination,
            DestinationHash::new(hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap()),
        );
        assert_eq!(payload, hx(SELF_ANNOUNCE_RNS_ANNOUNCE_DATA));
    }

    #[test]
    fn a_ratcheted_self_announce_originates_the_rns_1_3_1_vector() {
        let mut state = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let now = InstantMillis(0x44_4444_4444 * 1000);
        let self_announce_entropy = SelfAnnounceEntropy::new([0x44; SelfAnnounceEntropy::LEN]);

        let mut buf = [0u8; MTU];
        let n = state
            .write_due_self_announce(now, self_announce_entropy, TEST_RATCHET_ENTROPY, &mut buf)
            .written_len();

        assert_eq!(&buf[..n], hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE));
    }

    fn parsed_ratchet_of(wire: &[u8]) -> Option<RatchetKey> {
        let (header, payload) = WirePacketHeader::parse(wire).unwrap();
        Announce::from_wire(&header, payload).unwrap().maybe_ratchet
    }

    #[test]
    fn an_announce_inside_the_rotation_floor_recarries_the_newest_ratchet() {
        use crate::crypto::ratchets::MIN_RATCHET_ROTATION_INTERVAL_MS;
        use crate::crypto::{x25519_public_key, X25519SecretKey};

        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = state.held_identity_hashes()[0];
        let destination = state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::Ratcheted,
            )
            .unwrap();
        state
            .schedule_announce(
                &destination,
                AnnounceConfig {
                    app_data: b"hello-personal",
                    schedule: ReannounceSchedule::every(1_000),
                },
            )
            .unwrap();

        let expected_first =
            RatchetKey::new(x25519_public_key(&X25519SecretKey::new([0x55; 32])).0);
        let expected_rotated =
            RatchetKey::new(x25519_public_key(&X25519SecretKey::new([0x66; 32])).0);

        let mut buf = [0u8; MTU];
        let n = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                RatchetEntropy::new([0x55; RatchetEntropy::LEN]),
                &mut buf,
            )
            .written_len();
        assert_eq!(parsed_ratchet_of(&buf[..n]), Some(expected_first));

        let (n, came_home) = match state.write_due_self_announce(
            InstantMillis(2_000),
            TEST_SELF_ANNOUNCE_ENTROPY,
            RatchetEntropy::new([0x66; RatchetEntropy::LEN]),
            &mut buf,
        ) {
            DueSelfAnnounceWriteOutcome::Written {
                len,
                rotation: RatchetRotation::Unspent(entropy),
            } => (len, entropy),
            _ => panic!("inside the floor the announce writes and the entropy comes home"),
        };
        assert_eq!(
            parsed_ratchet_of(&buf[..n]),
            Some(expected_first),
            "inside the floor the unused entropy is handed home, not minted",
        );

        let n = state
            .write_due_self_announce(
                InstantMillis(1_000 + MIN_RATCHET_ROTATION_INTERVAL_MS),
                TEST_SELF_ANNOUNCE_ENTROPY,
                came_home,
                &mut buf,
            )
            .written_len();
        assert_eq!(
            parsed_ratchet_of(&buf[..n]),
            Some(expected_rotated),
            "the unit that came home mints byte-identical key material later",
        );
    }

    #[test]
    fn a_ratcheted_destination_reserves_announce_room_for_the_ratchet() {
        use crate::routing::announce::self_announce::MAX_SELF_ANNOUNCE_APP_DATA_LEN;

        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = state.held_identity_hashes()[0];
        let destination = state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::Ratcheted,
            )
            .unwrap();

        let exactly_ratcheted_max = [0u8; MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN];
        let one_over = [0u8; MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN + 1];
        assert!(one_over.len() <= MAX_SELF_ANNOUNCE_APP_DATA_LEN);

        assert_eq!(
            state.schedule_announce(
                &destination,
                AnnounceConfig {
                    app_data: &one_over,
                    schedule: ReannounceSchedule::default(),
                },
            ),
            Err(ScheduleAnnounceError::AppDataTooLong),
        );
        assert_eq!(
            state.schedule_announce(
                &destination,
                AnnounceConfig {
                    app_data: &exactly_ratcheted_max,
                    schedule: ReannounceSchedule::default(),
                },
            ),
            Ok(()),
        );
    }

    #[test]
    fn self_announce_is_not_due_again_until_the_interval_elapses() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; MTU];
        let interval = ReannounceSchedule::default().interval_millis();

        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();
        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .nothing_due();
        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000 + interval),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();
    }

    #[test]
    fn a_failed_announce_attempt_surfaces_the_error_and_consumes_the_due_ness() {
        let mut state = personal_node_announcer();
        let mut tiny = [0u8; 8];
        let (error, _rotation) = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut tiny,
            )
            .failure();
        assert_eq!(
            error,
            SelfAnnounceWriteFailure::Serialize(EgressSerializeError::BufferTooShort),
        );

        let mut buf = [0u8; MTU];
        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .nothing_due();
        let interval = ReannounceSchedule::default().interval_millis();
        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000 + interval),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();
    }

    #[test]
    fn nothing_due_hands_both_units_home_intact() {
        let mut state = personal_node_announcer();
        let mut probe = personal_node_announcer();
        let mut buf = [0u8; MTU];
        let interval = ReannounceSchedule::default().interval_millis();
        let later = InstantMillis(1_000 + interval);

        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();
        let (self_announce_entropy, ratchet) = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .nothing_due();

        let mut reused = [0u8; MTU];
        let n = state
            .write_due_self_announce(later, self_announce_entropy, ratchet, &mut reused)
            .written_len();

        let _ = probe
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();
        let mut fresh = [0u8; MTU];
        let m = probe
            .write_due_self_announce(
                later,
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut fresh,
            )
            .written_len();

        assert_eq!(
            &reused[..n],
            &fresh[..m],
            "units that came home write byte-identical wire later",
        );
    }

    #[test]
    fn a_rejected_commanded_announce_hands_both_units_home_intact() {
        let mut state = personal_node_announcer();
        let commanded = AnnounceNow {
            destination: DestinationHash::new([0xEE; 16]),
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        };
        let mut buf = [0u8; MTU];
        let (error, self_announce_entropy, ratchet) = state
            .write_commanded_announce(
                &commanded,
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .rejection();
        assert_eq!(error, SelfAnnounceRejection::NotRegisteredAsSingle);

        let mut reused = [0u8; MTU];
        let n = state
            .write_due_self_announce(
                InstantMillis(1_000),
                self_announce_entropy,
                ratchet,
                &mut reused,
            )
            .written_len();
        let mut fresh = [0u8; MTU];
        let m = personal_node_announcer()
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut fresh,
            )
            .written_len();
        assert_eq!(
            &reused[..n],
            &fresh[..m],
            "a rejected command's units come home intact for the scheduled announce",
        );
    }

    #[test]
    fn a_due_announce_for_a_destination_no_longer_registered_rejects_with_units_home() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        state
            .self_announces
            .schedule(
                dest(0xEE),
                AnnounceConfig {
                    app_data: b"drifted",
                    schedule: ReannounceSchedule::default(),
                },
            )
            .unwrap();

        let mut buf = [0u8; MTU];
        let (error, _self_announce_entropy, _ratchet) = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .rejection();
        assert_eq!(error, SelfAnnounceRejection::NotRegisteredAsSingle);

        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .nothing_due();
    }

    #[test]
    fn a_failed_commanded_announce_surfaces_the_error_and_the_rotation_verdict() {
        let mut state = personal_node_announcer();
        let destination = state.self_announced_destinations()[0];
        let commanded = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        };

        let mut tiny = [0u8; 8];
        let (error, rotation) = state
            .write_commanded_announce(
                &commanded,
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut tiny,
            )
            .failure();
        assert_eq!(
            error,
            SelfAnnounceWriteFailure::Serialize(EgressSerializeError::BufferTooShort),
        );
        assert!(
            matches!(rotation, RatchetRotation::Unspent(_)),
            "an unratcheted destination's rotation verdict hands the entropy home",
        );
    }

    #[test]
    fn a_relay_default_state_never_originates() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let mut buf = [0u8; MTU];
        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .nothing_due();
    }

    #[test]
    fn an_identity_only_node_never_originates() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let mut buf = [0u8; MTU];
        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .nothing_due();
    }

    #[test]
    fn self_announced_destinations_report_our_addresses_only_when_announcing() {
        assert_eq!(
            personal_node_announcer().self_announced_destinations(),
            &[DestinationHash::new(
                hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap()
            )],
        );
        let relay: EngineState<Cap> = EngineState::<Cap>::default();
        assert_eq!(relay.self_announced_destinations(), &[]);
        let identity_only: EngineState<Cap> = EngineState::new(fixed_secret_key());
        assert_eq!(identity_only.self_announced_destinations(), &[]);
    }

    #[test]
    fn schedule_announce_requires_a_registered_single() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = state.held_identity_hashes()[0];
        let config = AnnounceConfig {
            app_data: b"",
            schedule: ReannounceSchedule::default(),
        };

        let unknown = DestinationHash::new([0x99; 16]);
        assert_eq!(
            state.schedule_announce(&unknown, config),
            Err(ScheduleAnnounceError::UnknownDestination),
        );

        let plain = state
            .register_plain_destination("personal", &["node"])
            .unwrap();
        let config = AnnounceConfig {
            app_data: b"",
            schedule: ReannounceSchedule::default(),
        };
        assert_eq!(
            state.schedule_announce(&plain, config),
            Err(ScheduleAnnounceError::NotASingleDestination),
        );

        let single = state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let config = AnnounceConfig {
            app_data: b"",
            schedule: ReannounceSchedule::default(),
        };
        assert_eq!(state.schedule_announce(&single, config), Ok(()));
        assert_eq!(state.self_announced_destinations(), &[single]);
    }

    #[test]
    fn a_commanded_announce_is_the_scheduled_announce_on_the_wire() {
        let mut state = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let destination = state.self_announced_destinations()[0];
        let now = InstantMillis(0x44_4444_4444 * 1000);
        let self_announce_entropy = SelfAnnounceEntropy::new([0x44; SelfAnnounceEntropy::LEN]);
        let commanded = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        };

        let mut buf = [0u8; MTU];
        let n = state
            .write_commanded_announce(
                &commanded,
                now,
                self_announce_entropy,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();

        assert_eq!(&buf[..n], hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE));
    }

    #[test]
    fn a_commanded_announce_resets_the_reannounce_clock() {
        let mut state = personal_node_announcer();
        let destination = state.self_announced_destinations()[0];
        let commanded = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        };

        let mut buf = [0u8; MTU];
        let _ = state
            .write_commanded_announce(
                &commanded,
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();

        let _ = state
            .write_due_self_announce(
                InstantMillis(2_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .nothing_due();
    }

    #[test]
    fn a_commanded_announce_carries_explicit_data_on_the_wire() {
        let mut state = personal_node_announcer();
        let destination = state.self_announced_destinations()[0];
        let commanded = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Data(
                SelfAnnounceAppData::from_slice(b"manual-data").unwrap(),
            ),
        };

        let mut buf = [0u8; MTU];
        let n = state
            .write_commanded_announce(
                &commanded,
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();

        let (header, payload) = WirePacketHeader::parse(&buf[..n]).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();
        assert_eq!(announce.destination, destination);
        assert_eq!(announce.app_data, b"manual-data");
    }

    #[test]
    fn a_commanded_announce_for_an_unscheduled_destination_announces_bare() {
        let mut state = personal_node_announcer();
        let node = state.held_identity_hashes()[0];
        let unscheduled = state
            .register_single_destination(
                &node,
                "personal",
                &["unscheduled"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let commanded = AnnounceNow {
            destination: unscheduled,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        };

        let mut buf = [0u8; MTU];
        let n = state
            .write_commanded_announce(
                &commanded,
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();

        let (header, payload) = WirePacketHeader::parse(&buf[..n]).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();
        assert_eq!(announce.destination, unscheduled);
        assert_eq!(announce.app_data, b"");
    }

    #[test]
    fn each_announced_destination_signs_with_its_own_identity() {
        let mut state = personal_node_announcer();
        let second = state.hold_identity(second_secret_key()).unwrap();
        let second_destination = state
            .register_single_destination(
                &second,
                "personal",
                &["second"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        state
            .schedule_announce(
                &second_destination,
                AnnounceConfig {
                    app_data: b"hello-second",
                    schedule: ReannounceSchedule::default(),
                },
            )
            .unwrap();

        let now = InstantMillis(5_000);
        let mut first_buf = [0u8; MTU];
        let first_len = state
            .write_due_self_announce(
                now,
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut first_buf,
            )
            .written_len();
        let mut second_buf = [0u8; MTU];
        let second_len = state
            .write_due_self_announce(
                now,
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut second_buf,
            )
            .written_len();
        let _ = state
            .write_due_self_announce(
                now,
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut [0u8; MTU],
            )
            .nothing_due();

        let second_identity = InMemoryNodeIdentity::from_secret_key_bytes(&second_secret_key());
        let expected = Announce::build_signed(
            &second_identity,
            crate::routing::announce::expand_name("personal", &["second"]).unwrap(),
            AnnounceId::mint(TEST_SELF_ANNOUNCE_ENTROPY, now),
            None,
            b"hello-second",
        )
        .unwrap();
        let mut expected_buf = [0u8; MTU];
        let expected_len = write_announce_wire_packet(&expected, 0, &mut expected_buf).unwrap();

        assert_eq!(&second_buf[..second_len], &expected_buf[..expected_len]);
        assert_ne!(&first_buf[..first_len], &second_buf[..second_len]);
    }
}
