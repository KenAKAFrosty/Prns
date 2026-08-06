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
}

impl FlashJournalRecordKind {
    fn from_wire(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::ArenaCommit),
            2 => Some(Self::RouteUpsert),
            3 => Some(Self::RouteRemoval),
            4 => Some(Self::SelfRatchet),
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

pub struct FlashJournalRecord<'a> {
    pub epoch: u64,
    pub kind: FlashJournalRecordKind,
    pub payload: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub enum FlashJournalError<E> {
    Flash(E),
    Misaligned,
    OutOfBounds,
    ArenaFull,
    Uninitialized,
    CompactionInProgress,
    NoCompaction,
    PayloadTooLarge,
    ScratchTooShort,
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
        let mut newest = None;
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
            let state = scan_timebase_region::<F>(flash, region).await?;
            newest = max_millis(newest, state.newest);
        }
        Ok(newest.map(InstantMillis))
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
        let Some(mut cursor) = self.active else {
            return Err(FlashJournalError::Uninitialized);
        };
        cursor.append_at = write_record::<F>(
            &mut self.flash,
            self.layout.arenas[cursor.index],
            cursor.append_at,
            cursor.epoch,
            kind,
            payload,
        )
        .await?;
        self.active = Some(cursor);
        Ok(())
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
        let Some(mut cursor) = self.compaction else {
            return Err(FlashJournalError::NoCompaction);
        };
        cursor.append_at = write_record::<F>(
            &mut self.flash,
            self.layout.arenas[cursor.index],
            cursor.append_at,
            cursor.epoch,
            kind,
            payload,
        )
        .await?;
        self.compaction = Some(cursor);
        Ok(())
    }

    pub async fn commit_compaction(&mut self) -> Result<(), FlashJournalError<F::Error>> {
        let Some(mut cursor) = self.compaction else {
            return Err(FlashJournalError::NoCompaction);
        };
        cursor.append_at = write_record::<F>(
            &mut self.flash,
            self.layout.arenas[cursor.index],
            cursor.append_at,
            cursor.epoch,
            FlashJournalRecordKind::ArenaCommit,
            &[],
        )
        .await?;
        self.active = Some(cursor);
        self.compaction = None;
        Ok(())
    }

    pub fn abort_compaction(&mut self) {
        self.compaction = None;
    }

    pub async fn timebase_high_water(
        &mut self,
    ) -> Result<Option<InstantMillis>, FlashJournalError<F::Error>> {
        let mut newest = None;
        for region in self.layout.timebase_regions {
            let state = scan_timebase_region::<F>(&mut self.flash, region).await?;
            newest = max_millis(newest, state.newest);
        }
        Ok(newest.map(InstantMillis))
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
        let active = match (states[0].newest, states[1].newest) {
            (Some(first), Some(second)) if second > first => 1,
            (None, Some(_)) => 1,
            _ => 0,
        };
        match states[active].append_at {
            Some(slot) => {
                write_timebase_slot::<F>(
                    &mut self.flash,
                    self.layout.timebase_regions[active],
                    slot,
                    value,
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
                    value,
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
    let mut header = AlignedHeader([0xFF; HEADER_LEN]);
    header.0[..4].copy_from_slice(&MAGIC);
    header.0[4..6].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
    header.0[6..8].copy_from_slice(&(kind as u16).to_le_bytes());
    header.0[8..16].copy_from_slice(&epoch.to_le_bytes());
    header.0[16..20].copy_from_slice(&payload_len.to_le_bytes());
    let checksum = record_checksum(&header.0[..CHECKSUM_PREFIX_LEN], payload);
    header.0[20..24].copy_from_slice(&checksum.to_le_bytes());
    flash
        .write(at, &header.0[..COMMIT_OFFSET])
        .await
        .map_err(FlashJournalError::Flash)?;
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
    let commit = AlignedCommit(COMMIT_WORD.to_le_bytes());
    flash
        .write(at + COMMIT_OFFSET as u32, &commit.0)
        .await
        .map_err(FlashJournalError::Flash)?;
    Ok(end)
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
        any_used: last_used.is_some(),
    })
}

async fn write_timebase_slot<F: NorFlash>(
    flash: &mut F,
    region: u32,
    slot: usize,
    value: u64,
) -> Result<(), FlashJournalError<F::Error>> {
    let mut sealed = AlignedTimebase([0xFF; TIMEBASE_SLOT_LEN]);
    write_timebase_snapshot(InstantMillis(value), &mut sealed.0[..TIMEBASE_SNAPSHOT_LEN])
        .map_err(|_| FlashJournalError::PayloadTooLarge)?;
    let write_len =
        align_up(TIMEBASE_SNAPSHOT_LEN, F::WRITE_SIZE).ok_or(FlashJournalError::PayloadTooLarge)?;
    flash
        .write(
            region + (slot * TIMEBASE_SLOT_LEN) as u32,
            &sealed.0[..write_len],
        )
        .await
        .map_err(FlashJournalError::Flash)
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
mod tests {
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
    }

    impl FakeFlash {
        fn new() -> Self {
            Self {
                bytes: [0xFF; CAPACITY],
                operation: 0,
                fail_at: None,
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
            let (_, report, records) = open(journal.release()).await;
            assert_eq!(
                report,
                FlashJournalRestoreReport {
                    active_epoch: Some(0),
                    restored_records: 2,
                    warning: None,
                }
            );
            assert_eq!(
                records,
                vec![
                    (FlashJournalRecordKind::RouteUpsert, b"first".to_vec()),
                    (FlashJournalRecordKind::RouteRemoval, b"second".to_vec()),
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
}
