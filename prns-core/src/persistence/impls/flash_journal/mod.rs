use embedded_storage_async::nor_flash::NorFlash;
use zeroize::Zeroize;

use crate::persistence::envelope::crc32;
use crate::persistence::{read_timebase_snapshot, write_timebase_snapshot, TIMEBASE_SNAPSHOT_LEN};
use crate::units::InstantMillis;

use super::flash::TIMEBASE_HEADROOM_MILLIS;

const MAGIC: [u8; 4] = *b"PRNJ";
const SCHEMA_VERSION: u16 = 1;
const COMMIT_WORD: u32 = 0x5449_4D43;
const HEADER_LEN: usize = 32;
const CHECKSUM_PREFIX_LEN: usize = 20;
const COMMIT_OFFSET: usize = 28;
const IO_CHUNK_LEN: usize = 256;
const TIMEBASE_SLOT_LEN: usize = 32;
const COMPACTION_BUDGET_OFFSET: usize = 24;
const COMPACTION_BUDGET_LEN: usize = TIMEBASE_SLOT_LEN - COMPACTION_BUDGET_OFFSET;
const COMPACTION_BUDGET_MINUTE_MILLIS: u64 = 60_000;
const COMPACTION_BUDGET_DOMAIN: [u8; 4] = *b"PCB1";
const _: () = assert!(COMPACTION_BUDGET_LEN == 8);
const _: () = assert!(TIMEBASE_SNAPSHOT_LEN <= COMPACTION_BUDGET_OFFSET);

pub const FLASH_JOURNAL_RECORD_OVERHEAD: usize = HEADER_LEN;

#[must_use]
pub const fn flash_journal_record_storage_len(payload_len: usize, alignment: usize) -> usize {
    let remainder = payload_len % alignment;
    let padded = if remainder == 0 {
        payload_len
    } else {
        payload_len + alignment - remainder
    };
    FLASH_JOURNAL_RECORD_OVERHEAD + padded
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashArenaRange {
    pub start: u32,
    pub end: u32,
}

impl FlashArenaRange {
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashJournalLayout {
    pub timebase_regions: [u32; 2],
    pub arenas: [FlashArenaRange; 2],
}

impl FlashJournalLayout {
    #[must_use]
    pub const fn new(timebase_regions: [u32; 2], arenas: [FlashArenaRange; 2]) -> Self {
        Self {
            timebase_regions,
            arenas,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FlashJournalRecordKind {
    ArenaCommit = 1,
    RouteUpsert = 2,
    RouteRemoval = 3,
    SelfRatchet = 4,
    RemoteControlControllerGrants = 5,
    RemoteControlTargetAccesses = 6,
    DiscoveryGroupConfigurations = 7,
    NodeName = 8,
}

impl FlashJournalRecordKind {
    fn from_wire(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::ArenaCommit),
            2 => Some(Self::RouteUpsert),
            3 => Some(Self::RouteRemoval),
            4 => Some(Self::SelfRatchet),
            5 => Some(Self::RemoteControlControllerGrants),
            6 => Some(Self::RemoteControlTargetAccesses),
            7 => Some(Self::DiscoveryGroupConfigurations),
            8 => Some(Self::NodeName),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashJournalWarning {
    UnknownSchema { found: u16 },
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashJournalRestoreReport {
    pub active_epoch: Option<u64>,
    pub restored_records: u32,
    pub warning: Option<FlashJournalWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashJournalTimebaseState {
    pub high_water: Option<InstantMillis>,
    pub last_compaction_attempt: Option<InstantMillis>,
}

pub struct FlashJournalRecord<'a> {
    pub epoch: u64,
    pub kind: FlashJournalRecordKind,
    pub payload: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub enum FlashJournalError<E> {
    Flash(E),
    CommitUnconfirmed { at: u32, error: E },
    Misaligned,
    OutOfBounds,
    ArenaFull,
    Uninitialized,
    CompactionInProgress,
    NoCompaction,
    PayloadTooLarge,
    ScratchTooShort,
    VerificationFailed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FlashJournalCommitResolution {
    Committed,
    NotCommitted,
}

struct ArenaState {
    epoch: Option<u64>,
    append_at: u32,
    warning: Option<FlashJournalWarning>,
}

#[derive(Clone, Copy)]
struct ArenaCursor {
    index: usize,
    epoch: u64,
    append_at: u32,
}

struct RecordHeader {
    schema: u16,
    kind: u16,
    epoch: u64,
    payload_len: usize,
    checksum: u32,
    committed: bool,
}

struct TimebaseRegionState {
    append_at: Option<usize>,
    newest: Option<u64>,
    last_compaction_attempt: Option<u64>,
    any_used: bool,
}

pub struct FlashJournal<F: NorFlash> {
    flash: F,
    layout: FlashJournalLayout,
    active: Option<ArenaCursor>,
    compaction: Option<ArenaCursor>,
}

impl<F: NorFlash> FlashJournal<F> {
    pub async fn inspect_timebase(
        flash: &mut F,
        layout: FlashJournalLayout,
    ) -> Result<Option<InstantMillis>, FlashJournalError<F::Error>> {
        Ok(Self::inspect_timebase_state(flash, layout)
            .await?
            .high_water)
    }

    pub async fn inspect_timebase_state(
        flash: &mut F,
        layout: FlashJournalLayout,
    ) -> Result<FlashJournalTimebaseState, FlashJournalError<F::Error>> {
        let mut state = FlashJournalTimebaseState {
            high_water: None,
            last_compaction_attempt: None,
        };
        for region in layout.timebase_regions {
            if !(region as usize).is_multiple_of(F::ERASE_SIZE) {
                return Err(FlashJournalError::Misaligned);
            }
            let Some(end) = (region as usize).checked_add(F::ERASE_SIZE) else {
                return Err(FlashJournalError::OutOfBounds);
            };
            if end > flash.capacity() {
                return Err(FlashJournalError::OutOfBounds);
            }
            let region_state = scan_timebase_region::<F>(flash, region).await?;
            state.high_water =
                max_millis(state.high_water.map(|value| value.0), region_state.newest)
                    .map(InstantMillis);
            state.last_compaction_attempt = max_millis(
                state.last_compaction_attempt.map(|value| value.0),
                region_state.last_compaction_attempt,
            )
            .map(InstantMillis);
        }
        Ok(state)
    }

    pub async fn open(
        mut flash: F,
        layout: FlashJournalLayout,
        scratch: &mut [u8],
        mut on_record: impl FnMut(FlashJournalRecord<'_>),
    ) -> Result<(Self, FlashJournalRestoreReport), FlashJournalError<F::Error>> {
        validate::<F>(&flash, layout, scratch)?;
        let first =
            scan_arena::<F>(&mut flash, layout.arenas[0], scratch, None, &mut |_| {}).await?;
        let second =
            scan_arena::<F>(&mut flash, layout.arenas[1], scratch, None, &mut |_| {}).await?;
        let selected = select_active(&first, &second);
        let mut restored_records = 0u32;
        if let Some(index) = selected {
            let selected_state = if index == 0 { &first } else { &second };
            if selected_state.warning.is_none() {
                let epoch = selected_state.epoch;
                let _ = scan_arena::<F>(
                    &mut flash,
                    layout.arenas[index],
                    scratch,
                    epoch,
                    &mut |record| {
                        restored_records = restored_records.saturating_add(1);
                        on_record(record);
                    },
                )
                .await?;
            }
        }
        let active = selected.map(|index| {
            let state = if index == 0 { &first } else { &second };
            ArenaCursor {
                index,
                epoch: state.epoch.unwrap_or(0),
                append_at: if state.warning.is_none() {
                    state.append_at
                } else {
                    layout.arenas[index].end
                },
            }
        });
        let warning = selected.and_then(|index| {
            if index == 0 {
                first.warning
            } else {
                second.warning
            }
        });
        let active_epoch = active.map(|cursor| cursor.epoch);
        Ok((
            Self {
                flash,
                layout,
                active,
                compaction: None,
            },
            FlashJournalRestoreReport {
                active_epoch,
                restored_records,
                warning,
            },
        ))
    }

    pub fn release(self) -> F {
        self.flash
    }

    #[must_use]
    pub fn layout(&self) -> FlashJournalLayout {
        self.layout
    }

    #[must_use]
    pub fn active_epoch(&self) -> Option<u64> {
        self.active.map(|cursor| cursor.epoch)
    }

    #[must_use]
    /// Capture before `append` when the owner may need to confirm a cancelled write.
    /// This is a cursor position, not a guarantee that the next record fits.
    pub fn active_append_offset(&self) -> Option<u32> {
        self.active.map(|cursor| cursor.append_at)
    }

    #[must_use]
    pub fn active_remaining_bytes(&self) -> Option<usize> {
        self.active.map(|cursor| {
            (self.layout.arenas[cursor.index].end as usize)
                .saturating_sub(cursor.append_at as usize)
        })
    }

    #[must_use]
    pub fn active_can_fit(&self, payload_len: usize, reserve_bytes: usize) -> bool {
        let required = flash_journal_record_storage_len(payload_len, F::WRITE_SIZE)
            .saturating_add(reserve_bytes);
        self.active_remaining_bytes()
            .is_some_and(|remaining| remaining >= required)
    }

    pub async fn initialize_empty(&mut self) -> Result<(), FlashJournalError<F::Error>> {
        if self.active.is_some() {
            return Ok(());
        }
        let arena = self.layout.arenas[0];
        self.flash
            .erase(arena.start, arena.end)
            .await
            .map_err(FlashJournalError::Flash)?;
        let append_at = write_record::<F>(
            &mut self.flash,
            arena,
            arena.start,
            0,
            FlashJournalRecordKind::ArenaCommit,
            &[],
        )
        .await?;
        self.active = Some(ArenaCursor {
            index: 0,
            epoch: 0,
            append_at,
        });
        Ok(())
    }

    pub async fn append(
        &mut self,
        kind: FlashJournalRecordKind,
        payload: &[u8],
    ) -> Result<(), FlashJournalError<F::Error>> {
        if kind == FlashJournalRecordKind::ArenaCommit {
            return Err(FlashJournalError::CompactionInProgress);
        }
        if self.compaction.is_some() {
            return Err(FlashJournalError::CompactionInProgress);
        }
        let Some(cursor) = self.active.as_mut() else {
            return Err(FlashJournalError::Uninitialized);
        };
        write_cursor_record::<F>(
            &mut self.flash,
            self.layout.arenas[cursor.index],
            cursor,
            kind,
            payload,
        )
        .await?;
        Ok(())
    }

    /// Resolve an append whose completion was lost or whose commit readback failed. The owner must
    /// exclude other writes and compaction until this exact record is resolved.
    pub async fn confirm_append(
        &mut self,
        at: u32,
        kind: FlashJournalRecordKind,
        payload: &[u8],
    ) -> Result<FlashJournalCommitResolution, FlashJournalError<F::Error>> {
        if self.compaction.is_some() {
            return Err(FlashJournalError::CompactionInProgress);
        }
        let cursor = self
            .active
            .as_ref()
            .ok_or(FlashJournalError::Uninitialized)?;
        let arena = self.layout.arenas[cursor.index];
        check_record_bounds::<F>(arena, at, payload.len())?;
        let mut header = AlignedHeader([0; HEADER_LEN]);
        self.flash
            .read(at, &mut header.0)
            .await
            .map_err(FlashJournalError::Flash)?;
        let resolution = confirm_record_header(&header.0, cursor.epoch, kind, payload)?;
        if resolution == FlashJournalCommitResolution::Committed {
            verify_record_payload(&mut self.flash, at, payload).await?;
        }
        Ok(resolution)
    }

    #[must_use]
    pub fn inactive_sector_count(&self) -> usize {
        let arena = self.inactive_arena();
        arena.len() as usize / F::ERASE_SIZE
    }

    pub async fn erase_inactive_sector(
        &mut self,
        sector: usize,
    ) -> Result<(), FlashJournalError<F::Error>> {
        if self.compaction.is_some() {
            return Err(FlashJournalError::CompactionInProgress);
        }
        let arena = self.inactive_arena();
        let Some(offset) = sector.checked_mul(F::ERASE_SIZE) else {
            return Err(FlashJournalError::OutOfBounds);
        };
        let Some(from) = (arena.start as usize).checked_add(offset) else {
            return Err(FlashJournalError::OutOfBounds);
        };
        let Some(to) = from.checked_add(F::ERASE_SIZE) else {
            return Err(FlashJournalError::OutOfBounds);
        };
        if to > arena.end as usize {
            return Err(FlashJournalError::OutOfBounds);
        }
        self.flash
            .erase(from as u32, to as u32)
            .await
            .map_err(FlashJournalError::Flash)
    }

    pub fn begin_compaction(&mut self) -> Result<(), FlashJournalError<F::Error>> {
        if self.compaction.is_some() {
            return Err(FlashJournalError::CompactionInProgress);
        }
        let index = self.inactive_index();
        self.compaction = Some(ArenaCursor {
            index,
            epoch: self.active.map_or(0, |cursor| cursor.epoch.wrapping_add(1)),
            append_at: self.layout.arenas[index].start,
        });
        Ok(())
    }

    pub async fn append_compacted(
        &mut self,
        kind: FlashJournalRecordKind,
        payload: &[u8],
    ) -> Result<(), FlashJournalError<F::Error>> {
        if kind == FlashJournalRecordKind::ArenaCommit {
            return Err(FlashJournalError::CompactionInProgress);
        }
        let Some(cursor) = self.compaction.as_mut() else {
            return Err(FlashJournalError::NoCompaction);
        };
        write_cursor_record::<F>(
            &mut self.flash,
            self.layout.arenas[cursor.index],
            cursor,
            kind,
            payload,
        )
        .await?;
        Ok(())
    }

    #[must_use]
    pub fn compaction_append_offset(&self) -> Option<u32> {
        self.compaction.map(|cursor| cursor.append_at)
    }

    /// Confirm a possibly completed arena commit before allowing other writes or erases.
    /// A verified commit selects the new arena and restores its append cursor.
    pub async fn confirm_compaction_commit(
        &mut self,
        at: u32,
    ) -> Result<FlashJournalCommitResolution, FlashJournalError<F::Error>> {
        let mut cursor = self.compaction.ok_or(FlashJournalError::NoCompaction)?;
        check_record_bounds::<F>(self.layout.arenas[cursor.index], at, 0)?;
        let mut header = AlignedHeader([0; HEADER_LEN]);
        self.flash
            .read(at, &mut header.0)
            .await
            .map_err(FlashJournalError::Flash)?;
        let resolution = confirm_record_header(
            &header.0,
            cursor.epoch,
            FlashJournalRecordKind::ArenaCommit,
            &[],
        )?;
        if resolution == FlashJournalCommitResolution::Committed {
            cursor.append_at = record_end::<F>(at, 0).ok_or(FlashJournalError::PayloadTooLarge)?;
            self.active = Some(cursor);
            self.compaction = None;
        }
        Ok(resolution)
    }

    pub async fn commit_compaction(&mut self) -> Result<(), FlashJournalError<F::Error>> {
        let Some(cursor) = self.compaction.as_mut() else {
            return Err(FlashJournalError::NoCompaction);
        };
        // A cancelled commit may already have selected the new epoch on flash. Do not
        // allow later appends to the old epoch, where replay would silently ignore them.
        if let Some(active) = self.active.as_mut() {
            active.append_at = self.layout.arenas[active.index].end;
        }
        write_cursor_record::<F>(
            &mut self.flash,
            self.layout.arenas[cursor.index],
            cursor,
            FlashJournalRecordKind::ArenaCommit,
            &[],
        )
        .await?;
        self.active = Some(*cursor);
        self.compaction = None;
        Ok(())
    }

    pub fn abort_compaction(&mut self) {
        self.compaction = None;
    }

    pub async fn timebase_high_water(
        &mut self,
    ) -> Result<Option<InstantMillis>, FlashJournalError<F::Error>> {
        Ok(Self::inspect_timebase_state(&mut self.flash, self.layout)
            .await?
            .high_water)
    }

    pub async fn record_timebase(
        &mut self,
        now: InstantMillis,
    ) -> Result<(), FlashJournalError<F::Error>> {
        let value = now.0.saturating_add(TIMEBASE_HEADROOM_MILLIS);
        let states = [
            scan_timebase_region::<F>(&mut self.flash, self.layout.timebase_regions[0]).await?,
            scan_timebase_region::<F>(&mut self.flash, self.layout.timebase_regions[1]).await?,
        ];
        let newest = max_millis(states[0].newest, states[1].newest);
        if newest.is_some_and(|stored| stored >= value) {
            return Ok(());
        }
        let last_compaction_attempt = max_millis(
            states[0].last_compaction_attempt,
            states[1].last_compaction_attempt,
        );
        self.write_timebase_state(&states, value, last_compaction_attempt)
            .await
    }

    pub async fn record_compaction_budget(
        &mut self,
        attempted_at: InstantMillis,
    ) -> Result<InstantMillis, FlashJournalError<F::Error>> {
        let rounded_attempt = round_compaction_attempt_up(attempted_at)?;
        let states = [
            scan_timebase_region::<F>(&mut self.flash, self.layout.timebase_regions[0]).await?,
            scan_timebase_region::<F>(&mut self.flash, self.layout.timebase_regions[1]).await?,
        ];
        let last_compaction_attempt = max_millis(
            states[0].last_compaction_attempt,
            states[1].last_compaction_attempt,
        );
        if let Some(stored) = last_compaction_attempt.filter(|stored| *stored >= rounded_attempt.0)
        {
            return Ok(InstantMillis(stored));
        }
        let high_water = max_millis(
            max_millis(states[0].newest, states[1].newest),
            Some(attempted_at.0.saturating_add(TIMEBASE_HEADROOM_MILLIS)),
        )
        .unwrap_or(attempted_at.0);
        self.write_timebase_state(&states, high_water, Some(rounded_attempt.0))
            .await?;
        Ok(rounded_attempt)
    }

    async fn write_timebase_state(
        &mut self,
        states: &[TimebaseRegionState; 2],
        high_water: u64,
        last_compaction_attempt: Option<u64>,
    ) -> Result<(), FlashJournalError<F::Error>> {
        let active = newer_timebase_region(states);
        match states[active].append_at {
            Some(slot) => {
                write_timebase_slot::<F>(
                    &mut self.flash,
                    self.layout.timebase_regions[active],
                    slot,
                    high_water,
                    last_compaction_attempt,
                )
                .await
            }
            None => {
                let other = 1 - active;
                if states[other].any_used {
                    let from = self.layout.timebase_regions[other];
                    self.flash
                        .erase(from, from + F::ERASE_SIZE as u32)
                        .await
                        .map_err(FlashJournalError::Flash)?;
                }
                write_timebase_slot::<F>(
                    &mut self.flash,
                    self.layout.timebase_regions[other],
                    0,
                    high_water,
                    last_compaction_attempt,
                )
                .await
            }
        }
    }

    fn inactive_index(&self) -> usize {
        self.active.map_or(0, |cursor| 1 - cursor.index)
    }

    fn inactive_arena(&self) -> FlashArenaRange {
        self.layout.arenas[self.inactive_index()]
    }
}

fn validate<F: NorFlash>(
    flash: &F,
    layout: FlashJournalLayout,
    scratch: &[u8],
) -> Result<(), FlashJournalError<F::Error>> {
    if F::WRITE_SIZE == 0
        || F::READ_SIZE == 0
        || F::ERASE_SIZE == 0
        || F::WRITE_SIZE > 4
        || !HEADER_LEN.is_multiple_of(F::WRITE_SIZE)
        || !HEADER_LEN.is_multiple_of(F::READ_SIZE)
        || scratch.len() < io_alignment::<F>()
    {
        return Err(FlashJournalError::Misaligned);
    }
    if scratch.len() < IO_CHUNK_LEN.min(io_alignment::<F>())
        || !scratch.len().is_multiple_of(F::READ_SIZE)
    {
        return Err(FlashJournalError::ScratchTooShort);
    }
    for region in layout.timebase_regions {
        if !(region as usize).is_multiple_of(F::ERASE_SIZE) {
            return Err(FlashJournalError::Misaligned);
        }
        let Some(end) = (region as usize).checked_add(F::ERASE_SIZE) else {
            return Err(FlashJournalError::OutOfBounds);
        };
        if end > flash.capacity() {
            return Err(FlashJournalError::OutOfBounds);
        }
    }
    if layout.timebase_regions[0].abs_diff(layout.timebase_regions[1]) < F::ERASE_SIZE as u32 {
        return Err(FlashJournalError::Misaligned);
    }
    for arena in layout.arenas {
        if arena.is_empty()
            || !(arena.start as usize).is_multiple_of(F::ERASE_SIZE)
            || !(arena.end as usize).is_multiple_of(F::ERASE_SIZE)
        {
            return Err(FlashJournalError::Misaligned);
        }
        if arena.end as usize > flash.capacity() {
            return Err(FlashJournalError::OutOfBounds);
        }
        for region in layout.timebase_regions {
            let timebase = FlashArenaRange::new(region, region + F::ERASE_SIZE as u32);
            if overlaps(arena, timebase) {
                return Err(FlashJournalError::Misaligned);
            }
        }
    }
    if overlaps(layout.arenas[0], layout.arenas[1]) {
        return Err(FlashJournalError::Misaligned);
    }
    Ok(())
}

fn overlaps(first: FlashArenaRange, second: FlashArenaRange) -> bool {
    first.start < second.end && second.start < first.end
}

fn select_active(first: &ArenaState, second: &ArenaState) -> Option<usize> {
    match (first.epoch, second.epoch) {
        (Some(first_epoch), Some(second_epoch)) => {
            if epoch_is_newer(second_epoch, first_epoch) {
                Some(1)
            } else {
                Some(0)
            }
        }
        (Some(_), None) => Some(0),
        (None, Some(_)) => Some(1),
        (None, None) => None,
    }
}

fn epoch_is_newer(candidate: u64, current: u64) -> bool {
    let delta = candidate.wrapping_sub(current);
    delta != 0 && delta < (1u64 << 63)
}

async fn scan_arena<F: NorFlash>(
    flash: &mut F,
    arena: FlashArenaRange,
    scratch: &mut [u8],
    replay_epoch: Option<u64>,
    on_record: &mut impl FnMut(FlashJournalRecord<'_>),
) -> Result<ArenaState, FlashJournalError<F::Error>> {
    let mut at = arena.start;
    let mut committed_epoch = None;
    let mut warning = None;
    while (at as usize).saturating_add(HEADER_LEN) <= arena.end as usize {
        let mut header_bytes = AlignedHeader([0xFF; HEADER_LEN]);
        flash
            .read(at, &mut header_bytes.0)
            .await
            .map_err(FlashJournalError::Flash)?;
        if header_bytes.0.iter().all(|byte| *byte == 0xFF) {
            break;
        }
        let Some(header) = parse_header(&header_bytes.0) else {
            at = arena.end;
            break;
        };
        if !header.committed {
            at = arena.end;
            break;
        }
        if header_bytes.0[..4] != MAGIC {
            warning = Some(FlashJournalWarning::Corrupt);
            at = arena.end;
            break;
        }
        let Some(record_end) = record_end::<F>(at, header.payload_len) else {
            warning = Some(FlashJournalWarning::Corrupt);
            at = arena.end;
            break;
        };
        if record_end > arena.end {
            warning = Some(FlashJournalWarning::Corrupt);
            at = arena.end;
            break;
        }
        if header.payload_len > scratch.len() {
            return Err(FlashJournalError::ScratchTooShort);
        }
        let padded_len =
            align_up(header.payload_len, F::READ_SIZE).ok_or(FlashJournalError::PayloadTooLarge)?;
        if padded_len > scratch.len() {
            return Err(FlashJournalError::ScratchTooShort);
        }
        if padded_len > 0 {
            flash
                .read(at + HEADER_LEN as u32, &mut scratch[..padded_len])
                .await
                .map_err(FlashJournalError::Flash)?;
        }
        if record_checksum(
            &header_bytes.0[..CHECKSUM_PREFIX_LEN],
            &scratch[..header.payload_len],
        ) != header.checksum
        {
            warning = Some(FlashJournalWarning::Corrupt);
            at = arena.end;
            break;
        }
        if header.schema != SCHEMA_VERSION {
            warning = Some(FlashJournalWarning::UnknownSchema {
                found: header.schema,
            });
            if header.kind == FlashJournalRecordKind::ArenaCommit as u16 {
                committed_epoch = Some(header.epoch);
            }
            at = record_end;
            continue;
        }
        let Some(kind) = FlashJournalRecordKind::from_wire(header.kind) else {
            warning = Some(FlashJournalWarning::Corrupt);
            at = arena.end;
            break;
        };
        if kind == FlashJournalRecordKind::ArenaCommit {
            if header.payload_len != 0 {
                warning = Some(FlashJournalWarning::Corrupt);
                at = arena.end;
                break;
            }
            committed_epoch = Some(header.epoch);
        } else if replay_epoch == Some(header.epoch) {
            on_record(FlashJournalRecord {
                epoch: header.epoch,
                kind,
                payload: &scratch[..header.payload_len],
            });
        }
        at = record_end;
    }
    Ok(ArenaState {
        epoch: committed_epoch,
        append_at: at,
        warning,
    })
}

fn parse_header(bytes: &[u8; HEADER_LEN]) -> Option<RecordHeader> {
    let schema = u16::from_le_bytes(bytes[4..6].try_into().ok()?);
    let kind = u16::from_le_bytes(bytes[6..8].try_into().ok()?);
    let epoch = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let payload_len = u32::from_le_bytes(bytes[16..20].try_into().ok()?) as usize;
    let checksum = u32::from_le_bytes(bytes[20..24].try_into().ok()?);
    let commit = u32::from_le_bytes(bytes[COMMIT_OFFSET..HEADER_LEN].try_into().ok()?);
    Some(RecordHeader {
        schema,
        kind,
        epoch,
        payload_len,
        checksum,
        committed: commit == COMMIT_WORD,
    })
}

fn check_record_bounds<F: NorFlash>(
    arena: FlashArenaRange,
    at: u32,
    payload_len: usize,
) -> Result<(), FlashJournalError<F::Error>> {
    let end = record_end::<F>(at, payload_len).ok_or(FlashJournalError::PayloadTooLarge)?;
    if at < arena.start || end > arena.end || !at.is_multiple_of(F::READ_SIZE as u32) {
        return Err(FlashJournalError::OutOfBounds);
    }
    Ok(())
}

fn confirm_record_header<E>(
    header: &[u8; HEADER_LEN],
    epoch: u64,
    kind: FlashJournalRecordKind,
    payload: &[u8],
) -> Result<FlashJournalCommitResolution, FlashJournalError<E>> {
    if header[COMMIT_OFFSET..] != COMMIT_WORD.to_le_bytes() {
        return Ok(FlashJournalCommitResolution::NotCommitted);
    }
    let checksum = record_checksum(&header[..CHECKSUM_PREFIX_LEN], payload);
    if !header_matches_record(header, epoch, kind, payload.len() as u32, checksum, true) {
        return Err(FlashJournalError::VerificationFailed);
    }
    Ok(FlashJournalCommitResolution::Committed)
}

async fn write_cursor_record<F: NorFlash>(
    flash: &mut F,
    arena: FlashArenaRange,
    cursor: &mut ArenaCursor,
    kind: FlashJournalRecordKind,
    payload: &[u8],
) -> Result<(), FlashJournalError<F::Error>> {
    let at = cursor.append_at;
    let end = record_end::<F>(at, payload.len()).ok_or(FlashJournalError::PayloadTooLarge)?;
    if at < arena.start || end > arena.end {
        return Err(FlashJournalError::ArenaFull);
    }
    // An error or cancellation can leave programmed cells behind. Only a verified write
    // makes this tail appendable again; otherwise it must be recovered through compaction.
    cursor.append_at = arena.end;
    cursor.append_at = write_record(flash, arena, at, cursor.epoch, kind, payload).await?;
    Ok(())
}

async fn write_record<F: NorFlash>(
    flash: &mut F,
    arena: FlashArenaRange,
    at: u32,
    epoch: u64,
    kind: FlashJournalRecordKind,
    payload: &[u8],
) -> Result<u32, FlashJournalError<F::Error>> {
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| FlashJournalError::PayloadTooLarge)?;
    let Some(end) = record_end::<F>(at, payload.len()) else {
        return Err(FlashJournalError::PayloadTooLarge);
    };
    if at < arena.start || end > arena.end {
        return Err(FlashJournalError::ArenaFull);
    }
    let checksum = {
        let mut header = record_header(epoch, kind, payload_len, 0, false);
        let checksum = record_checksum(&header.0[..CHECKSUM_PREFIX_LEN], payload);
        header.0[20..24].copy_from_slice(&checksum.to_le_bytes());
        flash
            .write(at, &header.0[..COMMIT_OFFSET])
            .await
            .map_err(FlashJournalError::Flash)?;
        checksum
    };
    let mut payload_at = at + HEADER_LEN as u32;
    let mut remaining = payload;
    while !remaining.is_empty() {
        let take = remaining.len().min(IO_CHUNK_LEN);
        let final_chunk = take == remaining.len();
        let write_len = if final_chunk {
            align_up(take, F::WRITE_SIZE).ok_or(FlashJournalError::PayloadTooLarge)?
        } else {
            take
        };
        let mut chunk = AlignedIo([0xFF; IO_CHUNK_LEN]);
        chunk.0[..take].copy_from_slice(&remaining[..take]);
        flash
            .write(payload_at, &chunk.0[..write_len])
            .await
            .map_err(FlashJournalError::Flash)?;
        payload_at += write_len as u32;
        remaining = &remaining[take..];
    }

    // Verify the complete uncommitted record before making it visible. A driver accepting a write
    // is not proof that the flash cells hold the requested bytes; the commit word is deliberately
    // last so a torn or misprogrammed record remains unavailable during replay.
    {
        let mut header = AlignedHeader([0u8; HEADER_LEN]);
        flash
            .read(at, &mut header.0)
            .await
            .map_err(FlashJournalError::Flash)?;
        if !header_matches_record(&header.0, epoch, kind, payload_len, checksum, false) {
            return Err(FlashJournalError::VerificationFailed);
        }
    }
    verify_record_payload(flash, at, payload).await?;

    let commit = AlignedCommit(COMMIT_WORD.to_le_bytes());
    let commit_result = flash.write(at + COMMIT_OFFSET as u32, &commit.0).await;
    {
        let mut header = AlignedHeader([0u8; HEADER_LEN]);
        let readback = flash.read(at, &mut header.0).await;
        if let Err(error) = commit_result {
            if readback.is_err() {
                return Err(FlashJournalError::CommitUnconfirmed { at, error });
            }
            if readback.is_ok()
                && header_matches_record(&header.0, epoch, kind, payload_len, checksum, true)
            {
                return Ok(end);
            }
            return Err(FlashJournalError::Flash(error));
        }
        readback.map_err(|error| FlashJournalError::CommitUnconfirmed { at, error })?;
        if !header_matches_record(&header.0, epoch, kind, payload_len, checksum, true) {
            return Err(FlashJournalError::VerificationFailed);
        }
    }
    Ok(end)
}

#[inline(never)]
async fn verify_record_payload<F: NorFlash>(
    flash: &mut F,
    at: u32,
    payload: &[u8],
) -> Result<(), FlashJournalError<F::Error>> {
    let mut payload_at = at + HEADER_LEN as u32;
    for remaining in payload.chunks(IO_CHUNK_LEN) {
        let read_len =
            align_up(remaining.len(), F::READ_SIZE).ok_or(FlashJournalError::PayloadTooLarge)?;
        let mut chunk = AlignedIo([0; IO_CHUNK_LEN]);
        flash
            .read(payload_at, &mut chunk.0[..read_len])
            .await
            .map_err(FlashJournalError::Flash)?;
        if chunk.0[..remaining.len()] != *remaining
            || chunk.0[remaining.len()..read_len]
                .iter()
                .any(|byte| *byte != 0xff)
        {
            return Err(FlashJournalError::VerificationFailed);
        }
        payload_at += read_len as u32;
    }
    Ok(())
}

fn header_matches_record(
    bytes: &[u8; HEADER_LEN],
    epoch: u64,
    kind: FlashJournalRecordKind,
    payload_len: u32,
    checksum: u32,
    committed: bool,
) -> bool {
    *bytes == record_header(epoch, kind, payload_len, checksum, committed).0
}

fn record_header(
    epoch: u64,
    kind: FlashJournalRecordKind,
    payload_len: u32,
    checksum: u32,
    committed: bool,
) -> AlignedHeader {
    let expected_commit = if committed {
        COMMIT_WORD.to_le_bytes()
    } else {
        [0xFF; 4]
    };
    let mut header = AlignedHeader([0xFF; HEADER_LEN]);
    header.0[..4].copy_from_slice(&MAGIC);
    header.0[4..6].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
    header.0[6..8].copy_from_slice(&(kind as u16).to_le_bytes());
    header.0[8..16].copy_from_slice(&epoch.to_le_bytes());
    header.0[16..20].copy_from_slice(&payload_len.to_le_bytes());
    header.0[20..24].copy_from_slice(&checksum.to_le_bytes());
    header.0[COMMIT_OFFSET..].copy_from_slice(&expected_commit);
    header
}

fn record_end<F: NorFlash>(at: u32, payload_len: usize) -> Option<u32> {
    let padded = align_up(payload_len, io_alignment::<F>())?;
    let len = HEADER_LEN.checked_add(padded)?;
    at.checked_add(u32::try_from(len).ok()?)
}

fn io_alignment<F: NorFlash>() -> usize {
    F::WRITE_SIZE.max(F::READ_SIZE)
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    let remainder = value % alignment;
    if remainder == 0 {
        return Some(value);
    }
    value.checked_add(alignment - remainder)
}

fn record_checksum(prefix: &[u8], payload: &[u8]) -> u32 {
    let prefix_crc = crc32(prefix);
    let mut crc = !prefix_crc;
    for &byte in payload {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let low_bit_set = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & low_bit_set);
        }
    }
    !crc
}

async fn scan_timebase_region<F: NorFlash>(
    flash: &mut F,
    region: u32,
) -> Result<TimebaseRegionState, FlashJournalError<F::Error>> {
    let mut newest = None;
    let mut last_compaction_attempt = None;
    let mut last_used = None;
    for slot in 0..F::ERASE_SIZE / TIMEBASE_SLOT_LEN {
        let mut bytes = AlignedTimebase([0u8; TIMEBASE_SLOT_LEN]);
        flash
            .read(region + (slot * TIMEBASE_SLOT_LEN) as u32, &mut bytes.0)
            .await
            .map_err(FlashJournalError::Flash)?;
        if bytes.0.iter().all(|byte| *byte == 0xFF) {
            continue;
        }
        last_used = Some(slot);
        if let Ok(value) = read_timebase_snapshot(&bytes.0) {
            newest = max_millis(newest, Some(value.0));
            last_compaction_attempt = max_millis(
                last_compaction_attempt,
                read_compaction_budget(&bytes.0[COMPACTION_BUDGET_OFFSET..]),
            );
        }
    }
    let append_at = match last_used {
        None => Some(0),
        Some(last) if last + 1 < F::ERASE_SIZE / TIMEBASE_SLOT_LEN => Some(last + 1),
        Some(_) => None,
    };
    Ok(TimebaseRegionState {
        append_at,
        newest,
        last_compaction_attempt,
        any_used: last_used.is_some(),
    })
}

async fn write_timebase_slot<F: NorFlash>(
    flash: &mut F,
    region: u32,
    slot: usize,
    value: u64,
    last_compaction_attempt: Option<u64>,
) -> Result<(), FlashJournalError<F::Error>> {
    let mut sealed = AlignedTimebase([0xFF; TIMEBASE_SLOT_LEN]);
    write_timebase_snapshot(InstantMillis(value), &mut sealed.0[..TIMEBASE_SNAPSHOT_LEN])
        .map_err(|_| FlashJournalError::PayloadTooLarge)?;
    if let Some(attempt) = last_compaction_attempt {
        write_compaction_budget(attempt, &mut sealed.0[COMPACTION_BUDGET_OFFSET..])?;
    }
    flash
        .write(region + (slot * TIMEBASE_SLOT_LEN) as u32, &sealed.0)
        .await
        .map_err(FlashJournalError::Flash)
}

fn newer_timebase_region(states: &[TimebaseRegionState; 2]) -> usize {
    let first = (states[0].newest, states[0].last_compaction_attempt);
    let second = (states[1].newest, states[1].last_compaction_attempt);
    match (first, second) {
        ((Some(first_high), first_attempt), (Some(second_high), second_attempt)) => {
            if second_high > first_high
                || (second_high == first_high && second_attempt > first_attempt)
            {
                1
            } else {
                0
            }
        }
        ((None, first_attempt), (None, second_attempt)) if second_attempt > first_attempt => 1,
        ((None, _), (Some(_), _)) => 1,
        _ => 0,
    }
}

fn round_compaction_attempt_up<E>(
    attempted_at: InstantMillis,
) -> Result<InstantMillis, FlashJournalError<E>> {
    let minutes = attempted_at.0 / COMPACTION_BUDGET_MINUTE_MILLIS;
    let rounded_minutes = minutes.saturating_add(u64::from(
        !attempted_at
            .0
            .is_multiple_of(COMPACTION_BUDGET_MINUTE_MILLIS),
    ));
    let encoded = u32::try_from(rounded_minutes).map_err(|_| FlashJournalError::PayloadTooLarge)?;
    Ok(InstantMillis(
        u64::from(encoded).saturating_mul(COMPACTION_BUDGET_MINUTE_MILLIS),
    ))
}

fn write_compaction_budget<E>(
    attempted_at: u64,
    out: &mut [u8],
) -> Result<(), FlashJournalError<E>> {
    let minutes = u32::try_from(attempted_at / COMPACTION_BUDGET_MINUTE_MILLIS)
        .map_err(|_| FlashJournalError::PayloadTooLarge)?;
    let encoded = minutes.to_le_bytes();
    out[..4].copy_from_slice(&encoded);
    out[4..].copy_from_slice(&compaction_budget_checksum(encoded).to_le_bytes());
    Ok(())
}

fn read_compaction_budget(bytes: &[u8]) -> Option<u64> {
    let encoded: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    let checksum = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    if checksum != compaction_budget_checksum(encoded) {
        return None;
    }
    Some(u64::from(u32::from_le_bytes(encoded)) * COMPACTION_BUDGET_MINUTE_MILLIS)
}

fn compaction_budget_checksum(encoded: [u8; 4]) -> u32 {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&COMPACTION_BUDGET_DOMAIN);
    bytes[4..].copy_from_slice(&encoded);
    crc32(&bytes)
}

fn max_millis(first: Option<u64>, second: Option<u64>) -> Option<u64> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first.max(second)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[repr(C, align(4))]
struct AlignedHeader([u8; HEADER_LEN]);

#[repr(C, align(4))]
struct AlignedIo([u8; IO_CHUNK_LEN]);

impl Drop for AlignedIo {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[repr(C, align(4))]
struct AlignedCommit([u8; 4]);

#[repr(C, align(4))]
struct AlignedTimebase([u8; TIMEBASE_SLOT_LEN]);

#[cfg(test)]
mod tests;
