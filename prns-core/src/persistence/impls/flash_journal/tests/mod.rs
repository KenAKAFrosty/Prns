use super::*;
use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind};
use embedded_storage_async::nor_flash::ReadNorFlash;
use std::vec::Vec;

const ERASE: usize = 256;
const CAPACITY: usize = ERASE * 6;
const LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0, ERASE as u32],
    [
        FlashArenaRange::new((ERASE * 2) as u32, (ERASE * 4) as u32),
        FlashArenaRange::new((ERASE * 4) as u32, (ERASE * 6) as u32),
    ],
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FakeError {
    Interrupted,
    OutOfBounds,
    Misaligned,
}

impl NorFlashError for FakeError {
    fn kind(&self) -> NorFlashErrorKind {
        NorFlashErrorKind::Other
    }
}

struct FakeFlash {
    bytes: [u8; CAPACITY],
    operation: usize,
    fail_at: Option<usize>,
    corrupt_at: Option<usize>,
}

impl FakeFlash {
    fn new() -> Self {
        Self {
            bytes: [0xFF; CAPACITY],
            operation: 0,
            fail_at: None,
            corrupt_at: None,
        }
    }

    fn interrupt(&mut self) -> Result<(), FakeError> {
        self.operation += 1;
        if self.fail_at == Some(self.operation) {
            return Err(FakeError::Interrupted);
        }
        Ok(())
    }
}

impl ErrorType for FakeFlash {
    type Error = FakeError;
}

impl ReadNorFlash for FakeFlash {
    const READ_SIZE: usize = 4;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let start = offset as usize;
        let end = start.saturating_add(bytes.len());
        if !start.is_multiple_of(Self::READ_SIZE)
            || !bytes.len().is_multiple_of(Self::READ_SIZE)
            || end > CAPACITY
        {
            return Err(FakeError::OutOfBounds);
        }
        bytes.copy_from_slice(&self.bytes[start..end]);
        Ok(())
    }

    fn capacity(&self) -> usize {
        CAPACITY
    }
}

impl NorFlash for FakeFlash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = ERASE;

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let start = offset as usize;
        let end = start.saturating_add(bytes.len());
        if !start.is_multiple_of(Self::WRITE_SIZE)
            || !bytes.len().is_multiple_of(Self::WRITE_SIZE)
            || end > CAPACITY
        {
            return Err(FakeError::Misaligned);
        }
        self.interrupt()?;
        for (slot, byte) in self.bytes[start..end].iter_mut().zip(bytes) {
            *slot &= *byte;
        }
        if self.corrupt_at == Some(self.operation) {
            if let Some((slot, intended)) = self.bytes[start..end]
                .iter_mut()
                .zip(bytes)
                .find(|(_, intended)| **intended != 0xFF)
            {
                *slot |= 1 << (!*intended).trailing_zeros();
            }
        }
        Ok(())
    }

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        let start = from as usize;
        let end = to as usize;
        if !start.is_multiple_of(Self::ERASE_SIZE)
            || !end.is_multiple_of(Self::ERASE_SIZE)
            || start >= end
            || end > CAPACITY
        {
            return Err(FakeError::Misaligned);
        }
        self.interrupt()?;
        self.bytes[start..end].fill(0xFF);
        Ok(())
    }
}

async fn open(
    flash: FakeFlash,
) -> (
    FlashJournal<FakeFlash>,
    FlashJournalRestoreReport,
    Vec<(FlashJournalRecordKind, Vec<u8>)>,
) {
    let mut scratch = [0u8; IO_CHUNK_LEN];
    let mut records = Vec::new();
    let (journal, report) = FlashJournal::open(flash, LAYOUT, &mut scratch, |record| {
        records.push((record.kind, record.payload.to_vec()));
    })
    .await
    .unwrap();
    (journal, report, records)
}

mod commit_uncertainty;

#[test]
fn committed_records_restore_in_order() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        journal.initialize_empty().await.unwrap();
        journal
            .append(FlashJournalRecordKind::RouteUpsert, b"first")
            .await
            .unwrap();
        journal
            .append(FlashJournalRecordKind::RouteRemoval, b"second")
            .await
            .unwrap();
        journal
            .append(
                FlashJournalRecordKind::RemoteControlControllerGrants,
                b"third",
            )
            .await
            .unwrap();
        journal
            .append(
                FlashJournalRecordKind::RemoteControlTargetAccesses,
                b"fourth",
            )
            .await
            .unwrap();
        let (_, report, records) = open(journal.release()).await;
        assert_eq!(
            report,
            FlashJournalRestoreReport {
                active_epoch: Some(0),
                restored_records: 4,
                warning: None,
            }
        );
        assert_eq!(
            records,
            vec![
                (FlashJournalRecordKind::RouteUpsert, b"first".to_vec()),
                (FlashJournalRecordKind::RouteRemoval, b"second".to_vec()),
                (
                    FlashJournalRecordKind::RemoteControlControllerGrants,
                    b"third".to_vec(),
                ),
                (
                    FlashJournalRecordKind::RemoteControlTargetAccesses,
                    b"fourth".to_vec(),
                ),
            ]
        );
    });
}

#[test]
fn every_interrupted_initialization_stays_uninitialized() {
    embassy_futures::block_on(async {
        for failed_operation in 1..=3 {
            let flash = FakeFlash {
                bytes: [0xFF; CAPACITY],
                operation: 0,
                fail_at: Some(failed_operation),
                corrupt_at: None,
            };
            let (mut journal, _, _) = open(flash).await;
            assert_eq!(
                journal.initialize_empty().await,
                Err(FlashJournalError::Flash(FakeError::Interrupted))
            );
            let (_, report, records) = open(journal.release()).await;
            assert_eq!(report.active_epoch, None);
            assert!(records.is_empty());
        }
    });
}

#[test]
fn every_interrupted_append_restores_the_prior_complete_state() {
    embassy_futures::block_on(async {
        let (mut baseline, _, _) = open(FakeFlash::new()).await;
        baseline.initialize_empty().await.unwrap();
        baseline
            .append(FlashJournalRecordKind::RouteUpsert, b"prior")
            .await
            .unwrap();
        let baseline = baseline.release();
        for failed_operation in 1..=4 {
            let mut flash = FakeFlash {
                bytes: baseline.bytes,
                operation: 0,
                fail_at: Some(failed_operation),
                corrupt_at: None,
            };
            let (mut journal, _, _) = open(flash).await;
            let result = journal
                .append(FlashJournalRecordKind::RouteUpsert, &[0xA5; 300])
                .await;
            flash = journal.release();
            assert_eq!(
                result,
                Err(FlashJournalError::Flash(FakeError::Interrupted))
            );
            let (_, report, records) = open(flash).await;
            assert_eq!(report.warning, None);
            assert_eq!(
                records,
                vec![(FlashJournalRecordKind::RouteUpsert, b"prior".to_vec())]
            );
        }
    });
}

#[test]
fn every_silently_corrupted_append_write_is_refused_before_success() {
    embassy_futures::block_on(async {
        let (mut baseline, _, _) = open(FakeFlash::new()).await;
        baseline.initialize_empty().await.unwrap();
        baseline
            .append(FlashJournalRecordKind::RouteUpsert, b"prior")
            .await
            .unwrap();
        let baseline = baseline.release();

        // Header, payload, and commit are three distinct programming operations. A driver may
        // report success even when a cell did not program, so each boundary must be caught by
        // read-back verification and remain unavailable to replay.
        for corrupted_operation in 1..=3 {
            let flash = FakeFlash {
                bytes: baseline.bytes,
                operation: 0,
                fail_at: None,
                corrupt_at: Some(corrupted_operation),
            };
            let (mut journal, _, _) = open(flash).await;
            assert_eq!(
                journal
                    .append(FlashJournalRecordKind::RouteUpsert, b"candidate")
                    .await,
                Err(FlashJournalError::VerificationFailed)
            );
            let (_, report, records) = open(journal.release()).await;
            assert_eq!(report.warning, None);
            assert_eq!(
                records,
                vec![(FlashJournalRecordKind::RouteUpsert, b"prior".to_vec())]
            );
        }
    });
}

#[test]
fn every_interrupted_compaction_step_keeps_the_previous_arena() {
    embassy_futures::block_on(async {
        let (mut baseline, _, _) = open(FakeFlash::new()).await;
        baseline.initialize_empty().await.unwrap();
        baseline
            .append(FlashJournalRecordKind::RouteUpsert, b"prior")
            .await
            .unwrap();
        let baseline = baseline.release();
        for failed_operation in 1..=7 {
            let flash = FakeFlash {
                bytes: baseline.bytes,
                operation: 0,
                fail_at: Some(failed_operation),
                corrupt_at: None,
            };
            let (mut journal, _, _) = open(flash).await;
            let mut interrupted = false;
            for sector in 0..journal.inactive_sector_count() {
                if journal.erase_inactive_sector(sector).await.is_err() {
                    interrupted = true;
                    break;
                }
            }
            if !interrupted {
                journal.begin_compaction().unwrap();
                if journal
                    .append_compacted(FlashJournalRecordKind::RouteUpsert, b"new")
                    .await
                    .is_err()
                {
                    interrupted = true;
                }
            }
            if !interrupted && journal.commit_compaction().await.is_err() {
                interrupted = true;
            }
            assert!(interrupted);
            let (_, report, records) = open(journal.release()).await;
            assert_eq!(report.active_epoch, Some(0));
            assert_eq!(
                records,
                vec![(FlashJournalRecordKind::RouteUpsert, b"prior".to_vec())]
            );
        }
    });
}

#[test]
fn committed_compaction_selects_the_new_epoch() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        journal.initialize_empty().await.unwrap();
        journal
            .append(FlashJournalRecordKind::RouteUpsert, b"old")
            .await
            .unwrap();
        for sector in 0..journal.inactive_sector_count() {
            journal.erase_inactive_sector(sector).await.unwrap();
        }
        journal.begin_compaction().unwrap();
        journal
            .append_compacted(FlashJournalRecordKind::RouteUpsert, b"live")
            .await
            .unwrap();
        journal.commit_compaction().await.unwrap();
        let (_, report, records) = open(journal.release()).await;
        assert_eq!(report.active_epoch, Some(1));
        assert_eq!(
            records,
            vec![(FlashJournalRecordKind::RouteUpsert, b"live".to_vec())]
        );
    });
}

#[test]
fn reclaiming_the_old_arena_cannot_damage_the_new_epoch() {
    embassy_futures::block_on(async {
        let (mut baseline, _, _) = open(FakeFlash::new()).await;
        baseline.initialize_empty().await.unwrap();
        baseline
            .append(FlashJournalRecordKind::RouteUpsert, b"old")
            .await
            .unwrap();
        let baseline = baseline.release();
        for failed_operation in 8..=9 {
            let flash = FakeFlash {
                bytes: baseline.bytes,
                operation: 0,
                fail_at: Some(failed_operation),
                corrupt_at: None,
            };
            let (mut journal, _, _) = open(flash).await;
            for sector in 0..journal.inactive_sector_count() {
                journal.erase_inactive_sector(sector).await.unwrap();
            }
            journal.begin_compaction().unwrap();
            journal
                .append_compacted(FlashJournalRecordKind::RouteUpsert, b"new")
                .await
                .unwrap();
            journal.commit_compaction().await.unwrap();
            let mut interrupted = false;
            for sector in 0..journal.inactive_sector_count() {
                if journal.erase_inactive_sector(sector).await.is_err() {
                    interrupted = true;
                    break;
                }
            }
            assert!(interrupted);
            let (_, report, records) = open(journal.release()).await;
            assert_eq!(report.active_epoch, Some(1));
            assert_eq!(
                records,
                vec![(FlashJournalRecordKind::RouteUpsert, b"new".to_vec())]
            );
        }
    });
}

#[test]
fn arena_exhaustion_preserves_every_complete_record() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        journal.initialize_empty().await.unwrap();
        let mut written = 0usize;
        loop {
            match journal
                .append(FlashJournalRecordKind::RouteUpsert, &[written as u8; 96])
                .await
            {
                Ok(()) => written += 1,
                Err(FlashJournalError::ArenaFull) => break,
                Err(error) => panic!("unexpected append failure: {error:?}"),
            }
        }
        let (_, report, records) = open(journal.release()).await;
        assert_eq!(report.restored_records as usize, written);
        assert_eq!(records.len(), written);
    });
}

#[test]
fn active_capacity_accounts_for_the_complete_record_and_reserve() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        journal.initialize_empty().await.unwrap();
        assert_eq!(journal.active_remaining_bytes(), Some(480));
        assert!(journal.active_can_fit(4, 444));
        assert!(!journal.active_can_fit(4, 445));
        journal
            .append(FlashJournalRecordKind::RouteRemoval, &[0; 4])
            .await
            .unwrap();
        assert_eq!(journal.active_remaining_bytes(), Some(444));
    });
}

#[test]
fn corruption_and_unknown_schema_warn_without_blocking_open() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        journal.initialize_empty().await.unwrap();
        journal
            .append(FlashJournalRecordKind::RouteUpsert, b"route")
            .await
            .unwrap();
        let mut corrupt = journal.release();
        corrupt.bytes[ERASE * 2 + HEADER_LEN + HEADER_LEN] ^= 1;
        let (_, report, records) = open(corrupt).await;
        assert_eq!(report.warning, Some(FlashJournalWarning::Corrupt));
        assert!(records.is_empty());

        let (mut journal, _, _) = open(FakeFlash::new()).await;
        journal.initialize_empty().await.unwrap();
        let mut future = journal.release();
        future.bytes[ERASE * 2 + 4..ERASE * 2 + 6]
            .copy_from_slice(&(SCHEMA_VERSION + 1).to_le_bytes());
        let prefix = &future.bytes[ERASE * 2..ERASE * 2 + CHECKSUM_PREFIX_LEN];
        let checksum = record_checksum(prefix, &[]);
        future.bytes[ERASE * 2 + 20..ERASE * 2 + 24].copy_from_slice(&checksum.to_le_bytes());
        let (_, report, records) = open(future).await;
        assert_eq!(
            report.warning,
            Some(FlashJournalWarning::UnknownSchema {
                found: SCHEMA_VERSION + 1,
            })
        );
        assert!(records.is_empty());
    });
}

#[test]
fn epoch_selection_handles_rollover() {
    let first = ArenaState {
        epoch: Some(u64::MAX),
        append_at: 0,
        warning: None,
    };
    let second = ArenaState {
        epoch: Some(0),
        append_at: 0,
        warning: None,
    };
    assert_eq!(select_active(&first, &second), Some(1));
}

#[test]
fn timebase_uses_the_existing_sealed_format_and_headroom() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        assert_eq!(journal.timebase_high_water().await.unwrap(), None);
        journal.record_timebase(InstantMillis(5_000)).await.unwrap();
        assert_eq!(
            journal.timebase_high_water().await.unwrap(),
            Some(InstantMillis(5_000 + TIMEBASE_HEADROOM_MILLIS))
        );
    });
}

#[test]
fn legacy_timebase_slots_decode_without_a_compaction_budget() {
    embassy_futures::block_on(async {
        let mut flash = FakeFlash::new();
        let mut slot = [0xFF; TIMEBASE_SLOT_LEN];
        write_timebase_snapshot(InstantMillis(42_000), &mut slot[..TIMEBASE_SNAPSHOT_LEN]).unwrap();
        flash.bytes[..TIMEBASE_SNAPSHOT_LEN].copy_from_slice(&slot[..TIMEBASE_SNAPSHOT_LEN]);

        assert_eq!(
            FlashJournal::inspect_timebase_state(&mut flash, LAYOUT)
                .await
                .unwrap(),
            FlashJournalTimebaseState {
                high_water: Some(InstantMillis(42_000)),
                last_compaction_attempt: None,
            }
        );
    });
}

#[test]
fn compaction_budget_markers_round_trip_and_round_up_to_minutes() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        assert_eq!(
            journal
                .record_compaction_budget(InstantMillis(120_001))
                .await
                .unwrap(),
            InstantMillis(180_000)
        );
        let mut flash = journal.release();
        assert_eq!(
            FlashJournal::inspect_timebase_state(&mut flash, LAYOUT)
                .await
                .unwrap(),
            FlashJournalTimebaseState {
                high_water: Some(InstantMillis(120_001 + TIMEBASE_HEADROOM_MILLIS)),
                last_compaction_attempt: Some(InstantMillis(180_000)),
            }
        );
    });
}

#[test]
fn corrupt_and_partial_compaction_budget_markers_are_ignored() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        journal
            .record_compaction_budget(InstantMillis(120_001))
            .await
            .unwrap();
        let mut corrupt = journal.release();
        corrupt.bytes[COMPACTION_BUDGET_OFFSET + 4] ^= 1;
        assert_eq!(
            FlashJournal::inspect_timebase_state(&mut corrupt, LAYOUT)
                .await
                .unwrap()
                .last_compaction_attempt,
            None
        );

        let mut partial = FakeFlash::new();
        let mut slot = [0xFF; TIMEBASE_SLOT_LEN];
        write_timebase_snapshot(InstantMillis(42_000), &mut slot[..TIMEBASE_SNAPSHOT_LEN]).unwrap();
        slot[COMPACTION_BUDGET_OFFSET..COMPACTION_BUDGET_OFFSET + 4]
            .copy_from_slice(&3u32.to_le_bytes());
        partial.bytes[..TIMEBASE_SLOT_LEN].copy_from_slice(&slot);
        assert_eq!(
            FlashJournal::inspect_timebase_state(&mut partial, LAYOUT)
                .await
                .unwrap()
                .last_compaction_attempt,
            None
        );
    });
}

#[test]
fn compaction_budget_rounding_never_shortens_the_interval_floor() {
    for attempted_at in [0, 1, 59_999, 60_000, 60_001, 3_600_001] {
        let rounded = round_compaction_attempt_up::<FakeError>(InstantMillis(attempted_at))
            .unwrap()
            .0;
        assert!(rounded >= attempted_at);
        assert!(rounded.saturating_sub(attempted_at) < COMPACTION_BUDGET_MINUTE_MILLIS);
    }
}

#[test]
fn ordinary_timebase_rotation_preserves_the_compaction_budget_without_advancing_it() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        let attempt = journal
            .record_compaction_budget(InstantMillis(1))
            .await
            .unwrap();
        let step = TIMEBASE_HEADROOM_MILLIS + 1;
        for index in 1..=20 {
            journal
                .record_timebase(InstantMillis(index * step))
                .await
                .unwrap();
        }
        let mut flash = journal.release();
        assert_eq!(
            FlashJournal::inspect_timebase_state(&mut flash, LAYOUT)
                .await
                .unwrap()
                .last_compaction_attempt,
            Some(attempt)
        );
    });
}

#[test]
fn interrupted_timebase_rotation_keeps_the_previous_high_water() {
    embassy_futures::block_on(async {
        let (mut journal, _, _) = open(FakeFlash::new()).await;
        let step = TIMEBASE_HEADROOM_MILLIS + 1;
        for index in 0..16 {
            journal
                .record_timebase(InstantMillis(index * step))
                .await
                .unwrap();
        }
        let previous = InstantMillis(15 * step + TIMEBASE_HEADROOM_MILLIS);
        let baseline = journal.release();
        for failed_operation in 1..=2 {
            let flash = FakeFlash {
                bytes: baseline.bytes,
                operation: 0,
                fail_at: Some(failed_operation),
                corrupt_at: None,
            };
            let (mut journal, _, _) = open(flash).await;
            assert_eq!(
                journal.record_timebase(InstantMillis(16 * step)).await,
                Err(FlashJournalError::Flash(FakeError::Interrupted))
            );
            let mut flash = journal.release();
            assert_eq!(
                FlashJournal::inspect_timebase(&mut flash, LAYOUT)
                    .await
                    .unwrap(),
                Some(previous)
            );
        }
    });
}
