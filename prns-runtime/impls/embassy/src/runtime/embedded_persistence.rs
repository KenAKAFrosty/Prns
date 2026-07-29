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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedPersistencePolicy {
    first_route_commit_delay_millis: u64,
    minimum_route_commit_interval_millis: u64,
    ratchet_batch_delay_millis: u64,
    retry_interval_millis: u64,
    timebase_record_interval_millis: u64,
}

impl EmbeddedPersistencePolicy {
    #[must_use]
    pub const fn new(
        first_route_commit_delay_millis: u64,
        minimum_route_commit_interval_millis: u64,
        ratchet_batch_delay_millis: u64,
        retry_interval_millis: u64,
        timebase_record_interval_millis: u64,
    ) -> Self {
        Self {
            first_route_commit_delay_millis,
            minimum_route_commit_interval_millis,
            ratchet_batch_delay_millis,
            retry_interval_millis,
            timebase_record_interval_millis,
        }
    }

    #[must_use]
    pub const fn hopspot_default() -> Self {
        Self::new(
            2_000,
            5 * 60 * 1_000,
            2_000,
            5 * 60 * 1_000,
            TIMEBASE_RECORD_INTERVAL_MILLIS,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedPersistenceFailure {
    Flash,
    Codec,
    Capacity,
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
    },
    WriteFailed {
        failure: EmbeddedPersistenceFailure,
        retry_at: InstantMillis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingDelta {
    RouteUpsert(DestinationHash),
    RouteRemoval(DestinationHash),
    SelfRatchet(DestinationHash),
}

impl PendingDelta {
    fn destination(self) -> DestinationHash {
        match self {
            Self::RouteUpsert(destination)
            | Self::RouteRemoval(destination)
            | Self::SelfRatchet(destination) => destination,
        }
    }

    fn is_route(self) -> bool {
        matches!(self, Self::RouteUpsert(_) | Self::RouteRemoval(_))
    }

    fn is_ratchet(self) -> bool {
        matches!(self, Self::SelfRatchet(_))
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

pub struct EmbeddedFlashPersistence<F, Observe, const PENDING: usize>
where
    F: NorFlash,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
{
    flash: Option<F>,
    journal: Option<FlashJournal<F>>,
    layout: FlashJournalLayout,
    policy: EmbeddedPersistencePolicy,
    observe_diagnostic: Observe,
    pending: HeaplessVec<PendingDelta, PENDING>,
    route_dirty_since: Option<InstantMillis>,
    ratchet_dirty_since: Option<InstantMillis>,
    last_route_success: Option<InstantMillis>,
    last_timebase_success: Option<InstantMillis>,
    retry_not_before: Option<InstantMillis>,
    landing_batch: Option<BatchKind>,
    landing_records: u32,
    compaction: Option<CompactionPhase>,
    overflowed: bool,
    write_failed: bool,
}

impl<F, Observe, const PENDING: usize> EmbeddedFlashPersistence<F, Observe, PENDING>
where
    F: NorFlash,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
{
    #[must_use]
    pub fn new(
        flash: F,
        layout: FlashJournalLayout,
        policy: EmbeddedPersistencePolicy,
        observe_diagnostic: Observe,
    ) -> Self {
        Self {
            flash: Some(flash),
            journal: None,
            layout,
            policy,
            observe_diagnostic,
            pending: HeaplessVec::new(),
            route_dirty_since: None,
            ratchet_dirty_since: None,
            last_route_success: None,
            last_timebase_success: None,
            retry_not_before: None,
            landing_batch: None,
            landing_records: 0,
            compaction: None,
            overflowed: false,
            write_failed: false,
        }
    }

    #[must_use]
    pub fn state_not_saved(&self) -> bool {
        self.write_failed
    }

    pub async fn restore<S: StorageLayout>(
        &mut self,
        engine: &mut EngineState<S>,
        raw_now: InstantMillis,
    ) -> EmbeddedPersistenceRestoreReport {
        let Some(mut flash) = self.flash.take() else {
            return self.empty_restore_report(raw_now, Some(FlashJournalWarning::Corrupt));
        };
        let logical_start = FlashJournal::inspect_timebase(&mut flash, self.layout)
            .await
            .ok()
            .flatten()
            .unwrap_or(raw_now);
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
        if restored.active_epoch.is_none() && journal.initialize_empty().await.is_err() {
            self.note_write_failure(raw_now, EmbeddedPersistenceFailure::Flash);
        }
        self.journal = Some(journal);
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::Restored(report));
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
                self.queue_route(PendingDelta::RouteUpsert(observation.destination));
                if self.route_dirty_since.is_none() {
                    self.route_dirty_since = Some(now);
                }
            }
            Journaled::RouteRemoved { destination, .. } => {
                self.queue_route(PendingDelta::RouteRemoval(*destination));
                if self.route_dirty_since.is_none() {
                    self.route_dirty_since = Some(now);
                }
            }
            Journaled::SelfRatchetRotated { destination } => {
                self.queue_ratchet(*destination);
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

    fn queue_route(&mut self, delta: PendingDelta) {
        if let Some(existing) = self
            .pending
            .iter_mut()
            .find(|pending| pending.is_route() && pending.destination() == delta.destination())
        {
            *existing = delta;
            return;
        }
        if self.pending.push(delta).is_err() {
            self.overflowed = true;
        }
    }

    fn queue_ratchet(&mut self, destination: DestinationHash) {
        if self
            .pending
            .iter()
            .any(|pending| pending.is_ratchet() && pending.destination() == destination)
        {
            return;
        }
        if self
            .pending
            .push(PendingDelta::SelfRatchet(destination))
            .is_err()
        {
            self.overflowed = true;
        }
    }

    fn next_deadline(&self, now: InstantMillis) -> Option<InstantMillis> {
        self.journal.as_ref()?;
        let mut deadline = if self.compaction.is_some() || self.landing_batch.is_some() {
            Some(now)
        } else {
            None
        };
        if let Some(dirty_since) = self.ratchet_dirty_since {
            deadline = earlier(
                deadline,
                Some(InstantMillis(
                    dirty_since
                        .0
                        .saturating_add(self.policy.ratchet_batch_delay_millis),
                )),
            );
        }
        if let Some(dirty_since) = self.route_dirty_since {
            let first_ready = dirty_since
                .0
                .saturating_add(self.policy.first_route_commit_delay_millis);
            let interval_ready = self.last_route_success.map_or(0, |last| {
                last.0
                    .saturating_add(self.policy.minimum_route_commit_interval_millis)
            });
            deadline = earlier(
                deadline,
                Some(InstantMillis(first_ready.max(interval_ready))),
            );
        }
        match (deadline, self.retry_not_before) {
            (Some(deadline), Some(retry)) => Some(InstantMillis(deadline.0.max(retry.0))),
            (deadline, None) => deadline,
            (None, Some(_)) => None,
        }
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
            if self.pending.iter().any(|pending| match batch {
                BatchKind::Routes => pending.is_route(),
                BatchKind::Ratchets => pending.is_ratchet(),
                BatchKind::Compaction => true,
            }) {
                self.landing_batch = None;
            } else {
                self.land_timebase(batch, now).await;
                return;
            }
        }
        let ratchet_due = self.ratchet_dirty_since.is_some_and(|dirty| {
            now.0
                >= dirty
                    .0
                    .saturating_add(self.policy.ratchet_batch_delay_millis)
        });
        let route_due = self.route_dirty_since.is_some_and(|dirty| {
            let first_ready = dirty
                .0
                .saturating_add(self.policy.first_route_commit_delay_millis);
            let interval_ready = self.last_route_success.map_or(0, |last| {
                last.0
                    .saturating_add(self.policy.minimum_route_commit_interval_millis)
            });
            now.0 >= first_ready.max(interval_ready)
        });
        if self.overflowed {
            if ratchet_due || route_due {
                self.start_compaction();
            }
            return;
        }
        let selected = if ratchet_due {
            self.pending.iter().position(|pending| pending.is_ratchet())
        } else if route_due {
            self.pending.iter().position(|pending| pending.is_route())
        } else {
            None
        };
        let Some(index) = selected else {
            return;
        };
        let delta = self.pending[index];
        let encoded = encode_delta(engine, delta);
        let Ok(payload) = encoded else {
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
                self.pending.swap_remove(index);
                self.landing_records = self.landing_records.saturating_add(1);
                let batch = if delta.is_ratchet() {
                    BatchKind::Ratchets
                } else {
                    BatchKind::Routes
                };
                let more = self.pending.iter().any(|pending| {
                    if batch == BatchKind::Ratchets {
                        pending.is_ratchet()
                    } else {
                        pending.is_route()
                    }
                });
                if !more {
                    self.landing_batch = Some(batch);
                }
            }
            Err(FlashJournalError::ArenaFull) => self.start_compaction(),
            Err(error) => {
                let failure = failure_from_journal(error);
                self.note_write_failure(now, failure);
            }
        }
    }

    fn start_compaction(&mut self) {
        self.pending.clear();
        self.overflowed = false;
        self.compaction = Some(CompactionPhase::Erase { sector: 0 });
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
                let row = engine.persisted_route_rows().nth(index);
                let Some(row) = row else {
                    self.compaction = Some(CompactionPhase::Ratchets { index: 0 });
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
                let row = engine.persisted_self_ratchet_rows().nth(index);
                let Some((destination, last_rotated, secrets)) = row else {
                    self.compaction = Some(CompactionPhase::Commit);
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
                    if self.overflowed {
                        self.start_compaction();
                    } else if self.pending.is_empty() {
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
        let should_record = self.last_timebase_success.is_none_or(|last| {
            now.0.saturating_sub(last.0) >= self.policy.timebase_record_interval_millis
        });
        if should_record {
            let Some(journal) = self.journal.as_mut() else {
                return;
            };
            if let Err(error) = journal.record_timebase(now).await {
                self.note_write_failure(now, failure_from_journal(error));
                return;
            }
            self.last_timebase_success = Some(now);
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
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::BatchPersisted {
            records,
            at: now,
        });
    }

    fn note_codec_failure(&mut self, now: InstantMillis) {
        self.note_write_failure(now, EmbeddedPersistenceFailure::Codec);
    }

    fn note_write_failure(&mut self, now: InstantMillis, failure: EmbeddedPersistenceFailure) {
        let retry_at = InstantMillis(now.0.saturating_add(self.policy.retry_interval_millis));
        self.retry_not_before = Some(retry_at);
        if failure == EmbeddedPersistenceFailure::Flash {
            self.write_failed = true;
            if let Some(journal) = self.journal.as_mut() {
                journal.abort_compaction();
            }
            self.compaction = None;
            self.overflowed = true;
        }
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::WriteFailed { failure, retry_at });
    }
}

pub(crate) trait ManifoldPersistence<S: StorageLayout> {
    fn observe(&mut self, journaled: &Journaled<'_>, now: InstantMillis);
    fn deadline(&self, now: InstantMillis) -> Option<InstantMillis>;
    async fn progress(&mut self, engine: &mut EngineState<S>, now: InstantMillis);
}

impl<S, F, Observe, const PENDING: usize> ManifoldPersistence<S>
    for EmbeddedFlashPersistence<F, Observe, PENDING>
where
    S: StorageLayout,
    F: NorFlash,
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

fn encode_delta<S: StorageLayout>(
    engine: &EngineState<S>,
    delta: PendingDelta,
) -> Result<EncodedDelta, ()> {
    match delta {
        PendingDelta::RouteUpsert(destination) => {
            let Some(row) = engine
                .persisted_route_rows()
                .find(|row| row.destination == destination)
            else {
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
        PendingDelta::RouteRemoval(destination) => encode_tombstone(destination),
        PendingDelta::SelfRatchet(destination) => {
            let Some((last_rotated, secrets)) = engine.persisted_self_ratchet_row(&destination)
            else {
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
    }
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
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind};
    use embedded_storage_async::nor_flash::ReadNorFlash;

    const ERASE: usize = 256;
    const CAPACITY: usize = ERASE * 6;
    const LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
        [0, ERASE as u32],
        [
            crate::persistence::FlashArenaRange::new((ERASE * 2) as u32, (ERASE * 4) as u32),
            crate::persistence::FlashArenaRange::new((ERASE * 4) as u32, (ERASE * 6) as u32),
        ],
    );

    #[derive(Debug)]
    struct TestFlash([u8; CAPACITY]);

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
            bytes.copy_from_slice(&self.0[start..end]);
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
            let start = offset as usize;
            for (stored, written) in self.0[start..start + bytes.len()].iter_mut().zip(bytes) {
                *stored &= *written;
            }
            Ok(())
        }

        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            self.0[from as usize..to as usize].fill(0xFF);
            Ok(())
        }
    }

    fn ready() -> EmbeddedFlashPersistence<TestFlash, fn(EmbeddedPersistenceDiagnostic), 4> {
        embassy_futures::block_on(async {
            let flash = TestFlash([0xFF; CAPACITY]);
            let mut scratch = [0u8; RECORD_SCRATCH_LEN];
            let (mut journal, _) = FlashJournal::open(flash, LAYOUT, &mut scratch, |_| {})
                .await
                .unwrap();
            journal.initialize_empty().await.unwrap();
            let mut persistence = EmbeddedFlashPersistence::new(
                TestFlash([0xFF; CAPACITY]),
                LAYOUT,
                EmbeddedPersistencePolicy::hopspot_default(),
                (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
            );
            persistence.flash = None;
            persistence.journal = Some(journal);
            persistence
        })
    }

    #[test]
    fn exact_route_and_ratchet_deadlines_are_distinct() {
        let policy = EmbeddedPersistencePolicy::hopspot_default();
        assert_eq!(policy.first_route_commit_delay_millis, 2_000);
        assert_eq!(policy.minimum_route_commit_interval_millis, 300_000);
        assert_eq!(policy.ratchet_batch_delay_millis, 2_000);
        assert_eq!(policy.retry_interval_millis, 300_000);
        assert_eq!(
            policy.timebase_record_interval_millis,
            TIMEBASE_RECORD_INTERVAL_MILLIS
        );
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
        persistence.queue_route(PendingDelta::RouteUpsert(destination));
        persistence.queue_route(PendingDelta::RouteUpsert(destination));
        persistence.queue_route(PendingDelta::RouteRemoval(destination));
        persistence.queue_ratchet(destination);
        persistence.queue_ratchet(destination);
        assert_eq!(
            persistence.pending.as_slice(),
            &[
                PendingDelta::RouteRemoval(destination),
                PendingDelta::SelfRatchet(destination),
            ]
        );
    }

    #[test]
    fn pending_overflow_waits_for_the_batch_deadline_before_compacting() {
        let mut persistence = ready();
        for byte in 0..5 {
            persistence.queue_route(PendingDelta::RouteUpsert(DestinationHash::new(
                [byte; TRUNCATED_HASH_BYTE_LEN],
            )));
        }
        persistence.route_dirty_since = Some(InstantMillis(1_000));
        assert!(persistence.overflowed);
        assert_eq!(
            persistence.next_deadline(InstantMillis(1_500)),
            Some(InstantMillis(3_000))
        );

        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        embassy_futures::block_on(persistence.progress(&mut engine, InstantMillis(2_999)));
        assert_eq!(persistence.compaction, None);
        assert!(persistence.overflowed);

        embassy_futures::block_on(persistence.progress(&mut engine, InstantMillis(3_000)));
        assert_eq!(
            persistence.compaction,
            Some(CompactionPhase::Erase { sector: 0 })
        );
    }

    #[test]
    fn failures_keep_dirty_state_and_only_flash_failures_raise_the_notice() {
        let mut persistence = ready();
        let destination = DestinationHash::new([0x22; TRUNCATED_HASH_BYTE_LEN]);
        persistence.queue_route(PendingDelta::RouteUpsert(destination));
        persistence.note_codec_failure(InstantMillis(1_000));
        assert_eq!(persistence.pending.len(), 1);
        assert_eq!(persistence.retry_not_before, Some(InstantMillis(301_000)));
        assert!(!persistence.state_not_saved());
        assert!(!persistence.overflowed);

        persistence.retry_not_before = None;
        persistence.compaction = Some(CompactionPhase::Commit);
        persistence.note_write_failure(InstantMillis(2_000), EmbeddedPersistenceFailure::Flash);
        assert_eq!(persistence.pending.len(), 1);
        assert_eq!(persistence.retry_not_before, Some(InstantMillis(302_000)));
        assert!(persistence.state_not_saved());
        assert!(persistence.overflowed);
        assert_eq!(persistence.compaction, None);
    }
}
