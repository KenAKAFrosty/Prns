mod impls;

pub use impls::*;

use super::InstantMillis;
use crate::identity::IdentityHash;
use crate::routing::announce::{expand_name, DottedNameHash, ExpandNameError};
use crate::wire::DestinationHash;
use heapless::Vec as HeaplessVec;

pub const MAX_SELF_ANNOUNCE_APP_DATA_LEN: usize = 256; //REVIEW this can just be the actual maximum which isn't much more and avoids weird contention. I think that const is already pre-computed and properly shared form wire or somewhere else in announce maybe? We should just use that here I think.

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

pub struct SelfAnnounceConfig<'a> {
    pub app_name: &'a str,
    pub aspects: &'a [&'a str],
    pub app_data: &'a [u8],
    pub schedule: ReannounceSchedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfAnnounceConfigError {
    DotInName,
    NameTooLong,
    AppDataTooLong,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfAnnounceSettings {
    identity_hash: IdentityHash,
    name_hash: DottedNameHash,
    app_data: HeaplessVec<u8, MAX_SELF_ANNOUNCE_APP_DATA_LEN>,
    schedule: ReannounceSchedule,
    last_announced: Option<InstantMillis>,
}

impl SelfAnnounceSettings {
    pub fn from_config(
        config: SelfAnnounceConfig<'_>,
        identity_hash: IdentityHash,
    ) -> Result<Self, SelfAnnounceConfigError> {
        let name_hash = expand_name(config.app_name, config.aspects).map_err(|err| match err {
            ExpandNameError::DotInComponent => SelfAnnounceConfigError::DotInName,
            ExpandNameError::NameTooLong => SelfAnnounceConfigError::NameTooLong,
        })?;
        let mut app_data = HeaplessVec::new();
        app_data
            .extend_from_slice(config.app_data)
            .map_err(|_| SelfAnnounceConfigError::AppDataTooLong)?;
        Ok(Self {
            identity_hash,
            name_hash,
            app_data,
            schedule: config.schedule,
            last_announced: None,
        })
    }

    pub fn identity_hash(&self) -> IdentityHash {
        self.identity_hash
    }

    pub fn name_hash(&self) -> DottedNameHash {
        self.name_hash
    }

    pub fn app_data(&self) -> &[u8] {
        &self.app_data
    }

    pub fn is_due(&self, now: InstantMillis) -> bool {
        match self.last_announced {
            None => true,
            Some(last) => now.0.saturating_sub(last.0) >= self.schedule.interval_millis(),
        }
    }

    pub fn next_due_at(&self) -> Option<InstantMillis> {
        self.last_announced
            .map(|last| InstantMillis(last.0.saturating_add(self.schedule.interval_millis())))
    }

    pub fn mark_announced(&mut self, now: InstantMillis) {
        self.last_announced = Some(now);
    }
}

pub struct AnnounceConfig<'a> {
    pub app_data: &'a [u8],
    pub schedule: ReannounceSchedule,
}

pub type SelfAnnounceAppData = HeaplessVec<u8, MAX_SELF_ANNOUNCE_APP_DATA_LEN>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleAnnounceError {
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

    fn node_identity_hash() -> IdentityHash {
        IdentityHash::new([0x4c; 16])
    }

    fn personal_node() -> SelfAnnounceSettings {
        SelfAnnounceSettings::from_config(
            SelfAnnounceConfig {
                app_name: "personal",
                aspects: &["node"],
                app_data: b"hello-personal",
                schedule: ReannounceSchedule::default(),
            },
            node_identity_hash(),
        )
        .unwrap()
    }

    #[test]
    fn default_schedule_is_six_hours() {
        assert_eq!(
            ReannounceSchedule::default().interval_millis(),
            6 * 60 * 60 * 1000,
        );
    }

    #[test]
    fn from_config_derives_the_personal_node_name_hash() {
        assert_eq!(
            personal_node().name_hash(),
            DottedNameHash::new([0xab, 0x49, 0xba, 0xa8, 0x26, 0xf1, 0x22, 0xc1, 0x43, 0x7f]),
        );
        assert_eq!(personal_node().app_data(), b"hello-personal");
        assert_eq!(personal_node().identity_hash(), node_identity_hash());
    }

    #[test]
    fn is_due_immediately_then_only_after_the_interval() {
        let mut sa = personal_node();
        let interval = ReannounceSchedule::default().interval_millis();

        assert!(sa.is_due(InstantMillis(0)));

        sa.mark_announced(InstantMillis(1_000));
        assert!(!sa.is_due(InstantMillis(1_000)));
        assert!(!sa.is_due(InstantMillis(1_000 + interval - 1)));
        assert!(sa.is_due(InstantMillis(1_000 + interval)));
    }

    #[test]
    fn from_config_rejects_dotted_names_and_overlong_app_data() {
        assert_eq!(
            SelfAnnounceSettings::from_config(
                SelfAnnounceConfig {
                    app_name: "per.sonal",
                    aspects: &[],
                    app_data: b"",
                    schedule: ReannounceSchedule::default(),
                },
                node_identity_hash(),
            ),
            Err(SelfAnnounceConfigError::DotInName),
        );

        let too_long = [0u8; MAX_SELF_ANNOUNCE_APP_DATA_LEN + 1];
        assert_eq!(
            SelfAnnounceSettings::from_config(
                SelfAnnounceConfig {
                    app_name: "personal",
                    aspects: &["node"],
                    app_data: &too_long,
                    schedule: ReannounceSchedule::default(),
                },
                node_identity_hash(),
            ),
            Err(SelfAnnounceConfigError::AppDataTooLong),
        );
    }
}
