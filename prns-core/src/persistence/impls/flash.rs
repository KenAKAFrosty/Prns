use embedded_storage::nor_flash::NorFlash;

use crate::persistence::{
    read_timebase_snapshot, write_timebase_snapshot, SnapshotSealError, TIMEBASE_SNAPSHOT_LEN,
};
use crate::units::InstantMillis;

const SLOT_LEN: usize = 32;

pub const TIMEBASE_RECORD_INTERVAL_MILLIS: u64 = 30 * 60 * 1000;
pub const TIMEBASE_HEADROOM_MILLIS: u64 = 2 * 60 * 60 * 1000;
const _: () = assert!(TIMEBASE_HEADROOM_MILLIS >= 4 * TIMEBASE_RECORD_INTERVAL_MILLIS);

pub struct FlashTimebase<F: NorFlash> {
    flash: F,
    regions: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashTimebaseError<E> {
    Flash(E),
    Misaligned,
    OutOfBounds,
    Seal(SnapshotSealError),
}

struct RegionState {
    append_at: Option<usize>,
    newest: Option<u64>,
    any_used: bool,
}

impl<F: NorFlash> FlashTimebase<F> {
    pub fn new(flash: F, regions: [u32; 2]) -> Self {
        Self { flash, regions }
    }

    pub fn release(self) -> F {
        self.flash
    }

    pub fn high_water(&mut self) -> Result<Option<InstantMillis>, FlashTimebaseError<F::Error>> {
        validate(&self.flash, self.regions)?;
        let mut newest = None;
        for region in self.regions {
            let state = scan_region(&mut self.flash, region)?;
            newest = max_millis(newest, state.newest);
        }
        Ok(newest.map(InstantMillis))
    }

    pub fn record(&mut self, now: InstantMillis) -> Result<(), FlashTimebaseError<F::Error>> {
        validate(&self.flash, self.regions)?;
        let value = now.0.saturating_add(TIMEBASE_HEADROOM_MILLIS);
        let states = [
            scan_region(&mut self.flash, self.regions[0])?,
            scan_region(&mut self.flash, self.regions[1])?,
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
            Some(slot) => self.write_slot(self.regions[active], slot, value),
            None => {
                let other = 1 - active;
                if states[other].any_used {
                    let from = self.regions[other];
                    let to = from + erase_len::<F>();
                    self.flash
                        .erase(from, to)
                        .map_err(FlashTimebaseError::Flash)?;
                }
                self.write_slot(self.regions[other], 0, value)
            }
        }
    }

    fn write_slot(
        &mut self,
        region: u32,
        slot: usize,
        value: u64,
    ) -> Result<(), FlashTimebaseError<F::Error>> {
        let mut sealed = [0xFFu8; SLOT_LEN];
        write_timebase_snapshot(InstantMillis(value), &mut sealed[..TIMEBASE_SNAPSHOT_LEN])
            .map_err(FlashTimebaseError::Seal)?;
        let offset = region + (slot * SLOT_LEN) as u32;
        self.flash
            .write(offset, &sealed[..write_len::<F>()])
            .map_err(FlashTimebaseError::Flash)
    }
}

const fn slots_per_region<F: NorFlash>() -> usize {
    F::ERASE_SIZE / SLOT_LEN
}

const fn erase_len<F: NorFlash>() -> u32 {
    F::ERASE_SIZE as u32
}

const fn write_len<F: NorFlash>() -> usize {
    TIMEBASE_SNAPSHOT_LEN.div_ceil(F::WRITE_SIZE) * F::WRITE_SIZE
}

fn validate<F: NorFlash>(flash: &F, regions: [u32; 2]) -> Result<(), FlashTimebaseError<F::Error>> {
    if F::ERASE_SIZE < SLOT_LEN
        || !F::ERASE_SIZE.is_multiple_of(SLOT_LEN)
        || F::WRITE_SIZE > SLOT_LEN
        || !SLOT_LEN.is_multiple_of(F::WRITE_SIZE)
        || !SLOT_LEN.is_multiple_of(F::READ_SIZE)
    {
        return Err(FlashTimebaseError::Misaligned);
    }
    let span = regions[0].abs_diff(regions[1]);
    if span < erase_len::<F>() {
        return Err(FlashTimebaseError::Misaligned);
    }
    for region in regions {
        if !(region as usize).is_multiple_of(F::ERASE_SIZE) {
            return Err(FlashTimebaseError::Misaligned);
        }
        let Some(end) = (region as usize).checked_add(F::ERASE_SIZE) else {
            return Err(FlashTimebaseError::OutOfBounds);
        };
        if end > flash.capacity() {
            return Err(FlashTimebaseError::OutOfBounds);
        }
    }
    Ok(())
}

fn scan_region<F: NorFlash>(
    flash: &mut F,
    region: u32,
) -> Result<RegionState, FlashTimebaseError<F::Error>> {
    let mut newest = None;
    let mut last_used = None;
    for slot in 0..slots_per_region::<F>() {
        let mut bytes = [0u8; SLOT_LEN];
        flash
            .read(region + (slot * SLOT_LEN) as u32, &mut bytes)
            .map_err(FlashTimebaseError::Flash)?;
        if bytes.iter().all(|&byte| byte == 0xFF) {
            continue;
        }
        last_used = Some(slot);
        if let Ok(value) = read_timebase_snapshot(&bytes) {
            newest = max_millis(newest, Some(value.0));
        }
    }
    let append_at = match last_used {
        None => Some(0),
        Some(last) if last + 1 < slots_per_region::<F>() => Some(last + 1),
        Some(_) => None,
    };
    Ok(RegionState {
        append_at,
        newest,
        any_used: last_used.is_some(),
    })
}

const fn max_millis(first: Option<u64>, second: Option<u64>) -> Option<u64> {
    match (first, second) {
        (Some(first), Some(second)) => {
            if first >= second {
                Some(first)
            } else {
                Some(second)
            }
        }
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind, ReadNorFlash};

    const FAKE_WRITE: usize = 4;
    const FAKE_ERASE: usize = 4096;
    const REGIONS: [u32; 2] = [0, FAKE_ERASE as u32];
    const SLOTS: usize = FAKE_ERASE / SLOT_LEN;

    struct FakeFlash<const CAP: usize> {
        bytes: [u8; CAP],
        erase_count: usize,
        write_count: usize,
        fail_write_at: Option<usize>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeError {
        Unaligned,
        OutOfBounds,
        Interrupted,
    }

    impl<const CAP: usize> FakeFlash<CAP> {
        fn new() -> Self {
            Self {
                bytes: [0xFF; CAP],
                erase_count: 0,
                write_count: 0,
                fail_write_at: None,
            }
        }
    }

    impl NorFlashError for FakeError {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }

    impl<const CAP: usize> ErrorType for FakeFlash<CAP> {
        type Error = FakeError;
    }

    impl<const CAP: usize> ReadNorFlash for FakeFlash<CAP> {
        const READ_SIZE: usize = 4;

        fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let start = offset as usize;
            let end = start + bytes.len();
            if end > CAP {
                return Err(FakeError::OutOfBounds);
            }
            bytes.copy_from_slice(&self.bytes[start..end]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            CAP
        }
    }

    impl<const CAP: usize> NorFlash for FakeFlash<CAP> {
        const WRITE_SIZE: usize = FAKE_WRITE;
        const ERASE_SIZE: usize = FAKE_ERASE;

        fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            let (from, to) = (from as usize, to as usize);
            if !from.is_multiple_of(FAKE_ERASE)
                || !to.is_multiple_of(FAKE_ERASE)
                || from > to
                || to > CAP
            {
                return Err(FakeError::Unaligned);
            }
            for byte in &mut self.bytes[from..to] {
                *byte = 0xFF;
            }
            self.erase_count += 1;
            Ok(())
        }

        fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let start = offset as usize;
            if !start.is_multiple_of(FAKE_WRITE)
                || !bytes.len().is_multiple_of(FAKE_WRITE)
                || start + bytes.len() > CAP
            {
                return Err(FakeError::Unaligned);
            }
            self.write_count += 1;
            if self.fail_write_at == Some(self.write_count) {
                self.fail_write_at = None;
                return Err(FakeError::Interrupted);
            }
            for (index, byte) in bytes.iter().enumerate() {
                self.bytes[start + index] &= byte;
            }
            Ok(())
        }
    }

    fn journal() -> FlashTimebase<FakeFlash<8192>> {
        FlashTimebase::new(FakeFlash::new(), REGIONS)
    }

    #[test]
    fn an_empty_journal_has_no_high_water() {
        assert_eq!(journal().high_water(), Ok(None));
    }

    #[test]
    fn a_record_restores_with_headroom_past_the_recorded_instant() {
        let mut journal = journal();
        journal.record(InstantMillis(10_000)).unwrap();
        assert_eq!(
            journal.high_water(),
            Ok(Some(InstantMillis(10_000 + TIMEBASE_HEADROOM_MILLIS))),
        );
    }

    #[test]
    fn later_records_advance_the_high_water() {
        let mut journal = journal();
        journal.record(InstantMillis(10_000)).unwrap();
        journal
            .record(InstantMillis(10_000 + TIMEBASE_RECORD_INTERVAL_MILLIS))
            .unwrap();
        assert_eq!(
            journal.high_water(),
            Ok(Some(InstantMillis(
                10_000 + TIMEBASE_RECORD_INTERVAL_MILLIS + TIMEBASE_HEADROOM_MILLIS
            ))),
        );
        assert_eq!(journal.flash.write_count, 2);
    }

    #[test]
    fn a_stalled_clock_burns_no_flash_writes() {
        let mut journal = journal();
        journal.record(InstantMillis(10_000)).unwrap();
        journal.record(InstantMillis(10_000)).unwrap();
        journal.record(InstantMillis(9_000)).unwrap();
        assert_eq!(journal.flash.write_count, 1);
    }

    #[test]
    fn garbage_slots_are_skipped_without_hiding_valid_records() {
        let mut journal = journal();
        journal.flash.bytes[..SLOT_LEN].copy_from_slice(&[0xAB; SLOT_LEN]);
        journal.record(InstantMillis(10_000)).unwrap();
        assert_eq!(
            journal.high_water(),
            Ok(Some(InstantMillis(10_000 + TIMEBASE_HEADROOM_MILLIS))),
        );
    }

    #[test]
    fn an_interrupted_write_keeps_the_previous_floor_and_recovers() {
        let mut journal = journal();
        journal.record(InstantMillis(10_000)).unwrap();
        journal.flash.fail_write_at = Some(journal.flash.write_count + 1);
        assert_eq!(
            journal.record(InstantMillis(20_000_000)),
            Err(FlashTimebaseError::Flash(FakeError::Interrupted)),
        );
        assert_eq!(
            journal.high_water(),
            Ok(Some(InstantMillis(10_000 + TIMEBASE_HEADROOM_MILLIS))),
        );
        journal.record(InstantMillis(30_000_000)).unwrap();
        assert_eq!(
            journal.high_water(),
            Ok(Some(InstantMillis(30_000_000 + TIMEBASE_HEADROOM_MILLIS))),
        );
    }

    #[test]
    fn filling_one_region_rolls_into_the_empty_sibling_without_an_erase() {
        let mut journal = journal();
        for step in 0..SLOTS {
            journal
                .record(InstantMillis(
                    (step as u64 + 1) * TIMEBASE_RECORD_INTERVAL_MILLIS,
                ))
                .unwrap();
        }
        assert_eq!(journal.flash.write_count, SLOTS);
        assert_eq!(journal.flash.erase_count, 0);

        journal
            .record(InstantMillis(
                (SLOTS as u64 + 1) * TIMEBASE_RECORD_INTERVAL_MILLIS,
            ))
            .unwrap();
        assert_eq!(journal.flash.erase_count, 0);
        assert!(!journal.flash.bytes[FAKE_ERASE..FAKE_ERASE + SLOT_LEN]
            .iter()
            .all(|&byte| byte == 0xFF));
        assert_eq!(
            journal.high_water(),
            Ok(Some(InstantMillis(
                (SLOTS as u64 + 1) * TIMEBASE_RECORD_INTERVAL_MILLIS + TIMEBASE_HEADROOM_MILLIS
            ))),
        );
    }

    #[test]
    fn filling_the_sibling_erases_the_stale_region_before_reusing_it() {
        let mut journal = journal();
        for step in 0..(2 * SLOTS) {
            journal
                .record(InstantMillis(
                    (step as u64 + 1) * TIMEBASE_RECORD_INTERVAL_MILLIS,
                ))
                .unwrap();
        }
        assert_eq!(journal.flash.erase_count, 0);

        journal
            .record(InstantMillis(
                (2 * SLOTS as u64 + 1) * TIMEBASE_RECORD_INTERVAL_MILLIS,
            ))
            .unwrap();
        assert_eq!(journal.flash.erase_count, 1);
        assert_eq!(
            journal.high_water(),
            Ok(Some(InstantMillis(
                (2 * SLOTS as u64 + 1) * TIMEBASE_RECORD_INTERVAL_MILLIS + TIMEBASE_HEADROOM_MILLIS
            ))),
        );
    }

    #[test]
    fn misaligned_and_overlapping_regions_are_refused() {
        let mut unaligned = FlashTimebase::new(FakeFlash::<8192>::new(), [0, 100]);
        assert_eq!(
            unaligned.high_water(),
            Err(FlashTimebaseError::Misaligned::<FakeError>),
        );
        let mut overlapping = FlashTimebase::new(FakeFlash::<8192>::new(), [0, 0]);
        assert_eq!(
            overlapping.high_water(),
            Err(FlashTimebaseError::Misaligned::<FakeError>),
        );
        let mut outside = FlashTimebase::new(FakeFlash::<4096>::new(), REGIONS);
        assert_eq!(
            outside.high_water(),
            Err(FlashTimebaseError::OutOfBounds::<FakeError>),
        );
    }
}
