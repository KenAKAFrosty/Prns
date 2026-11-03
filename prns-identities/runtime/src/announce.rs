//! Bounded unsolicited announce scheduling shared by one node owner.
//!
//! The embedding host supplies slots, clock observations, entropy, durable
//! storage and dispatch. Requested Reticulum path responses are deliberately
//! outside this scheduler. A reservation spends the budget before any external
//! effect; it is not evidence of transmission or peer receipt.

use prns_identities_core::{LocalIdentityId, ServiceId};

pub const COOLDOWN_MS: u64 = 60_000;
/// Keeps arithmetic bounded without coupling the scheduler to app choices.
pub const MAX_INTERVAL_MS: u64 = 366 * 24 * 60 * 60 * 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceKey {
    pub identity: LocalIdentityId,
    pub service: ServiceId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configuration {
    /// Caller-defined destinations in one bounded announce batch.
    pub channels: u64,
    /// None disables unsolicited startup/periodic work, not manual requests.
    pub interval_ms: Option<u64>,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub sequence: u64,
    pub wall_ms: Option<u64>,
    pub channels: u64,
    pub requested_mask: u64,
    /// Submission failed or was not confirmed; it may still have taken effect.
    /// Never interpret this as proof that no radio transmission occurred.
    pub failed_mask: u64,
    pub completed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: AnnounceKey,
    pub configuration: Configuration,
    pub last_attempt: Option<Attempt>,
}

/// Caller-owned durable state. Storage adapters may encode these fields, but
/// must validate decoded state before exposing it to a running owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot<const N: usize> {
    pub revision: u64,
    pub last_sequence: u64,
    pub cursor: usize,
    pub entries: [Option<Entry>; N],
}

impl<const N: usize> Snapshot<N> {
    pub const fn empty() -> Self {
        Self {
            revision: 0,
            last_sequence: 0,
            cursor: 0,
            entries: [None; N],
        }
    }

    pub fn validate(&self) -> Result<(), Fault> {
        if N == 0 || self.cursor >= N || self.last_sequence > self.revision {
            return Err(Fault::InvalidSnapshot);
        }
        let mut greatest_sequence = 0;
        for (index, slot) in self.entries.iter().enumerate() {
            let Some(entry) = slot else { continue };
            validate_configuration(entry.key, entry.configuration)?;
            if self.entries[..index]
                .iter()
                .flatten()
                .any(|prior| prior.key == entry.key)
            {
                return Err(Fault::InvalidSnapshot);
            }
            if let Some(attempt) = entry.last_attempt {
                if attempt.sequence == 0
                    || attempt.sequence > self.last_sequence
                    || attempt.channels == 0
                    || attempt.requested_mask & attempt.failed_mask != 0
                    || (attempt.requested_mask | attempt.failed_mask) & !attempt.channels != 0
                    || (attempt.completed
                        && (attempt.requested_mask | attempt.failed_mask) != attempt.channels)
                    || (!attempt.completed && (attempt.requested_mask | attempt.failed_mask) != 0)
                    || self.entries[..index].iter().flatten().any(|prior| {
                        prior
                            .last_attempt
                            .is_some_and(|value| value.sequence == attempt.sequence)
                    })
                {
                    return Err(Fault::InvalidSnapshot);
                }
                greatest_sequence = greatest_sequence.max(attempt.sequence);
            }
        }
        if greatest_sequence != self.last_sequence {
            return Err(Fault::InvalidSnapshot);
        }
        Ok(())
    }
}

/// Implementations must durably publish the complete snapshot atomically and
/// reject a stale expected revision. Any write error is potentially committed:
/// this owner becomes unusable until reloaded, and must not dispatch a token.
pub trait Storage<const N: usize> {
    type Error;
    fn load(&mut self, into: &mut Snapshot<N>) -> Result<(), Self::Error>;
    fn compare_and_store(
        &mut self,
        expected_revision: u64,
        next: &Snapshot<N>,
    ) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    InvalidConfiguration,
    InvalidSnapshot,
    UnknownKey,
    Exhausted,
    ReloadRequired,
    StaleReservation,
    InvalidOutcome,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Fault(Fault),
    Storage(E),
}
impl<E: core::fmt::Debug> core::fmt::Display for Error<E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "announce scheduler: {self:?}")
    }
}
impl<E: core::fmt::Debug> core::error::Error for Error<E> {}
impl<E> From<Fault> for Error<E> {
    fn from(value: Fault) -> Self {
        Self::Fault(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityManualAnnounce"))]
pub enum ManualRequest {
    Queued,
    /// The request remains coalesced and will be eligible after this delay.
    Throttled {
        retry_after_ms: u64,
    },
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reservation {
    key: AnnounceKey,
    sequence: u64,
    channels: u64,
}
impl Reservation {
    pub const fn key(self) -> AnnounceKey {
        self.key
    }
    pub const fn sequence(self) -> u64 {
        self.sequence
    }
    pub const fn channels(self) -> u64 {
        self.channels
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    manual: bool,
    automatic: bool,
    ready_at: u64,
    next_auto: Option<u64>,
}
impl Pending {
    const EMPTY: Self = Self {
        manual: false,
        automatic: false,
        ready_at: 0,
        next_auto: None,
    };
}

/// Volatile state belongs to the same exclusive owner as the durable storage.
/// Reconstruct only through `reload`: recreating it must not bypass cooldowns.
pub struct Runtime<const N: usize> {
    slots: [Pending; N],
    ready_at: u64,
    last_now: u64,
    last_wall: Option<u64>,
    loaded: bool,
    poisoned: bool,
}
impl<const N: usize> Runtime<N> {
    pub const fn empty() -> Self {
        Self {
            slots: [Pending::EMPTY; N],
            ready_at: 0,
            last_now: 0,
            last_wall: None,
            loaded: false,
            poisoned: false,
        }
    }
}

/// Borrowed controller: no allocator, executor, background task or wire format.
pub struct Scheduler<'a, const N: usize, S: Storage<N>> {
    pub storage: &'a mut S,
    pub snapshot: &'a mut Snapshot<N>,
    pub scratch: &'a mut Snapshot<N>,
    pub runtime: &'a mut Runtime<N>,
}

impl<const N: usize, S: Storage<N>> Scheduler<'_, N, S> {
    /// An unknown reboot interval always starts a fresh cooldown if any attempt
    /// has ever been reserved. Unfinished attempts are not replayed. Enabled
    /// automatic services coalesce into one startup batch each.
    pub fn reload(&mut self, now_ms: u64, wall_ms: Option<u64>) -> Result<(), Error<S::Error>> {
        self.runtime.poisoned = true;
        validate_clock(now_ms)?;
        self.storage.load(self.scratch).map_err(Error::Storage)?;
        self.scratch.validate()?;
        *self.snapshot = *self.scratch;
        let ready_at = if self.snapshot.last_sequence == 0 {
            now_ms
        } else {
            deadline(now_ms, COOLDOWN_MS)
        };
        *self.runtime = Runtime::empty();
        self.runtime.loaded = true;
        self.runtime.ready_at = ready_at;
        self.runtime.last_now = now_ms;
        self.runtime.last_wall = wall_ms;
        for (slot, entry) in self
            .runtime
            .slots
            .iter_mut()
            .zip(self.snapshot.entries.iter())
        {
            slot.ready_at = ready_at;
            slot.automatic = entry.is_some_and(|value| {
                value.configuration.enabled && value.configuration.interval_ms.is_some()
            });
        }
        Ok(())
    }

    pub fn configure(
        &mut self,
        key: AnnounceKey,
        configuration: Configuration,
        now_ms: u64,
        wall_ms: Option<u64>,
    ) -> Result<(), Error<S::Error>> {
        self.observe(now_ms, wall_ms)?;
        validate_configuration(key, configuration)?;
        let existing = self.find(key);
        let index = existing
            .or_else(|| self.snapshot.entries.iter().position(Option::is_none))
            .ok_or(Fault::Capacity)?;
        let prior = self.snapshot.entries[index];
        if prior.is_some_and(|entry| entry.configuration == configuration) {
            return Ok(());
        }
        *self.scratch = *self.snapshot;
        self.scratch.entries[index] = Some(Entry {
            key,
            configuration,
            last_attempt: prior.and_then(|entry| entry.last_attempt),
        });
        self.commit()?;
        let pending = &mut self.runtime.slots[index];
        if !configuration.enabled {
            pending.manual = false;
            pending.automatic = false;
            pending.next_auto = None;
        } else if prior.is_none_or(|entry| !entry.configuration.enabled) {
            pending.automatic = configuration.interval_ms.is_some();
            pending.next_auto = None;
        } else if prior
            .is_some_and(|entry| entry.configuration.interval_ms != configuration.interval_ms)
        {
            // A preference edit never itself creates an immediate announce or
            // resets a spent budget. Existing manual intent remains coalesced.
            pending.automatic = false;
            pending.next_auto = configuration
                .interval_ms
                .map(|interval| deadline(now_ms, interval));
        }
        Ok(())
    }

    pub fn request_manual(
        &mut self,
        key: AnnounceKey,
        now_ms: u64,
        wall_ms: Option<u64>,
    ) -> Result<ManualRequest, Error<S::Error>> {
        self.observe(now_ms, wall_ms)?;
        let index = self.find(key).ok_or(Fault::UnknownKey)?;
        if !self.snapshot.entries[index].is_some_and(|entry| entry.configuration.enabled) {
            return Ok(ManualRequest::Disabled);
        }
        self.runtime.slots[index].manual = true;
        let ready_at = self
            .runtime
            .ready_at
            .max(self.runtime.slots[index].ready_at);
        Ok(if ready_at > now_ms {
            ManualRequest::Throttled {
                retry_after_ms: ready_at - now_ms,
            }
        } else {
            ManualRequest::Queued
        })
    }

    /// Next useful scheduling observation. This is a timer hint, not admission:
    /// reserve_next still rechecks clocks, eligibility and durable reservation.
    pub fn next_ready_at(&self, now_ms: u64) -> Result<Option<u64>, Error<S::Error>> {
        self.check_loaded()?;
        Ok(self
            .snapshot
            .entries
            .iter()
            .zip(&self.runtime.slots)
            .filter_map(|(entry, pending)| {
                let entry = entry.as_ref()?;
                if !entry.configuration.enabled {
                    return None;
                }
                let due = if pending.manual || pending.automatic {
                    now_ms
                } else {
                    pending.next_auto?
                };
                Some(due.max(self.runtime.ready_at).max(pending.ready_at))
            })
            .min())
    }

    /// At most one batch. `eligible_interface` is supplied by the embedding
    /// transport and jitter is a uniformly sampled u16 for a 0–25% addition.
    /// The returned token exists only after durable reservation succeeds.
    pub fn reserve_next(
        &mut self,
        now_ms: u64,
        wall_ms: Option<u64>,
        eligible_interface: bool,
        jitter: u16,
    ) -> Result<Option<Reservation>, Error<S::Error>> {
        self.reserve_next_matching(now_ms, wall_ms, eligible_interface, jitter, |_| true)
    }

    /// Skip temporarily unavailable application owners without spending their
    /// pending intent or a shared attempt. The caller predicate owns no storage.
    pub fn reserve_next_matching(
        &mut self,
        now_ms: u64,
        wall_ms: Option<u64>,
        eligible_interface: bool,
        jitter: u16,
        mut available: impl FnMut(AnnounceKey) -> bool,
    ) -> Result<Option<Reservation>, Error<S::Error>> {
        self.observe(now_ms, wall_ms)?;
        for slot in &mut self.runtime.slots {
            if slot.next_auto.is_some_and(|due| due <= now_ms) {
                slot.automatic = true;
            }
        }
        if !eligible_interface || self.runtime.ready_at > now_ms {
            return Ok(None);
        }
        let index = (0..N)
            .map(|offset| (self.snapshot.cursor + offset) % N)
            .find(|&index| {
                let slot = self.runtime.slots[index];
                self.snapshot.entries[index]
                    .is_some_and(|entry| entry.configuration.enabled && available(entry.key))
                    && (slot.manual || slot.automatic)
                    && slot.ready_at <= now_ms
            });
        let Some(index) = index else { return Ok(None) };
        let entry = self.snapshot.entries[index].ok_or(Fault::InvalidSnapshot)?;
        let sequence = self
            .snapshot
            .last_sequence
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        let reservation = Reservation {
            key: entry.key,
            sequence,
            channels: entry.configuration.channels,
        };
        *self.scratch = *self.snapshot;
        self.scratch.last_sequence = sequence;
        self.scratch.cursor = (index + 1) % N;
        self.scratch.entries[index] = Some(Entry {
            last_attempt: Some(Attempt {
                sequence,
                wall_ms,
                channels: reservation.channels,
                requested_mask: 0,
                failed_mask: 0,
                completed: false,
            }),
            ..entry
        });
        self.commit()?;
        let ready_at = deadline(now_ms, COOLDOWN_MS);
        self.runtime.ready_at = ready_at;
        self.runtime.slots[index] = Pending {
            manual: false,
            automatic: false,
            ready_at,
            next_auto: entry
                .configuration
                .interval_ms
                .map(|interval| deadline(now_ms, jittered_interval(interval, jitter))),
        };
        Ok(Some(reservation))
    }

    /// Outcomes refer to host submission, never radio transmission. Preserve
    /// partial acceptance using disjoint masks covering the reserved batch.
    /// An uncertain crash before this write leaves an incomplete spent attempt.
    pub fn complete(
        &mut self,
        reservation: Reservation,
        requested_mask: u64,
        failed_mask: u64,
    ) -> Result<(), Error<S::Error>> {
        self.check_loaded()?;
        if requested_mask & failed_mask != 0 || requested_mask | failed_mask != reservation.channels
        {
            return Err(Fault::InvalidOutcome.into());
        }
        let index = self.find(reservation.key).ok_or(Fault::StaleReservation)?;
        let mut entry = self.snapshot.entries[index].ok_or(Fault::StaleReservation)?;
        let mut attempt = entry.last_attempt.ok_or(Fault::StaleReservation)?;
        if attempt.sequence != reservation.sequence || attempt.channels != reservation.channels {
            return Err(Fault::StaleReservation.into());
        }
        if attempt.completed {
            return if attempt.requested_mask == requested_mask && attempt.failed_mask == failed_mask
            {
                Ok(())
            } else {
                Err(Fault::InvalidOutcome.into())
            };
        }
        attempt.completed = true;
        attempt.requested_mask = requested_mask;
        attempt.failed_mask = failed_mask;
        entry.last_attempt = Some(attempt);
        *self.scratch = *self.snapshot;
        self.scratch.entries[index] = Some(entry);
        self.commit()
    }

    fn find(&self, key: AnnounceKey) -> Option<usize> {
        self.snapshot
            .entries
            .iter()
            .position(|entry| entry.is_some_and(|value| value.key == key))
    }

    fn check_loaded(&self) -> Result<(), Fault> {
        if !self.runtime.loaded || self.runtime.poisoned {
            Err(Fault::ReloadRequired)
        } else {
            Ok(())
        }
    }

    fn observe(&mut self, now_ms: u64, wall_ms: Option<u64>) -> Result<(), Fault> {
        self.check_loaded()?;
        validate_clock(now_ms)?;
        let rollback = now_ms < self.runtime.last_now
            || matches!((self.runtime.last_wall, wall_ms), (Some(before), Some(after)) if after < before);
        if rollback {
            self.runtime.ready_at = deadline(now_ms, COOLDOWN_MS);
            for (pending, entry) in self
                .runtime
                .slots
                .iter_mut()
                .zip(self.snapshot.entries.iter())
            {
                pending.ready_at = self.runtime.ready_at;
                pending.next_auto = entry
                    .and_then(|value| value.configuration.interval_ms)
                    .map(|interval| deadline(now_ms, interval));
            }
        }
        self.runtime.last_now = now_ms;
        // A missing wall observation does not erase rollback evidence.
        if wall_ms.is_some() {
            self.runtime.last_wall = wall_ms;
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error<S::Error>> {
        self.scratch.revision = self
            .snapshot
            .revision
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        self.scratch.validate()?;
        if let Err(error) = self
            .storage
            .compare_and_store(self.snapshot.revision, self.scratch)
        {
            self.runtime.poisoned = true;
            return Err(Error::Storage(error));
        }
        *self.snapshot = *self.scratch;
        Ok(())
    }
}

fn deadline(now: u64, delay: u64) -> u64 {
    now.saturating_add(delay)
}

fn validate_clock(now: u64) -> Result<(), Fault> {
    // A saturated deadline could look immediately eligible at u64::MAX. Stop
    // before this rather than allowing counter exhaustion to bypass the budget.
    now.checked_add(MAX_INTERVAL_MS + MAX_INTERVAL_MS / 4)
        .map(|_| ())
        .ok_or(Fault::Exhausted)
}

pub fn jittered_interval(interval_ms: u64, entropy: u16) -> u64 {
    // u128 keeps this helper safe even for callers outside validated policies.
    let extra = (u128::from(interval_ms) * u128::from(entropy)) / (4 * u128::from(u16::MAX));
    interval_ms.saturating_add(extra as u64)
}

fn validate_configuration(key: AnnounceKey, configuration: Configuration) -> Result<(), Fault> {
    if key.identity.incarnation == [0; 16]
        || key.identity.sequence == 0
        || configuration.channels == 0
        || configuration
            .interval_ms
            .is_some_and(|interval| !(COOLDOWN_MS..=MAX_INTERVAL_MS).contains(&interval))
    {
        return Err(Fault::InvalidConfiguration);
    }
    Ok(())
}

#[cfg(test)]
#[path = "announce_tests.rs"]
mod tests;
