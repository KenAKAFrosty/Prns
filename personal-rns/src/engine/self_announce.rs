//! Self-announce policy and state — the optional, app-driven decision to
//! originate our own destination's announce on a cadence.
//!
//! Owning a node identity (so we can sign and agree) is separate from choosing
//! to announce ourselves: a pure repeater forwards others' announces and never
//! announces a destination of its own. So an engine holds at most one
//! [`SelfAnnounce`] — absent for a relay, present for a node that periodically
//! emits its own announce.

use super::InstantMillis;
use crate::routing::announce::{expand_name, DottedNameHash, ExpandNameError};
use heapless::Vec as HeaplessVec;

/// Maximum app-data bytes carried in our own announce. RNS app data on an
/// announce is small (a node label, an LXMF display name); 128 bytes is
/// generous and keeps the announce well within [`MTU`](crate::wire::MTU).
pub const MAX_SELF_ANNOUNCE_APP_DATA_LEN: usize = 128;

/// Reticulum's default re-announce cadence — every 6 hours, in milliseconds.
pub const DEFAULT_REANNOUNCE_INTERVAL_MS: u64 = 6 * 60 * 60 * 1000;

/// How often we re-emit our own announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReannounceSchedule {
    interval_millis: u64,
}

impl ReannounceSchedule {
    /// Re-announce once every `interval_millis`.
    pub const fn every(interval_millis: u64) -> Self {
        Self { interval_millis }
    }

    pub const fn interval_millis(&self) -> u64 {
        self.interval_millis
    }
}

impl Default for ReannounceSchedule {
    /// Reticulum's default cadence: every 6 hours.
    fn default() -> Self {
        Self::every(DEFAULT_REANNOUNCE_INTERVAL_MS)
    }
}

/// What a node announces about itself: which destination (app name + aspects),
/// what app data rides along, and how often. The transient input the engine
/// parses into a [`SelfAnnounce`] at construction.
pub struct SelfAnnounceConfig<'a> {
    pub app_name: &'a str,
    pub aspects: &'a [&'a str],
    pub app_data: &'a [u8],
    pub schedule: ReannounceSchedule,
}

/// Why a [`SelfAnnounceConfig`] couldn't be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfAnnounceConfigError {
    /// An app name or aspect contained a `.` (see [`ExpandNameError::DotInComponent`]).
    DotInName,
    /// The expanded destination name exceeded the bound
    /// (see [`ExpandNameError::NameTooLong`]).
    NameTooLong,
    /// `app_data` exceeded [`MAX_SELF_ANNOUNCE_APP_DATA_LEN`].
    AppDataTooLong,
}

/// The engine's resolved self-announce: the destination name hash it derived
/// once, the app data it carries, the cadence, and when it last emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfAnnounce {
    name_hash: DottedNameHash,
    app_data: HeaplessVec<u8, MAX_SELF_ANNOUNCE_APP_DATA_LEN>,
    schedule: ReannounceSchedule,
    last_announced: Option<InstantMillis>,
}

impl SelfAnnounce {
    /// Parse a [`SelfAnnounceConfig`] into resolved state: derive the
    /// destination name hash from the app name + aspects once, and copy the app
    /// data into bounded storage. This is the parse-don't-validate seam — every
    /// later read of a `SelfAnnounce` already holds valid, fixed-size material.
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

    /// Whether an announce is due at `now`: immediately the first time (we have
    /// never announced), then once the schedule's interval has elapsed since the
    /// last emission.
    pub fn is_due(&self, now: InstantMillis) -> bool {
        match self.last_announced {
            None => true,
            Some(last) => now.0.saturating_sub(last.0) >= self.schedule.interval_millis(),
        }
    }

    /// Record that we emitted an announce at `now`.
    pub fn mark_announced(&mut self, now: InstantMillis) {
        self.last_announced = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn personal_node() -> SelfAnnounce {
        SelfAnnounce::from_config(SelfAnnounceConfig {
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
        // Same name hash expand_name pins against RNS 1.3.1.
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

        // Never announced → due right away.
        assert!(sa.is_due(InstantMillis(0)));

        sa.mark_announced(InstantMillis(1_000));
        assert!(!sa.is_due(InstantMillis(1_000)));
        // One millisecond short of the interval is still not due.
        assert!(!sa.is_due(InstantMillis(1_000 + interval - 1)));
        // Exactly the interval later is due again.
        assert!(sa.is_due(InstantMillis(1_000 + interval)));
    }

    #[test]
    fn from_config_rejects_dotted_names_and_overlong_app_data() {
        assert_eq!(
            SelfAnnounce::from_config(SelfAnnounceConfig {
                app_name: "per.sonal",
                aspects: &[],
                app_data: b"",
                schedule: ReannounceSchedule::default(),
            }),
            Err(SelfAnnounceConfigError::DotInName),
        );

        let too_long = [0u8; MAX_SELF_ANNOUNCE_APP_DATA_LEN + 1];
        assert_eq!(
            SelfAnnounce::from_config(SelfAnnounceConfig {
                app_name: "personal",
                aspects: &["node"],
                app_data: &too_long,
                schedule: ReannounceSchedule::default(),
            }),
            Err(SelfAnnounceConfigError::AppDataTooLong),
        );
    }
}
