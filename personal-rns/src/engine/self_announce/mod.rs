mod impls;

pub use impls::*;

use super::InstantMillis;
use crate::routing::announce::ANNOUNCE_FIXED_FIELDS_LEN;
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

use crate::engine::commands::{AnnounceAppData, AnnounceNow};
use crate::engine::egress::{write_announce_wire_packet, EgressSerializeError};
use crate::engine::self_ratchets::RatchetEntropy;
use crate::engine::EngineState;
use crate::routing::announce::{
    Announce, AnnounceBuildError, AnnounceId, RatchetKey, SelfAnnounceEntropy,
};
use crate::routing::storage::EngineStorage;
use crate::routing::upstream_app_destinations::UpstreamAppDestinationKind;

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

    /// `Ok(None)` means nothing was due — the common case, not a failure. An
    /// attempt at a due announce consumes its due-ness whether or not it
    /// succeeds: a persistently failing announce retries next interval instead
    /// of spinning the engine's `Immediate` wakeup forever.
    ///
    /// A ratcheted destination rotates here, before the announce is framed
    /// (RNS 1.3.1 `Destination.announce` calls `rotate_ratchets` first), so
    /// the announce always carries the newest ratchet.
    pub fn write_due_self_announce(
        &mut self,
        now: InstantMillis,
        entropy: SelfAnnounceEntropy,
        ratchet_entropy: RatchetEntropy,
        buf: &mut [u8],
    ) -> Result<Option<usize>, WriteSelfAnnounceError> {
        let Some(due) = self.self_announces.due_announce(now) else {
            return Ok(None);
        };
        let destination = due.destination;
        self.self_ratchets
            .rotate_if_due(&destination, now, ratchet_entropy);
        let maybe_ratchet = self.self_ratchets.newest_ratchet_key(&destination);
        let outcome =
            self.write_announce_for(&destination, due.app_data, now, entropy, maybe_ratchet, buf);
        self.self_announces.mark_announced(&destination, now);
        outcome.map(Some)
    }

    pub fn write_commanded_announce(
        &mut self,
        commanded: &AnnounceNow,
        now: InstantMillis,
        entropy: SelfAnnounceEntropy,
        ratchet_entropy: RatchetEntropy,
        buf: &mut [u8],
    ) -> Result<usize, WriteSelfAnnounceError> {
        let destination = commanded.destination;
        self.self_ratchets
            .rotate_if_due(&destination, now, ratchet_entropy);
        let maybe_ratchet = self.self_ratchets.newest_ratchet_key(&destination);
        let app_data = match &commanded.app_data {
            AnnounceAppData::Scheduled => self
                .self_announces
                .scheduled_app_data(&destination)
                .unwrap_or(&[]),
            AnnounceAppData::Data(data) => data,
        };
        let outcome =
            self.write_announce_for(&destination, app_data, now, entropy, maybe_ratchet, buf);
        self.self_announces.mark_announced(&destination, now);
        outcome
    }

    fn write_announce_for(
        &self,
        destination: &DestinationHash,
        app_data: &[u8],
        now: InstantMillis,
        entropy: SelfAnnounceEntropy,
        maybe_ratchet: Option<RatchetKey>,
        buf: &mut [u8],
    ) -> Result<usize, WriteSelfAnnounceError> {
        let registered = self
            .upstream_app_destinations
            .lookup(destination, DestinationType::Single)
            .ok_or(WriteSelfAnnounceError::NotRegisteredAsSingle)?;
        let UpstreamAppDestinationKind::Single { identity, .. } = registered.kind else {
            return Err(WriteSelfAnnounceError::NotRegisteredAsSingle);
        };
        let identity = self
            .held_identities
            .get(&identity)
            .ok_or(WriteSelfAnnounceError::IdentityNotHeld)?;

        let announce = Announce::build_signed(
            &identity,
            registered.name_hash,
            AnnounceId::mint(entropy, now),
            maybe_ratchet,
            app_data,
        )
        .map_err(WriteSelfAnnounceError::Build)?;
        write_announce_wire_packet(&announce, 0, buf).map_err(WriteSelfAnnounceError::Serialize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let now = InstantMillis(0x44_4444_4444);
        let nonce = SelfAnnounceEntropy::new([0x44; SelfAnnounceEntropy::LEN]);

        let mut buf = [0u8; MTU];
        let n = state
            .write_due_self_announce(now, nonce, TEST_RATCHET_ENTROPY, &mut buf)
            .expect("writing a due self-announce succeeds")
            .expect("a self-announce is due on the first call");

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

    const RATCHETED_SELF_ANNOUNCE_RNS_WIRE: &str = "2100c3cfae69b36bb6e3bbfd96a3b5867a5900\
         0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
         d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737\
         ab49baa826f122c1437f44444444444444444444\
         38ab664bd86f77d7e66bdd9ae0792913a94fd8b33a1260027e4b46c1f4884c67\
         91d8c21a401611ca859e9ae293e86a6860fb2babd90fe4c58cf315d7a111cc0a\
         3e9646aa7ffdf1530150aa30d0c684aab5b6236ea71a4b8f8c72b2b02768bf02\
         68656c6c6f2d706572736f6e616c";

    #[test]
    fn a_ratcheted_self_announce_originates_the_rns_1_3_1_vector() {
        let mut state = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let now = InstantMillis(0x44_4444_4444);
        let nonce = SelfAnnounceEntropy::new([0x44; SelfAnnounceEntropy::LEN]);

        let mut buf = [0u8; MTU];
        let n = state
            .write_due_self_announce(now, nonce, TEST_RATCHET_ENTROPY, &mut buf)
            .expect("writing a due self-announce succeeds")
            .expect("a self-announce is due on the first call");

        assert_eq!(&buf[..n], hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE));
    }

    fn parsed_ratchet_of(wire: &[u8]) -> Option<RatchetKey> {
        let (header, payload) = WirePacketHeader::parse(wire).unwrap();
        Announce::from_wire(&header, payload).unwrap().maybe_ratchet
    }

    #[test]
    fn an_announce_inside_the_rotation_floor_recarries_the_newest_ratchet() {
        use crate::crypto::{x25519_public_key, X25519SecretKey};
        use crate::engine::self_ratchets::MIN_RATCHET_ROTATION_INTERVAL_MS;

        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = state.transport_identity().unwrap();
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
            RatchetKey::new(x25519_public_key(&X25519SecretKey::new([0x77; 32])).0);

        let mut buf = [0u8; MTU];
        let n = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_NONCE,
                RatchetEntropy::new([0x55; RatchetEntropy::LEN]),
                &mut buf,
            )
            .unwrap()
            .unwrap();
        assert_eq!(parsed_ratchet_of(&buf[..n]), Some(expected_first));

        let n = state
            .write_due_self_announce(
                InstantMillis(2_000),
                TEST_NONCE,
                RatchetEntropy::new([0x66; RatchetEntropy::LEN]),
                &mut buf,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            parsed_ratchet_of(&buf[..n]),
            Some(expected_first),
            "inside the floor the unused entropy is discarded, not minted",
        );

        let n = state
            .write_due_self_announce(
                InstantMillis(1_000 + MIN_RATCHET_ROTATION_INTERVAL_MS),
                TEST_NONCE,
                RatchetEntropy::new([0x77; RatchetEntropy::LEN]),
                &mut buf,
            )
            .unwrap()
            .unwrap();
        assert_eq!(parsed_ratchet_of(&buf[..n]), Some(expected_rotated));
    }

    #[test]
    fn a_ratcheted_destination_reserves_announce_room_for_the_ratchet() {
        use crate::engine::self_announce::MAX_SELF_ANNOUNCE_APP_DATA_LEN;

        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = state.transport_identity().unwrap();
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

        assert!(state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf
            )
            .unwrap()
            .is_some());
        assert!(state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf
            )
            .unwrap()
            .is_none());
        assert!(state
            .write_due_self_announce(
                InstantMillis(1_000 + interval),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf
            )
            .unwrap()
            .is_some());
    }

    #[test]
    fn a_failed_announce_attempt_surfaces_the_error_and_consumes_the_due_ness() {
        let mut state = personal_node_announcer();
        let mut tiny = [0u8; 8];
        assert_eq!(
            state.write_due_self_announce(
                InstantMillis(1_000),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut tiny
            ),
            Err(WriteSelfAnnounceError::Serialize(
                EgressSerializeError::BufferTooShort
            )),
        );

        let mut buf = [0u8; MTU];
        assert_eq!(
            state.write_due_self_announce(
                InstantMillis(1_000),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf
            ),
            Ok(None),
        );
        let interval = ReannounceSchedule::default().interval_millis();
        assert!(state
            .write_due_self_announce(
                InstantMillis(1_000 + interval),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf
            )
            .unwrap()
            .is_some());
    }

    #[test]
    fn a_relay_default_state_never_originates() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let mut buf = [0u8; MTU];
        assert_eq!(
            state.write_due_self_announce(
                InstantMillis(1_000),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf
            ),
            Ok(None),
        );
    }

    #[test]
    fn an_identity_only_node_never_originates() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let mut buf = [0u8; MTU];
        assert_eq!(
            state.write_due_self_announce(
                InstantMillis(1_000),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf
            ),
            Ok(None),
        );
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
        let node = state.transport_identity().unwrap();
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
        let now = InstantMillis(0x44_4444_4444);
        let nonce = SelfAnnounceEntropy::new([0x44; SelfAnnounceEntropy::LEN]);
        let commanded = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        };

        let mut buf = [0u8; MTU];
        let n = state
            .write_commanded_announce(&commanded, now, nonce, TEST_RATCHET_ENTROPY, &mut buf)
            .unwrap();

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
        state
            .write_commanded_announce(
                &commanded,
                InstantMillis(1_000),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .unwrap();

        assert_eq!(
            state.write_due_self_announce(
                InstantMillis(2_000),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            ),
            Ok(None),
        );
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
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .unwrap();

        let (header, payload) = WirePacketHeader::parse(&buf[..n]).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();
        assert_eq!(announce.destination, destination);
        assert_eq!(announce.app_data, b"manual-data");
    }

    #[test]
    fn a_commanded_announce_for_an_unscheduled_destination_announces_bare() {
        let mut state = personal_node_announcer();
        let node = state.transport_identity().unwrap();
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
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .unwrap();

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
            .write_due_self_announce(now, TEST_NONCE, TEST_RATCHET_ENTROPY, &mut first_buf)
            .expect("writing a due self-announce succeeds")
            .expect("the first scheduled announce fires");
        let mut second_buf = [0u8; MTU];
        let second_len = state
            .write_due_self_announce(now, TEST_NONCE, TEST_RATCHET_ENTROPY, &mut second_buf)
            .expect("writing a due self-announce succeeds")
            .expect("the second scheduled announce fires");
        assert_eq!(
            state.write_due_self_announce(now, TEST_NONCE, TEST_RATCHET_ENTROPY, &mut [0u8; MTU]),
            Ok(None),
        );

        let second_identity = InMemoryNodeIdentity::from_secret_key_bytes(&second_secret_key());
        let expected = Announce::build_signed(
            &second_identity,
            crate::routing::announce::expand_name("personal", &["second"]).unwrap(),
            AnnounceId::mint(TEST_NONCE, now),
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
