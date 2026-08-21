use embedded_storage_async::nor_flash::NorFlash;
use heapless::Vec as HeaplessVec;

use crate::crypto::ratchets::SeedSelfRatchetsOutcome;
use crate::engine::{EngineState, InstantMillis, Journaled, RouteSeedOutcome};
use crate::identity::Zeroizing;
use crate::interfaces::AttachedInterfaces;
use crate::persistence::{
    maximum_route_upsert_payload_len, read_routing_table_snapshot, read_self_ratchets_snapshot,
    routing_table_snapshot_len, self_ratchets_snapshot_len, write_routing_table_snapshot,
    write_self_ratchets_snapshot, FlashJournal, FlashJournalError, FlashJournalLayout,
    FlashJournalRecord, FlashJournalRecordKind, FlashJournalWarning,
    TIMEBASE_RECORD_INTERVAL_MILLIS,
};
use crate::routing::announce::emit::MAX_ANNOUNCE_APP_DATA_LEN;
use crate::routing::AnnounceIdRing;
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};

const RECORD_SCRATCH_LEN: usize =
    (maximum_route_upsert_payload_len(MAX_ANNOUNCE_APP_DATA_LEN, 0) + 3) & !3;
const HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedCompactionPolicy {
    minimum_interval_millis: u64,
    critical_reserve_bytes: usize,
}

impl EmbeddedCompactionPolicy {
    #[must_use]
    pub const fn new(minimum_interval_millis: u64, critical_reserve_bytes: usize) -> Self {
        Self {
            minimum_interval_millis,
            critical_reserve_bytes,
        }
    }

    #[must_use]
    pub const fn hopspot(critical_reserve_bytes: usize) -> Self {
        Self::new(
            HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS,
            critical_reserve_bytes,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedPersistencePolicy {
    first_route_commit_delay_millis: u64,
    minimum_route_commit_interval_millis: u64,
    ratchet_batch_delay_millis: u64,
    retry_interval_millis: u64,
    timebase_record_interval_millis: u64,
    compaction: EmbeddedCompactionPolicy,
}

impl EmbeddedPersistencePolicy {
    #[must_use]
    pub const fn new(
        first_route_commit_delay_millis: u64,
        minimum_route_commit_interval_millis: u64,
        ratchet_batch_delay_millis: u64,
        retry_interval_millis: u64,
        timebase_record_interval_millis: u64,
        compaction: EmbeddedCompactionPolicy,
    ) -> Self {
        Self {
            first_route_commit_delay_millis,
            minimum_route_commit_interval_millis,
            ratchet_batch_delay_millis,
            retry_interval_millis,
            timebase_record_interval_millis,
            compaction,
        }
    }

    #[must_use]
    pub const fn hopspot_default(compaction: EmbeddedCompactionPolicy) -> Self {
        Self::new(
            2_000,
            5 * 60 * 1_000,
            2_000,
            5 * 60 * 1_000,
            TIMEBASE_RECORD_INTERVAL_MILLIS,
            compaction,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSnapshotKeyError {
    Capacity,
}

pub trait RouteSnapshotKeys {
    fn clear(&mut self);
    fn push(&mut self, destination: DestinationHash) -> Result<(), RouteSnapshotKeyError>;
    fn get(&self, index: usize) -> Option<DestinationHash>;
}

pub struct FixedRouteSnapshotKeys<const N: usize> {
    keys: HeaplessVec<DestinationHash, N>,
}

impl<const N: usize> FixedRouteSnapshotKeys<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            keys: HeaplessVec::new(),
        }
    }
}

impl<const N: usize> Default for FixedRouteSnapshotKeys<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RouteSnapshotKeys for FixedRouteSnapshotKeys<N> {
    fn clear(&mut self) {
        self.keys.clear();
    }

    fn push(&mut self, destination: DestinationHash) -> Result<(), RouteSnapshotKeyError> {
        self.keys
            .push(destination)
            .map_err(|_| RouteSnapshotKeyError::Capacity)
    }

    fn get(&self, index: usize) -> Option<DestinationHash> {
        self.keys.get(index).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedPersistenceFailure {
    Flash,
    Codec,
    Capacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedPersistenceTarget {
    Routes,
    CriticalState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedPersistenceRestoreReport {
    pub logical_start: InstantMillis,
    pub route_seeded_count: u32,
    pub route_refused_count: u32,
    pub route_dropped_count: u32,
    pub ratchet_seeded_count: u32,
    pub ratchet_refused_count: u32,
    pub warning: Option<FlashJournalWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedPersistenceDiagnostic {
    Restored(EmbeddedPersistenceRestoreReport),
    BatchPersisted {
        records: u32,
        at: InstantMillis,
        state_not_saved: bool,
    },
    CompactionStarted {
        at: InstantMillis,
        next_allowed_at: InstantMillis,
    },
    CompactionCompleted {
        records: u32,
        at: InstantMillis,
        state_not_saved: bool,
    },
    DurabilityDeferred {
        target: EmbeddedPersistenceTarget,
        until: InstantMillis,
    },
    WriteFailed {
        failure: EmbeddedPersistenceFailure,
        retry_at: InstantMillis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRouteDelta {
    RouteUpsert(DestinationHash),
    RouteRemoval(DestinationHash),
}

impl PendingRouteDelta {
    fn destination(self) -> DestinationHash {
        match self {
            Self::RouteUpsert(destination) | Self::RouteRemoval(destination) => destination,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchKind {
    Routes,
    Ratchets,
    Compaction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionPhase {
    RecordBudget { at: InstantMillis },
    Erase { sector: usize },
    Routes { index: usize },
    Ratchets { index: usize },
    Commit,
}

struct EncodedDelta {
    kind: FlashJournalRecordKind,
    payload: Zeroizing<[u8; RECORD_SCRATCH_LEN]>,
    len: usize,
}

pub struct EmbeddedFlashPersistence<F, Keys, Observe, const PENDING: usize>
where
    F: NorFlash,
    Keys: RouteSnapshotKeys,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
{
    flash: Option<F>,
    journal: Option<FlashJournal<F>>,
    layout: FlashJournalLayout,
    policy: EmbeddedPersistencePolicy,
    observe_diagnostic: Observe,
    pending_routes: HeaplessVec<PendingRouteDelta, PENDING>,
    pending_ratchets: HeaplessVec<DestinationHash, PENDING>,
    compaction_route_keys: Keys,
    compaction_ratchet_keys: HeaplessVec<DestinationHash, PENDING>,
    route_dirty_since: Option<InstantMillis>,
    ratchet_dirty_since: Option<InstantMillis>,
    last_route_success: Option<InstantMillis>,
    last_timebase_success: Option<InstantMillis>,
    retry_not_before: Option<InstantMillis>,
    landing_batch: Option<BatchKind>,
    landing_records: u32,
    compaction: Option<CompactionPhase>,
    compaction_target: Option<EmbeddedPersistenceTarget>,
    snapshot_required: bool,
    snapshot_target: EmbeddedPersistenceTarget,
    next_compaction_not_before: Option<InstantMillis>,
    deferred_target: Option<EmbeddedPersistenceTarget>,
    deferred_until: Option<InstantMillis>,
    write_failed: bool,
}

impl<F, Keys, Observe, const PENDING: usize> EmbeddedFlashPersistence<F, Keys, Observe, PENDING>
where
    F: NorFlash,
    Keys: RouteSnapshotKeys,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
{
    #[must_use]
    pub fn new(
        flash: F,
        layout: FlashJournalLayout,
        policy: EmbeddedPersistencePolicy,
        compaction_route_keys: Keys,
        observe_diagnostic: Observe,
    ) -> Self {
        Self {
            flash: Some(flash),
            journal: None,
            layout,
            policy,
            observe_diagnostic,
            pending_routes: HeaplessVec::new(),
            pending_ratchets: HeaplessVec::new(),
            compaction_route_keys,
            compaction_ratchet_keys: HeaplessVec::new(),
            route_dirty_since: None,
            ratchet_dirty_since: None,
            last_route_success: None,
            last_timebase_success: None,
            retry_not_before: None,
            landing_batch: None,
            landing_records: 0,
            compaction: None,
            compaction_target: None,
            snapshot_required: false,
            snapshot_target: EmbeddedPersistenceTarget::Routes,
            next_compaction_not_before: None,
            deferred_target: None,
            deferred_until: None,
            write_failed: false,
        }
    }

    #[must_use]
    pub fn state_not_saved(&self) -> bool {
        self.write_failed || self.deferred_target.is_some()
    }

    pub async fn restore<S: StorageLayout>(
        &mut self,
        engine: &mut EngineState<S>,
        raw_now: InstantMillis,
    ) -> EmbeddedPersistenceRestoreReport {
        let Some(mut flash) = self.flash.take() else {
            return self.empty_restore_report(raw_now, Some(FlashJournalWarning::Corrupt));
        };
        let timebase_state = FlashJournal::inspect_timebase_state(&mut flash, self.layout)
            .await
            .ok();
        let logical_start = timebase_state
            .and_then(|state| state.high_water)
            .map_or(raw_now, |high_water| high_water.max(raw_now));
        let mut scratch = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
        let mut report = EmbeddedPersistenceRestoreReport {
            logical_start,
            route_seeded_count: 0,
            route_refused_count: 0,
            route_dropped_count: 0,
            ratchet_seeded_count: 0,
            ratchet_refused_count: 0,
            warning: None,
        };
        let opened = FlashJournal::open(flash, self.layout, &mut scratch[..], |record| {
            apply_record(engine, logical_start, record, &mut report)
        })
        .await;
        let Ok((mut journal, restored)) = opened else {
            report.warning = Some(FlashJournalWarning::Corrupt);
            (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::Restored(report));
            return report;
        };
        report.warning = restored.warning;
        let initialization_failed =
            restored.active_epoch.is_none() && journal.initialize_empty().await.is_err();
        self.next_compaction_not_before = timebase_state
            .and_then(|state| state.last_compaction_attempt)
            .map(|attempt| {
                InstantMillis(
                    attempt
                        .0
                        .saturating_add(self.policy.compaction.minimum_interval_millis),
                )
            });
        self.journal = Some(journal);
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::Restored(report));
        if initialization_failed {
            self.note_write_failure(raw_now, EmbeddedPersistenceFailure::Flash);
        }
        report
    }

    fn empty_restore_report(
        &mut self,
        logical_start: InstantMillis,
        warning: Option<FlashJournalWarning>,
    ) -> EmbeddedPersistenceRestoreReport {
        let report = EmbeddedPersistenceRestoreReport {
            logical_start,
            route_seeded_count: 0,
            route_refused_count: 0,
            route_dropped_count: 0,
            ratchet_seeded_count: 0,
            ratchet_refused_count: 0,
            warning,
        };
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::Restored(report));
        report
    }

    fn observe_journaled(&mut self, journaled: &Journaled<'_>, now: InstantMillis) {
        match journaled {
            Journaled::AnnounceHeard { observation, .. } => {
                self.queue_route(PendingRouteDelta::RouteUpsert(observation.destination), now);
                if self.route_dirty_since.is_none() {
                    self.route_dirty_since = Some(now);
                }
            }
            Journaled::RouteRemoved { destination, .. } => {
                self.queue_route(PendingRouteDelta::RouteRemoval(*destination), now);
                if self.route_dirty_since.is_none() {
                    self.route_dirty_since = Some(now);
                }
            }
            Journaled::SelfRatchetRotated { destination } => {
                self.queue_ratchet(*destination, now);
                if self.ratchet_dirty_since.is_none() {
                    self.ratchet_dirty_since = Some(now);
                }
            }
            Journaled::AnnounceHeldDropped { .. }
            | Journaled::Delivered(_)
            | Journaled::CommandSettled { .. }
            | Journaled::PersistenceFlushed { .. }
            | Journaled::PersistenceFlushFailed { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::LinkInterfaceMismatch { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. } => {}
        }
    }

    fn queue_route(&mut self, delta: PendingRouteDelta, now: InstantMillis) {
        if self.snapshot_required {
            return;
        }
        if let Some(existing) = self
            .pending_routes
            .iter_mut()
            .find(|pending| pending.destination() == delta.destination())
        {
            *existing = delta;
            return;
        }
        if self.pending_routes.push(delta).is_err() {
            self.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
        }
    }

    fn queue_ratchet(&mut self, destination: DestinationHash, now: InstantMillis) {
        if self.pending_ratchets.contains(&destination) {
            return;
        }
        if self.pending_ratchets.push(destination).is_err() {
            self.require_snapshot(EmbeddedPersistenceTarget::CriticalState, now);
        }
    }

    fn require_snapshot(&mut self, target: EmbeddedPersistenceTarget, now: InstantMillis) {
        self.snapshot_required = true;
        self.snapshot_target = match (self.snapshot_target, target) {
            (EmbeddedPersistenceTarget::CriticalState, _)
            | (_, EmbeddedPersistenceTarget::CriticalState) => {
                EmbeddedPersistenceTarget::CriticalState
            }
            (EmbeddedPersistenceTarget::Routes, EmbeddedPersistenceTarget::Routes) => {
                EmbeddedPersistenceTarget::Routes
            }
        };
        self.pending_routes.clear();
        if target == EmbeddedPersistenceTarget::CriticalState {
            self.pending_ratchets.clear();
        }
        let Some(until) = self.next_compaction_not_before else {
            return;
        };
        if now.0 >= until.0 {
            return;
        }
        if self.deferred_target == Some(self.snapshot_target) && self.deferred_until == Some(until)
        {
            return;
        }
        self.deferred_target = Some(self.snapshot_target);
        self.deferred_until = Some(until);
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::DurabilityDeferred {
            target: self.snapshot_target,
            until,
        });
    }

    fn next_deadline(&self, now: InstantMillis) -> Option<InstantMillis> {
        self.journal.as_ref()?;
        let mut deadline = if self.compaction.is_some() || self.landing_batch.is_some() {
            Some(now)
        } else {
            None
        };
        let ratchet_ready = self.ratchet_ready_at();
        let route_ready = self.route_ready_at();
        deadline = earlier(deadline, ratchet_ready);
        if self.snapshot_required {
            let requested = match self.snapshot_target {
                EmbeddedPersistenceTarget::Routes => route_ready,
                EmbeddedPersistenceTarget::CriticalState => {
                    earlier(route_ready, ratchet_ready).or(Some(now))
                }
            };
            if let Some(requested) = requested {
                let allowed = self.next_compaction_not_before.unwrap_or(requested);
                deadline = earlier(deadline, Some(InstantMillis(requested.0.max(allowed.0))));
            }
        } else {
            deadline = earlier(deadline, route_ready);
        }
        let timebase_ready = self.last_timebase_success.map_or(now, |last| {
            InstantMillis(
                last.0
                    .saturating_add(self.policy.timebase_record_interval_millis),
            )
        });
        deadline = earlier(deadline, Some(timebase_ready));
        match (deadline, self.retry_not_before) {
            (Some(deadline), Some(retry)) => Some(InstantMillis(deadline.0.max(retry.0))),
            (deadline, None) => deadline,
            (None, Some(_)) => None,
        }
    }

    fn ratchet_ready_at(&self) -> Option<InstantMillis> {
        self.ratchet_dirty_since.map(|dirty| {
            InstantMillis(
                dirty
                    .0
                    .saturating_add(self.policy.ratchet_batch_delay_millis),
            )
        })
    }

    fn route_ready_at(&self) -> Option<InstantMillis> {
        self.route_dirty_since.map(|dirty| {
            let first_ready = dirty
                .0
                .saturating_add(self.policy.first_route_commit_delay_millis);
            let interval_ready = self.last_route_success.map_or(0, |last| {
                last.0
                    .saturating_add(self.policy.minimum_route_commit_interval_millis)
            });
            InstantMillis(first_ready.max(interval_ready))
        })
    }

    async fn progress<S: StorageLayout>(
        &mut self,
        engine: &mut EngineState<S>,
        now: InstantMillis,
    ) {
        if self.retry_not_before.is_some_and(|retry| now.0 < retry.0) {
            return;
        }
        if self.compaction.is_some() {
            self.progress_compaction(engine, now).await;
            return;
        }
        if let Some(batch) = self.landing_batch {
            let new_work = match batch {
                BatchKind::Routes => !self.pending_routes.is_empty() || self.snapshot_required,
                BatchKind::Ratchets => !self.pending_ratchets.is_empty(),
                BatchKind::Compaction => {
                    !self.pending_routes.is_empty()
                        || !self.pending_ratchets.is_empty()
                        || self.snapshot_required
                }
            };
            if new_work {
                self.landing_batch = None;
            } else {
                self.land_timebase(batch, now).await;
                return;
            }
        }
        let ratchet_due = self
            .ratchet_ready_at()
            .is_some_and(|ready| now.0 >= ready.0);
        let route_due = self.route_ready_at().is_some_and(|ready| now.0 >= ready.0);
        if ratchet_due {
            if let Some(index) = (!self.pending_ratchets.is_empty()).then_some(0) {
                self.append_ratchet(engine, index, now).await;
                return;
            }
            if self.snapshot_required
                && self.snapshot_target == EmbeddedPersistenceTarget::CriticalState
            {
                self.try_start_compaction(engine, now);
                return;
            }
        }
        if !route_due {
            let timebase_due = self.last_timebase_success.is_none_or(|last| {
                now.0.saturating_sub(last.0) >= self.policy.timebase_record_interval_millis
            });
            if timebase_due {
                self.record_timebase(now).await;
            }
            return;
        }
        if self.snapshot_required {
            self.try_start_compaction(engine, now);
            return;
        }
        if !self.pending_routes.is_empty() {
            self.append_route(engine, 0, now).await;
        }
    }

    async fn append_route<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        index: usize,
        now: InstantMillis,
    ) {
        let delta = self.pending_routes[index];
        let encoded = encode_route_delta(engine, delta);
        let Ok(payload) = encoded else {
            self.note_codec_failure(now);
            return;
        };
        let can_fit = self.journal.as_ref().is_some_and(|journal| {
            journal.active_can_fit(payload.len, self.policy.compaction.critical_reserve_bytes)
        });
        if !can_fit {
            self.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
            self.try_start_compaction(engine, now);
            return;
        }
        let Some(journal) = self.journal.as_mut() else {
            return;
        };
        let result = journal
            .append(payload.kind, &payload.payload[..payload.len])
            .await;
        match result {
            Ok(()) => {
                self.pending_routes.swap_remove(index);
                self.landing_records = self.landing_records.saturating_add(1);
                if self.pending_routes.is_empty() {
                    self.landing_batch = Some(BatchKind::Routes);
                }
            }
            Err(FlashJournalError::ArenaFull) => {
                self.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
                self.try_start_compaction(engine, now);
            }
            Err(error) => {
                let failure = failure_from_journal(error);
                self.note_write_failure(now, failure);
            }
        }
    }

    async fn append_ratchet<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        index: usize,
        now: InstantMillis,
    ) {
        let destination = self.pending_ratchets[index];
        let Ok(payload) = encode_ratchet(engine, destination) else {
            self.note_codec_failure(now);
            return;
        };
        let Some(journal) = self.journal.as_mut() else {
            return;
        };
        let result = journal
            .append(payload.kind, &payload.payload[..payload.len])
            .await;
        match result {
            Ok(()) => {
                self.pending_ratchets.swap_remove(index);
                self.landing_records = self.landing_records.saturating_add(1);
                if self.pending_ratchets.is_empty() {
                    self.landing_batch = Some(BatchKind::Ratchets);
                }
            }
            Err(FlashJournalError::ArenaFull) => {
                self.require_snapshot(EmbeddedPersistenceTarget::CriticalState, now);
                self.try_start_compaction(engine, now);
            }
            Err(error) => self.note_write_failure(now, failure_from_journal(error)),
        }
    }

    fn try_start_compaction<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        now: InstantMillis,
    ) {
        let allowed = self.next_compaction_not_before.unwrap_or(now);
        if now.0 < allowed.0 {
            self.require_snapshot(self.snapshot_target, now);
            return;
        }
        self.compaction_route_keys.clear();
        let mut route_capacity_failed = false;
        for destination in engine.persisted_route_destinations() {
            if self.compaction_route_keys.push(destination).is_err() {
                route_capacity_failed = true;
                break;
            }
        }
        if route_capacity_failed {
            self.note_write_failure(now, EmbeddedPersistenceFailure::Capacity);
            return;
        }
        self.compaction_ratchet_keys.clear();
        let mut ratchet_capacity_failed = false;
        for (destination, _, _) in engine.persisted_self_ratchet_rows() {
            if self.compaction_ratchet_keys.push(destination).is_err() {
                ratchet_capacity_failed = true;
                break;
            }
        }
        if ratchet_capacity_failed {
            self.note_write_failure(now, EmbeddedPersistenceFailure::Capacity);
            return;
        }
        let target = self.snapshot_target;
        self.pending_routes.clear();
        self.pending_ratchets.clear();
        self.route_dirty_since = None;
        self.ratchet_dirty_since = None;
        self.snapshot_required = false;
        self.snapshot_target = EmbeddedPersistenceTarget::Routes;
        self.compaction_target = Some(target);
        self.compaction = Some(CompactionPhase::RecordBudget { at: now });
        self.landing_batch = None;
        self.landing_records = 0;
    }

    async fn progress_compaction<S: StorageLayout>(
        &mut self,
        engine: &mut EngineState<S>,
        now: InstantMillis,
    ) {
        let Some(phase) = self.compaction else {
            return;
        };
        let Some(journal) = self.journal.as_mut() else {
            return;
        };
        match phase {
            CompactionPhase::RecordBudget { at } => {
                match journal.record_compaction_budget(at).await {
                    Ok(recorded_at) => {
                        self.last_timebase_success = Some(at);
                        let next_allowed_at = InstantMillis(
                            recorded_at
                                .0
                                .saturating_add(self.policy.compaction.minimum_interval_millis),
                        );
                        self.next_compaction_not_before = Some(next_allowed_at);
                        (self.observe_diagnostic)(
                            EmbeddedPersistenceDiagnostic::CompactionStarted {
                                at,
                                next_allowed_at,
                            },
                        );
                        self.compaction = Some(CompactionPhase::Erase { sector: 0 });
                    }
                    Err(error) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::Erase { sector } => {
                if sector < journal.inactive_sector_count() {
                    match journal.erase_inactive_sector(sector).await {
                        Ok(()) => {
                            let next = sector + 1;
                            if next == journal.inactive_sector_count() {
                                if journal.begin_compaction().is_err() {
                                    self.note_write_failure(
                                        now,
                                        EmbeddedPersistenceFailure::Capacity,
                                    );
                                    return;
                                }
                                self.compaction = Some(CompactionPhase::Routes { index: 0 });
                            } else {
                                self.compaction = Some(CompactionPhase::Erase { sector: next });
                            }
                        }
                        Err(error) => {
                            self.note_write_failure(now, failure_from_journal(error));
                        }
                    }
                }
            }
            CompactionPhase::Routes { index } => {
                let mut scratch = [0u8; RECORD_SCRATCH_LEN];
                let Some(destination) = self.compaction_route_keys.get(index) else {
                    self.compaction = Some(CompactionPhase::Ratchets { index: 0 });
                    return;
                };
                let Some(row) = engine.persisted_route_row(&destination) else {
                    self.compaction = Some(CompactionPhase::Routes { index: index + 1 });
                    return;
                };
                let mut durable = row.clone();
                durable.announce_id_ring = AnnounceIdRing::Table(&[]);
                let required = routing_table_snapshot_len(core::iter::once(durable.clone()));
                if required > scratch.len() {
                    self.note_codec_failure(now);
                    return;
                }
                let Ok(written) = write_routing_table_snapshot(
                    core::iter::once(durable),
                    &mut scratch[..required],
                ) else {
                    self.note_codec_failure(now);
                    return;
                };
                match journal
                    .append_compacted(FlashJournalRecordKind::RouteUpsert, &scratch[..written])
                    .await
                {
                    Ok(()) => {
                        self.landing_records = self.landing_records.saturating_add(1);
                        self.compaction = Some(CompactionPhase::Routes { index: index + 1 });
                    }
                    Err(error) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::Ratchets { index } => {
                let Some(destination) = self.compaction_ratchet_keys.get(index).copied() else {
                    self.compaction = Some(CompactionPhase::Commit);
                    return;
                };
                let Some((last_rotated, secrets)) = engine.persisted_self_ratchet_row(&destination)
                else {
                    self.compaction = Some(CompactionPhase::Ratchets { index: index + 1 });
                    return;
                };
                let mut scratch = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
                scratch[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(destination.as_bytes());
                let required = self_ratchets_snapshot_len(secrets.len());
                let end = TRUNCATED_HASH_BYTE_LEN.saturating_add(required);
                if end > scratch.len() {
                    self.note_codec_failure(now);
                    return;
                }
                let Ok(written) = write_self_ratchets_snapshot(
                    last_rotated,
                    secrets,
                    &mut scratch[TRUNCATED_HASH_BYTE_LEN..end],
                ) else {
                    self.note_codec_failure(now);
                    return;
                };
                match journal
                    .append_compacted(
                        FlashJournalRecordKind::SelfRatchet,
                        &scratch[..TRUNCATED_HASH_BYTE_LEN + written],
                    )
                    .await
                {
                    Ok(()) => {
                        self.landing_records = self.landing_records.saturating_add(1);
                        self.compaction = Some(CompactionPhase::Ratchets { index: index + 1 });
                    }
                    Err(error) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::Commit => match journal.commit_compaction().await {
                Ok(()) => {
                    self.compaction = None;
                    self.compaction_target = None;
                    let records = core::mem::take(&mut self.landing_records);
                    self.retry_not_before = None;
                    self.write_failed = false;
                    if self.snapshot_required {
                        self.require_snapshot(self.snapshot_target, now);
                    } else {
                        self.deferred_target = None;
                        self.deferred_until = None;
                    }
                    let state_not_saved = self.state_not_saved();
                    (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::CompactionCompleted {
                        records,
                        at: now,
                        state_not_saved,
                    });
                    if !self.snapshot_required
                        && self.pending_routes.is_empty()
                        && self.pending_ratchets.is_empty()
                    {
                        self.landing_batch = Some(BatchKind::Compaction);
                    }
                }
                Err(error) => {
                    self.note_write_failure(now, failure_from_journal(error));
                }
            },
        }
    }

    async fn land_timebase(&mut self, batch: BatchKind, now: InstantMillis) {
        if !self.record_timebase(now).await {
            return;
        }
        match batch {
            BatchKind::Routes => {
                self.route_dirty_since = None;
                self.last_route_success = Some(now);
            }
            BatchKind::Ratchets => {
                self.ratchet_dirty_since = None;
            }
            BatchKind::Compaction => {
                self.route_dirty_since = None;
                self.ratchet_dirty_since = None;
                self.last_route_success = Some(now);
            }
        }
        self.retry_not_before = None;
        self.write_failed = false;
        let records = core::mem::take(&mut self.landing_records);
        self.landing_batch = None;
        if batch != BatchKind::Compaction {
            let state_not_saved = self.state_not_saved();
            (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::BatchPersisted {
                records,
                at: now,
                state_not_saved,
            });
        }
    }

    async fn record_timebase(&mut self, now: InstantMillis) -> bool {
        let should_record = self.last_timebase_success.is_none_or(|last| {
            now.0.saturating_sub(last.0) >= self.policy.timebase_record_interval_millis
        });
        if !should_record {
            return true;
        }
        let Some(journal) = self.journal.as_mut() else {
            return false;
        };
        if let Err(error) = journal.record_timebase(now).await {
            self.note_write_failure(now, failure_from_journal(error));
            return false;
        }
        self.last_timebase_success = Some(now);
        self.retry_not_before = None;
        self.write_failed = false;
        true
    }

    fn note_codec_failure(&mut self, now: InstantMillis) {
        self.note_write_failure(now, EmbeddedPersistenceFailure::Codec);
    }

    fn note_write_failure(&mut self, now: InstantMillis, failure: EmbeddedPersistenceFailure) {
        let retry_at = InstantMillis(now.0.saturating_add(self.policy.retry_interval_millis));
        self.retry_not_before = Some(retry_at);
        self.write_failed = true;
        if self.compaction.is_some() {
            let target = self
                .compaction_target
                .unwrap_or(EmbeddedPersistenceTarget::Routes);
            if let Some(journal) = self.journal.as_mut() {
                journal.abort_compaction();
            }
            self.compaction = None;
            self.compaction_target = None;
            self.require_snapshot(target, now);
            match target {
                EmbeddedPersistenceTarget::Routes => {
                    self.route_dirty_since.get_or_insert(now);
                }
                EmbeddedPersistenceTarget::CriticalState => {
                    self.ratchet_dirty_since.get_or_insert(now);
                }
            }
        }
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::WriteFailed { failure, retry_at });
    }
}

pub(crate) trait ManifoldPersistence<S: StorageLayout> {
    fn observe(&mut self, journaled: &Journaled<'_>, now: InstantMillis);
    fn deadline(&self, now: InstantMillis) -> Option<InstantMillis>;
    async fn progress(&mut self, engine: &mut EngineState<S>, now: InstantMillis);
}

impl<S, F, Keys, Observe, const PENDING: usize> ManifoldPersistence<S>
    for EmbeddedFlashPersistence<F, Keys, Observe, PENDING>
where
    S: StorageLayout,
    F: NorFlash,
    Keys: RouteSnapshotKeys,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
{
    fn observe(&mut self, journaled: &Journaled<'_>, now: InstantMillis) {
        self.observe_journaled(journaled, now);
    }

    fn deadline(&self, now: InstantMillis) -> Option<InstantMillis> {
        self.next_deadline(now)
    }

    async fn progress(&mut self, engine: &mut EngineState<S>, now: InstantMillis) {
        self.progress(engine, now).await;
    }
}

pub(crate) struct NoManifoldPersistence;

impl<S: StorageLayout> ManifoldPersistence<S> for NoManifoldPersistence {
    fn observe(&mut self, _journaled: &Journaled<'_>, _now: InstantMillis) {}

    fn deadline(&self, _now: InstantMillis) -> Option<InstantMillis> {
        None
    }

    async fn progress(&mut self, _engine: &mut EngineState<S>, _now: InstantMillis) {}
}

fn encode_route_delta<S: StorageLayout>(
    engine: &EngineState<S>,
    delta: PendingRouteDelta,
) -> Result<EncodedDelta, ()> {
    match delta {
        PendingRouteDelta::RouteUpsert(destination) => {
            let Some(row) = engine.persisted_route_row(&destination) else {
                return encode_tombstone(destination);
            };
            let mut durable = row.clone();
            durable.announce_id_ring = AnnounceIdRing::Table(&[]);
            let required = routing_table_snapshot_len(core::iter::once(durable.clone()));
            if required > RECORD_SCRATCH_LEN {
                return Err(());
            }
            let mut scratch = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
            let written =
                write_routing_table_snapshot(core::iter::once(durable), &mut scratch[..required])
                    .map_err(|_| ())?;
            Ok(EncodedDelta {
                kind: FlashJournalRecordKind::RouteUpsert,
                payload: scratch,
                len: written,
            })
        }
        PendingRouteDelta::RouteRemoval(destination) => encode_tombstone(destination),
    }
}

fn encode_ratchet<S: StorageLayout>(
    engine: &EngineState<S>,
    destination: DestinationHash,
) -> Result<EncodedDelta, ()> {
    let Some((last_rotated, secrets)) = engine.persisted_self_ratchet_row(&destination) else {
        return Err(());
    };
    let required = self_ratchets_snapshot_len(secrets.len());
    if TRUNCATED_HASH_BYTE_LEN + required > RECORD_SCRATCH_LEN {
        return Err(());
    }
    let mut scratch = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
    scratch[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(destination.as_bytes());
    let written = write_self_ratchets_snapshot(
        last_rotated,
        secrets,
        &mut scratch[TRUNCATED_HASH_BYTE_LEN..TRUNCATED_HASH_BYTE_LEN + required],
    )
    .map_err(|_| ())?;
    Ok(EncodedDelta {
        kind: FlashJournalRecordKind::SelfRatchet,
        payload: scratch,
        len: TRUNCATED_HASH_BYTE_LEN + written,
    })
}

fn encode_tombstone(destination: DestinationHash) -> Result<EncodedDelta, ()> {
    let mut payload = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
    payload[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(destination.as_bytes());
    Ok(EncodedDelta {
        kind: FlashJournalRecordKind::RouteRemoval,
        payload,
        len: TRUNCATED_HASH_BYTE_LEN,
    })
}

fn apply_record<S: StorageLayout>(
    engine: &mut EngineState<S>,
    now: InstantMillis,
    record: FlashJournalRecord<'_>,
    report: &mut EmbeddedPersistenceRestoreReport,
) {
    match record.kind {
        FlashJournalRecordKind::ArenaCommit => {}
        FlashJournalRecordKind::RouteUpsert => {
            let Ok(mut rows) = read_routing_table_snapshot(record.payload) else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            let Some(Ok(row)) = rows.next() else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            if rows.next().is_some() {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            }
            let destination = row.destination;
            let Ok(pending) = engine.prepare_persisted_route(row) else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            let Ok(verified) = pending.verify() else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            let _ = engine.drop_route(&destination, AttachedInterfaces::new(&[]));
            match engine.seed_verified_route(verified, now) {
                RouteSeedOutcome::Seeded => {
                    report.route_seeded_count = report.route_seeded_count.saturating_add(1);
                }
                RouteSeedOutcome::RefusedDestinationMismatch
                | RouteSeedOutcome::RefusedBlackholedIdentity
                | RouteSeedOutcome::RefusedInvalidSignature => {
                    report.route_refused_count = report.route_refused_count.saturating_add(1);
                }
                RouteSeedOutcome::AlreadyPresent
                | RouteSeedOutcome::TableFull
                | RouteSeedOutcome::AppDataArenaFull => {
                    report.route_dropped_count = report.route_dropped_count.saturating_add(1);
                }
            }
        }
        FlashJournalRecordKind::RouteRemoval => {
            let Ok(bytes) = <[u8; TRUNCATED_HASH_BYTE_LEN]>::try_from(record.payload) else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            let destination = DestinationHash::new(bytes);
            let _ = engine.drop_route(&destination, AttachedInterfaces::new(&[]));
        }
        FlashJournalRecordKind::SelfRatchet => {
            let Some((destination, sealed)) = record
                .payload
                .split_first_chunk::<TRUNCATED_HASH_BYTE_LEN>()
            else {
                report.ratchet_refused_count = report.ratchet_refused_count.saturating_add(1);
                return;
            };
            let Ok(restored) = read_self_ratchets_snapshot(sealed) else {
                report.ratchet_refused_count = report.ratchet_refused_count.saturating_add(1);
                return;
            };
            match engine.replace_persisted_self_ratchets(
                &DestinationHash::new(*destination),
                restored.last_rotated,
                restored.secrets_newest_first(),
            ) {
                SeedSelfRatchetsOutcome::Seeded => {
                    report.ratchet_seeded_count = report.ratchet_seeded_count.saturating_add(1);
                }
                SeedSelfRatchetsOutcome::AlreadyMinted | SeedSelfRatchetsOutcome::Untracked => {
                    report.ratchet_refused_count = report.ratchet_refused_count.saturating_add(1);
                }
            }
        }
    }
}

fn failure_from_journal<E>(error: FlashJournalError<E>) -> EmbeddedPersistenceFailure {
    match error {
        FlashJournalError::Flash(_) => EmbeddedPersistenceFailure::Flash,
        FlashJournalError::ArenaFull
        | FlashJournalError::OutOfBounds
        | FlashJournalError::Misaligned
        | FlashJournalError::Uninitialized
        | FlashJournalError::CompactionInProgress
        | FlashJournalError::NoCompaction
        | FlashJournalError::PayloadTooLarge
        | FlashJournalError::ScratchTooShort => EmbeddedPersistenceFailure::Capacity,
    }
}

fn earlier(first: Option<InstantMillis>, second: Option<InstantMillis>) -> Option<InstantMillis> {
    match (first, second) {
        (Some(first), Some(second)) => Some(InstantMillis(first.0.min(second.0))),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::TIMEBASE_HEADROOM_MILLIS;
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind};
    use embedded_storage_async::nor_flash::ReadNorFlash;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::vec::Vec;

    const ERASE: usize = 512;
    const CAPACITY: usize = ERASE * 6;
    const LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
        [0, ERASE as u32],
        [
            crate::persistence::FlashArenaRange::new((ERASE * 2) as u32, (ERASE * 4) as u32),
            crate::persistence::FlashArenaRange::new((ERASE * 4) as u32, (ERASE * 6) as u32),
        ],
    );

    #[derive(Debug)]
    struct TestFlash {
        bytes: [u8; CAPACITY],
        sector_erases: [u32; CAPACITY / ERASE],
        fail_next_write: Rc<Cell<bool>>,
    }

    impl TestFlash {
        fn new() -> Self {
            Self {
                bytes: [0xFF; CAPACITY],
                sector_erases: [0; CAPACITY / ERASE],
                fail_next_write: Rc::new(Cell::new(false)),
            }
        }

        fn controlled() -> (Self, Rc<Cell<bool>>) {
            let flash = Self::new();
            let control = Rc::clone(&flash.fail_next_write);
            (flash, control)
        }
    }

    #[derive(Debug)]
    struct TestFlashError;

    impl NorFlashError for TestFlashError {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }

    impl ErrorType for TestFlash {
        type Error = TestFlashError;
    }

    impl ReadNorFlash for TestFlash {
        const READ_SIZE: usize = 4;

        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let start = offset as usize;
            let end = start + bytes.len();
            bytes.copy_from_slice(&self.bytes[start..end]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            CAPACITY
        }
    }

    impl NorFlash for TestFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = ERASE;

        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            if self.fail_next_write.replace(false) {
                return Err(TestFlashError);
            }
            let start = offset as usize;
            for (stored, written) in self.bytes[start..start + bytes.len()].iter_mut().zip(bytes) {
                *stored &= *written;
            }
            Ok(())
        }

        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            self.bytes[from as usize..to as usize].fill(0xFF);
            for sector in from as usize / ERASE..to as usize / ERASE {
                self.sector_erases[sector] = self.sector_erases[sector].saturating_add(1);
            }
            Ok(())
        }
    }

    fn ready_with_observer<Observe>(
        observe: Observe,
    ) -> EmbeddedFlashPersistence<TestFlash, FixedRouteSnapshotKeys<8>, Observe, 4>
    where
        Observe: FnMut(EmbeddedPersistenceDiagnostic),
    {
        embassy_futures::block_on(async {
            let flash = TestFlash::new();
            let mut scratch = [0u8; RECORD_SCRATCH_LEN];
            let (mut journal, _) = FlashJournal::open(flash, LAYOUT, &mut scratch, |_| {})
                .await
                .unwrap();
            journal.initialize_empty().await.unwrap();
            journal
                .record_compaction_budget(InstantMillis(0))
                .await
                .unwrap();
            let mut persistence = EmbeddedFlashPersistence::new(
                TestFlash::new(),
                LAYOUT,
                EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
                FixedRouteSnapshotKeys::new(),
                observe,
            );
            persistence.flash = None;
            persistence.journal = Some(journal);
            persistence.next_compaction_not_before =
                Some(InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS));
            persistence.last_timebase_success = Some(InstantMillis(0));
            persistence
        })
    }

    fn ready() -> EmbeddedFlashPersistence<
        TestFlash,
        FixedRouteSnapshotKeys<8>,
        fn(EmbeddedPersistenceDiagnostic),
        4,
    > {
        ready_with_observer((|_| {}) as fn(EmbeddedPersistenceDiagnostic))
    }

    fn signed_route(secret: u8, app_data: &[u8]) -> crate::routing::PersistedRouteRow<'_> {
        use crate::identity::in_memory::InMemoryNodeIdentity;
        use crate::interfaces::InterfaceId;
        use crate::routing::announce::{Announce, AnnounceId, DottedNameHash};
        use crate::routing::routes::RouteEntry;
        use crate::routing::{AnnounceIdRing, NextHop, RouteResponsiveness};

        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[secret; 64]);
        let announce = Announce::build_signed(
            &signer,
            DottedNameHash::new([secret; 10]),
            AnnounceId::from_wire([secret.wrapping_add(1); 10]),
            None,
            app_data,
        )
        .unwrap();
        crate::routing::PersistedRouteRow {
            destination: announce.destination,
            entry: RouteEntry {
                hops: secret,
                learned_at: InstantMillis(500),
                last_route_activity_at: InstantMillis(700),
                responsiveness: RouteResponsiveness::Responsive,
                receiving_interface: InterfaceId::new([secret; 8]),
                next_hop: NextHop::Direct,
            },
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            announce_id: announce.announce_id,
            ratchet: announce.ratchet,
            signature: announce.signature,
            app_data,
            announce_id_ring: AnnounceIdRing::Wire(
                &[0; crate::routing::announce::ANNOUNCE_ID_WIRE_LEN],
            ),
        }
    }

    #[test]
    fn exact_route_and_ratchet_deadlines_are_distinct() {
        let policy =
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(64));
        assert_eq!(policy.first_route_commit_delay_millis, 2_000);
        assert_eq!(policy.minimum_route_commit_interval_millis, 300_000);
        assert_eq!(policy.ratchet_batch_delay_millis, 2_000);
        assert_eq!(policy.retry_interval_millis, 300_000);
        assert_eq!(
            policy.timebase_record_interval_millis,
            TIMEBASE_RECORD_INTERVAL_MILLIS
        );
        assert_eq!(
            policy.compaction.minimum_interval_millis,
            HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS
        );
        assert_eq!(policy.compaction.critical_reserve_bytes, 64);
    }

    #[test]
    fn deadline_formula_batches_first_write_then_honors_five_minutes() {
        let mut persistence = ready();
        persistence.route_dirty_since = Some(InstantMillis(1_000));
        assert_eq!(
            persistence.next_deadline(InstantMillis(1_500)),
            Some(InstantMillis(3_000))
        );
        persistence.last_route_success = Some(InstantMillis(10_000));
        persistence.route_dirty_since = Some(InstantMillis(11_000));
        assert_eq!(
            persistence.next_deadline(InstantMillis(12_000)),
            Some(InstantMillis(310_000))
        );
        persistence.ratchet_dirty_since = Some(InstantMillis(12_000));
        assert_eq!(
            persistence.next_deadline(InstantMillis(12_000)),
            Some(InstantMillis(14_000))
        );
        persistence.retry_not_before = Some(InstantMillis(400_000));
        assert_eq!(
            persistence.next_deadline(InstantMillis(12_000)),
            Some(InstantMillis(400_000))
        );
    }

    #[test]
    fn repeated_route_and_ratchet_changes_coalesce_by_destination() {
        let mut persistence = ready();
        let destination = DestinationHash::new([0x11; TRUNCATED_HASH_BYTE_LEN]);
        persistence.queue_route(
            PendingRouteDelta::RouteUpsert(destination),
            InstantMillis(0),
        );
        persistence.queue_route(
            PendingRouteDelta::RouteUpsert(destination),
            InstantMillis(0),
        );
        persistence.queue_route(
            PendingRouteDelta::RouteRemoval(destination),
            InstantMillis(0),
        );
        persistence.queue_ratchet(destination, InstantMillis(0));
        persistence.queue_ratchet(destination, InstantMillis(0));
        assert_eq!(
            persistence.pending_routes.as_slice(),
            &[PendingRouteDelta::RouteRemoval(destination)]
        );
        assert_eq!(persistence.pending_ratchets.as_slice(), &[destination]);
    }

    #[test]
    fn pending_overflow_waits_for_the_batch_deadline_before_compacting() {
        let mut persistence = ready();
        for byte in 0..5 {
            persistence.queue_route(
                PendingRouteDelta::RouteUpsert(DestinationHash::new(
                    [byte; TRUNCATED_HASH_BYTE_LEN],
                )),
                InstantMillis(1_000),
            );
        }
        persistence.route_dirty_since = Some(InstantMillis(1_000));
        assert!(persistence.snapshot_required);
        assert_eq!(
            persistence.deferred_target,
            Some(EmbeddedPersistenceTarget::Routes)
        );
        assert_eq!(
            persistence.next_deadline(InstantMillis(1_500)),
            Some(InstantMillis(TIMEBASE_RECORD_INTERVAL_MILLIS))
        );

        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        embassy_futures::block_on(persistence.progress(&mut engine, InstantMillis(2_999)));
        assert_eq!(persistence.compaction, None);
        assert!(persistence.snapshot_required);

        embassy_futures::block_on(persistence.progress(
            &mut engine,
            InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS),
        ));
        assert_eq!(
            persistence.compaction,
            Some(CompactionPhase::RecordBudget {
                at: InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS)
            })
        );
    }

    #[test]
    fn failures_keep_dirty_state_and_raise_the_notice() {
        let mut persistence = ready();
        let destination = DestinationHash::new([0x22; TRUNCATED_HASH_BYTE_LEN]);
        persistence.queue_route(
            PendingRouteDelta::RouteUpsert(destination),
            InstantMillis(0),
        );
        persistence.note_codec_failure(InstantMillis(1_000));
        assert_eq!(persistence.pending_routes.len(), 1);
        assert_eq!(persistence.retry_not_before, Some(InstantMillis(301_000)));
        assert!(persistence.state_not_saved());
        assert!(!persistence.snapshot_required);

        persistence.retry_not_before = None;
        persistence.compaction = Some(CompactionPhase::Commit);
        persistence.compaction_target = Some(EmbeddedPersistenceTarget::Routes);
        persistence.note_write_failure(InstantMillis(2_000), EmbeddedPersistenceFailure::Flash);
        assert_eq!(persistence.pending_routes.len(), 0);
        assert_eq!(persistence.retry_not_before, Some(InstantMillis(302_000)));
        assert!(persistence.state_not_saved());
        assert!(persistence.snapshot_required);
        assert_eq!(persistence.compaction, None);
    }

    #[test]
    fn legacy_timebase_allows_one_needed_compaction_then_adopts_the_budget_marker() {
        embassy_futures::block_on(async {
            let (mut journal, _) = {
                let flash = TestFlash::new();
                let mut scratch = [0u8; RECORD_SCRATCH_LEN];
                FlashJournal::open(flash, LAYOUT, &mut scratch, |_| {})
                    .await
                    .unwrap()
            };
            journal.initialize_empty().await.unwrap();
            journal
                .record_timebase(InstantMillis(10_000))
                .await
                .unwrap();
            let mut persistence =
                EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                    journal.release(),
                    LAYOUT,
                    EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
                        0,
                    )),
                    FixedRouteSnapshotKeys::new(),
                    (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
                );
            let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
            let report = persistence.restore(&mut engine, InstantMillis(0)).await;
            assert_eq!(persistence.next_compaction_not_before, None);
            persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, report.logical_start);
            persistence.route_dirty_since = Some(InstantMillis(report.logical_start.0 - 2_000));
            persistence
                .progress(&mut engine, report.logical_start)
                .await;
            persistence
                .progress(&mut engine, report.logical_start)
                .await;
            assert!(matches!(
                persistence.compaction,
                Some(CompactionPhase::Erase { sector: 0 })
            ));

            let flash = persistence.journal.take().unwrap().release();
            let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                flash,
                LAYOUT,
                EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
                FixedRouteSnapshotKeys::new(),
                (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
            );
            restored.restore(&mut engine, InstantMillis(0)).await;
            assert!(restored.next_compaction_not_before.is_some());
        });
    }

    #[test]
    fn restore_uses_the_later_of_flash_high_water_and_the_raw_clock() {
        embassy_futures::block_on(async {
            let recorded_at = InstantMillis(10_000);
            let flash_high_water = InstantMillis(recorded_at.0 + TIMEBASE_HEADROOM_MILLIS);
            let rtc_after_downtime = InstantMillis(flash_high_water.0 + 86_400_000);

            for raw_now in [InstantMillis(0), rtc_after_downtime] {
                let flash = TestFlash::new();
                let mut scratch = [0u8; RECORD_SCRATCH_LEN];
                let (mut journal, _) = FlashJournal::open(flash, LAYOUT, &mut scratch, |_| {})
                    .await
                    .unwrap();
                journal.initialize_empty().await.unwrap();
                journal.record_timebase(recorded_at).await.unwrap();

                let mut persistence =
                    EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                        journal.release(),
                        LAYOUT,
                        EmbeddedPersistencePolicy::hopspot_default(
                            EmbeddedCompactionPolicy::hopspot(0),
                        ),
                        FixedRouteSnapshotKeys::new(),
                        (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
                    );
                let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
                let report = persistence.restore(&mut engine, raw_now).await;

                assert_eq!(report.logical_start, raw_now.max(flash_high_water));
            }
        });
    }

    #[test]
    fn idle_persistence_advances_the_flash_timebase_on_schedule() {
        embassy_futures::block_on(async {
            let mut persistence =
                EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                    TestFlash::new(),
                    LAYOUT,
                    EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
                        0,
                    )),
                    FixedRouteSnapshotKeys::new(),
                    (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
                );
            let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
            let start = persistence.restore(&mut engine, InstantMillis(1_000)).await;

            assert_eq!(
                persistence.next_deadline(start.logical_start),
                Some(start.logical_start)
            );
            persistence.progress(&mut engine, start.logical_start).await;
            assert_eq!(persistence.last_timebase_success, Some(start.logical_start));

            let next = InstantMillis(
                start
                    .logical_start
                    .0
                    .saturating_add(TIMEBASE_RECORD_INTERVAL_MILLIS),
            );
            assert_eq!(persistence.next_deadline(start.logical_start), Some(next));
            persistence
                .progress(&mut engine, InstantMillis(next.0 - 1))
                .await;
            assert_eq!(persistence.last_timebase_success, Some(start.logical_start));
            persistence.progress(&mut engine, next).await;
            assert_eq!(persistence.last_timebase_success, Some(next));

            let mut flash = persistence.journal.take().unwrap().release();
            assert_eq!(
                FlashJournal::inspect_timebase(&mut flash, LAYOUT)
                    .await
                    .unwrap(),
                Some(InstantMillis(next.0 + TIMEBASE_HEADROOM_MILLIS))
            );
        });
    }

    #[test]
    fn failed_compaction_attempt_consumes_the_daily_budget() {
        let diagnostics = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&diagnostics);
        let mut persistence = ready_with_observer(move |diagnostic| {
            observed.borrow_mut().push(diagnostic);
        });
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let first = InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
        persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, first);
        persistence.route_dirty_since = Some(InstantMillis(first.0 - 2_000));
        embassy_futures::block_on(persistence.progress(&mut engine, first));
        let second = InstantMillis(first.0 + HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
        assert_eq!(persistence.next_compaction_not_before, Some(first));
        embassy_futures::block_on(persistence.progress(&mut engine, first));
        assert_eq!(persistence.next_compaction_not_before, Some(second));
        assert_eq!(
            persistence.compaction,
            Some(CompactionPhase::Erase { sector: 0 })
        );

        persistence.note_write_failure(first, EmbeddedPersistenceFailure::Flash);
        assert_eq!(persistence.compaction, None);
        assert!(persistence.snapshot_required);
        assert_eq!(
            persistence.next_deadline(first),
            Some(InstantMillis(first.0 + TIMEBASE_RECORD_INTERVAL_MILLIS))
        );
        embassy_futures::block_on(persistence.progress(&mut engine, InstantMillis(second.0 - 1)));
        assert_eq!(persistence.compaction, None);
        embassy_futures::block_on(persistence.progress(&mut engine, second));
        assert_eq!(
            persistence.compaction,
            Some(CompactionPhase::RecordBudget { at: second })
        );
        embassy_futures::block_on(persistence.progress(&mut engine, second));
        assert_eq!(
            persistence.compaction,
            Some(CompactionPhase::Erase { sector: 0 })
        );

        let starts = diagnostics
            .borrow()
            .iter()
            .filter(|diagnostic| {
                matches!(
                    diagnostic,
                    EmbeddedPersistenceDiagnostic::CompactionStarted { .. }
                )
            })
            .count();
        assert_eq!(starts, 2);
    }

    #[test]
    fn recorded_compaction_budget_survives_reboot() {
        embassy_futures::block_on(async {
            let mut persistence = ready();
            let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
            let attempt = InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
            persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, attempt);
            persistence.route_dirty_since = Some(InstantMillis(attempt.0 - 2_000));
            persistence.progress(&mut engine, attempt).await;
            persistence.progress(&mut engine, attempt).await;
            assert_eq!(
                persistence.compaction,
                Some(CompactionPhase::Erase { sector: 0 })
            );

            let flash = persistence.journal.take().unwrap().release();
            let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                flash,
                LAYOUT,
                EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
                FixedRouteSnapshotKeys::new(),
                (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
            );
            let report = restored.restore(&mut engine, InstantMillis(0)).await;
            assert!(report.logical_start.0 >= attempt.0);
            assert_eq!(
                restored.next_compaction_not_before,
                Some(InstantMillis(
                    attempt
                        .0
                        .saturating_add(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS)
                ))
            );
        });
    }

    #[test]
    fn marker_write_failure_does_not_consume_the_compaction_budget() {
        embassy_futures::block_on(async {
            let (flash, fail_next_write) = TestFlash::controlled();
            let mut scratch = [0u8; RECORD_SCRATCH_LEN];
            let (mut journal, _) = FlashJournal::open(flash, LAYOUT, &mut scratch, |_| {})
                .await
                .unwrap();
            journal.initialize_empty().await.unwrap();
            let mut persistence =
                EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                    TestFlash::new(),
                    LAYOUT,
                    EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
                        0,
                    )),
                    FixedRouteSnapshotKeys::new(),
                    (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
                );
            persistence.flash = None;
            persistence.journal = Some(journal);
            let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
            let attempt = InstantMillis(2_000);
            persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, attempt);
            persistence.route_dirty_since = Some(InstantMillis(0));
            persistence.progress(&mut engine, attempt).await;
            fail_next_write.set(true);
            persistence.progress(&mut engine, attempt).await;
            assert_eq!(persistence.next_compaction_not_before, None);
            assert_eq!(persistence.compaction, None);

            let flash = persistence.journal.take().unwrap().release();
            let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                flash,
                LAYOUT,
                EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
                FixedRouteSnapshotKeys::new(),
                (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
            );
            restored.restore(&mut engine, InstantMillis(0)).await;
            assert_eq!(restored.next_compaction_not_before, None);
        });
    }

    #[test]
    fn timebase_writes_and_repeated_reboots_do_not_move_the_compaction_deadline() {
        embassy_futures::block_on(async {
            let mut persistence = ready();
            let deadline = persistence.next_compaction_not_before;
            persistence
                .journal
                .as_mut()
                .unwrap()
                .record_timebase(InstantMillis(3 * 60 * 60 * 1_000))
                .await
                .unwrap();
            let flash = persistence.journal.take().unwrap().release();
            let mut engine = EngineState::<crate::storage::GrowableHeap>::default();

            let mut first = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                flash,
                LAYOUT,
                EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
                FixedRouteSnapshotKeys::new(),
                (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
            );
            first.restore(&mut engine, InstantMillis(0)).await;
            assert_eq!(first.next_compaction_not_before, deadline);

            let flash = first.journal.take().unwrap().release();
            let mut second = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                flash,
                LAYOUT,
                EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
                FixedRouteSnapshotKeys::new(),
                (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
            );
            second.restore(&mut engine, InstantMillis(0)).await;
            assert_eq!(second.next_compaction_not_before, deadline);
        });
    }

    #[test]
    fn overflow_during_compaction_commits_once_and_defers_the_next_snapshot() {
        let diagnostics = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&diagnostics);
        let mut persistence = ready_with_observer(move |diagnostic| {
            observed.borrow_mut().push(diagnostic);
        });
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let now = InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
        persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
        persistence.route_dirty_since = Some(InstantMillis(now.0 - 2_000));
        embassy_futures::block_on(persistence.progress(&mut engine, now));

        for byte in 0..6 {
            persistence.queue_route(
                PendingRouteDelta::RouteUpsert(DestinationHash::new(
                    [byte; TRUNCATED_HASH_BYTE_LEN],
                )),
                now,
            );
        }
        persistence.route_dirty_since = Some(now);
        for _ in 0..8 {
            embassy_futures::block_on(persistence.progress(&mut engine, now));
        }

        assert_eq!(persistence.compaction, None);
        assert!(persistence.snapshot_required);
        assert!(persistence.state_not_saved());
        assert_eq!(
            persistence.next_deadline(now),
            Some(InstantMillis(now.0 + TIMEBASE_RECORD_INTERVAL_MILLIS))
        );
        let diagnostics = diagnostics.borrow();
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| matches!(
                    diagnostic,
                    EmbeddedPersistenceDiagnostic::CompactionStarted { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| matches!(
                    diagnostic,
                    EmbeddedPersistenceDiagnostic::CompactionCompleted { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| matches!(
                    diagnostic,
                    EmbeddedPersistenceDiagnostic::DurabilityDeferred { .. }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn captured_route_keys_survive_slot_shifts_and_new_routes_land_after_compaction() {
        embassy_futures::block_on(async {
            let mut persistence = ready();
            let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
            let rows = [signed_route(0x31, &[0xA1]), signed_route(0x32, &[0xA2])];
            for row in &rows {
                assert_eq!(
                    engine.seed_route(row, InstantMillis(1_000)),
                    RouteSeedOutcome::Seeded
                );
            }
            let now = InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
            persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
            persistence.route_dirty_since = Some(InstantMillis(now.0 - 2_000));
            persistence.progress(&mut engine, now).await;
            persistence.progress(&mut engine, now).await;
            persistence.progress(&mut engine, now).await;

            let removed = rows[0].destination;
            let retained = rows[1].destination;
            let _ = engine.drop_route(&removed, AttachedInterfaces::new(&[]));
            let added = signed_route(0x34, &[0xA4]);
            assert_eq!(engine.seed_route(&added, now), RouteSeedOutcome::Seeded);
            persistence.queue_route(PendingRouteDelta::RouteUpsert(added.destination), now);
            persistence.route_dirty_since = Some(now);

            for _ in 0..8 {
                persistence.progress(&mut engine, now).await;
            }
            assert_eq!(persistence.compaction, None);
            assert_eq!(
                (
                    persistence.pending_routes.len(),
                    persistence.snapshot_required,
                    persistence.write_failed,
                    persistence.route_dirty_since,
                    persistence.landing_batch,
                ),
                (1, false, false, Some(now), None)
            );
            let correction_at = InstantMillis(now.0 + 2_000);
            persistence.progress(&mut engine, correction_at).await;
            persistence.progress(&mut engine, correction_at).await;

            let flash = persistence.journal.take().unwrap().release();
            let mut scratch = [0u8; RECORD_SCRATCH_LEN];
            let mut restored = Vec::new();
            let _ = FlashJournal::open(flash, LAYOUT, &mut scratch, |record| {
                if record.kind != FlashJournalRecordKind::RouteUpsert {
                    return;
                }
                let mut rows = read_routing_table_snapshot(record.payload).unwrap();
                restored.push(rows.next().unwrap().unwrap().destination);
            })
            .await
            .unwrap();
            assert_eq!(restored, vec![retained, added.destination]);
        });
    }

    #[test]
    fn sixteen_route_records_restore_eight_and_report_capacity_drops() {
        type EightRouteStorage =
            crate::storage::TestFixedStorage<8, 8, 256, 2, 2, 16, 4, 4, 4, 4, 4, 16>;
        let now = InstantMillis(1_000);
        let mut engine = EngineState::<EightRouteStorage>::default();
        let mut report = EmbeddedPersistenceRestoreReport {
            logical_start: now,
            route_seeded_count: 0,
            route_refused_count: 0,
            route_dropped_count: 0,
            ratchet_seeded_count: 0,
            ratchet_refused_count: 0,
            warning: None,
        };

        for secret in 1..=16 {
            let row = signed_route(secret, &[]);
            let required = routing_table_snapshot_len(core::iter::once(row.clone()));
            let mut scratch = [0u8; RECORD_SCRATCH_LEN];
            let written =
                write_routing_table_snapshot(core::iter::once(row), &mut scratch[..required])
                    .unwrap();
            apply_record(
                &mut engine,
                now,
                FlashJournalRecord {
                    epoch: 1,
                    kind: FlashJournalRecordKind::RouteUpsert,
                    payload: &scratch[..written],
                },
                &mut report,
            );
        }

        assert_eq!(
            report,
            EmbeddedPersistenceRestoreReport {
                logical_start: now,
                route_seeded_count: 8,
                route_refused_count: 0,
                route_dropped_count: 8,
                ratchet_seeded_count: 0,
                ratchet_refused_count: 0,
                warning: None,
            }
        );
    }

    #[test]
    fn thirty_days_of_pressure_erase_each_arena_sector_at_most_fifteen_times() {
        let diagnostics = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&diagnostics);
        let mut persistence = ready_with_observer(move |diagnostic| {
            observed.borrow_mut().push(diagnostic);
        });
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        embassy_futures::block_on(async {
            for day in 1..=30 {
                let now = InstantMillis(day * HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
                persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
                persistence.route_dirty_since = Some(InstantMillis(now.0 - 2_000));
                for _ in 0..8 {
                    persistence.progress(&mut engine, now).await;
                }
                assert_eq!(persistence.compaction, None);
            }
        });
        assert_eq!(
            diagnostics
                .borrow()
                .iter()
                .filter(|diagnostic| matches!(
                    diagnostic,
                    EmbeddedPersistenceDiagnostic::CompactionStarted { .. }
                ))
                .count(),
            30
        );
        let flash = persistence.journal.take().unwrap().release();
        assert_eq!(
            [
                flash.sector_erases[2] - 1,
                flash.sector_erases[3] - 1,
                flash.sector_erases[4],
                flash.sector_erases[5],
            ],
            [15, 15, 15, 15]
        );
    }
}
