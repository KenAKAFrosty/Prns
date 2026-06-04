use super::InstantMillis;
use crate::routing::announce::{expand_name, DottedNameHash, ExpandNameError};
use heapless::Vec as HeaplessVec;

/// Maximum app-data bytes carried in our own announce. RNS app data on an
/// announce (a node label, an LXMF display name, optional ticket bytes) is
/// modest; 256 bytes is generous, keeps the announce within
/// [`MTU`](crate::wire::MTU) (≈165 B overhead + app_data ≤ 500), and lets a
/// longer announce span two LoRa frames — so a self-announce exercises the
/// LoRa interface's split/reassembly, not just single frames. `from_config`
/// rejects app_data beyond this with [`SelfAnnounceConfigError::AppDataTooLong`]
/// rather than overflowing.
pub const MAX_SELF_ANNOUNCE_APP_DATA_LEN: usize = 256;

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
    name_hash: DottedNameHash,
    app_data: HeaplessVec<u8, MAX_SELF_ANNOUNCE_APP_DATA_LEN>,
    schedule: ReannounceSchedule,
    last_announced: Option<InstantMillis>,
}

impl SelfAnnounceSettings {
    pub fn from_config(config: SelfAnnounceConfig<'_>) -> Result<Self, SelfAnnounceConfigError> {
        let name_hash = expand_name(config.app_name, config.aspects).map_err(|err| match err {
            ExpandNameError::DotInComponent => SelfAnnounceConfigError::DotInName,
            ExpandNameError::NameTooLong => SelfAnnounceConfigError::NameTooLong,
        })?;
        let mut app_data = HeaplessVec::new();
        app_data
            .extend_from_slice(config.app_data)
            .map_err(|_| SelfAnnounceConfigError::AppDataTooLong)?;
        Ok(Self {
            name_hash,
            app_data,
            schedule: config.schedule,
            last_announced: None,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn personal_node() -> SelfAnnounceSettings {
        SelfAnnounceSettings::from_config(SelfAnnounceConfig {
            app_name: "personal",
            aspects: &["node"],
            app_data: b"hello-personal",
            schedule: ReannounceSchedule::default(),
        })
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
            SelfAnnounceSettings::from_config(SelfAnnounceConfig {
                app_name: "per.sonal",
                aspects: &[],
                app_data: b"",
                schedule: ReannounceSchedule::default(),
            }),
            Err(SelfAnnounceConfigError::DotInName),
        );

        let too_long = [0u8; MAX_SELF_ANNOUNCE_APP_DATA_LEN + 1];
        assert_eq!(
            SelfAnnounceSettings::from_config(SelfAnnounceConfig {
                app_name: "personal",
                aspects: &["node"],
                app_data: &too_long,
                schedule: ReannounceSchedule::default(),
            }),
            Err(SelfAnnounceConfigError::AppDataTooLong),
        );
    }
}
