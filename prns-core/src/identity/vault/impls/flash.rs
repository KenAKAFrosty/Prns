use core::cell::RefCell;

use embedded_storage::nor_flash::NorFlash;

use crate::identity::vault::{
    IdentityLabel, IdentitySecretKey, IdentityVault, Removal, MAX_IDENTITY_LABEL_LEN,
};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

const SLOT_LEN: usize = 256;
const SECRET_LEN: usize = IDENTITY_SECRET_KEY_LEN;
const LABEL_CAP: usize = MAX_IDENTITY_LABEL_LEN;
const STATE_OFFSET: usize = 0;
const LABEL_LEN_OFFSET: usize = 1;
const LABEL_OFFSET: usize = 2;
const SECRET_OFFSET: usize = LABEL_OFFSET + LABEL_CAP;
const STATE_EMPTY: u8 = 0xFF;
const STATE_OCCUPIED: u8 = 0xA5;

pub struct FlashVault<F: NorFlash, const SLOTS: usize> {
    flash: RefCell<F>,
    offset: u32,
}

#[derive(Debug)]
pub enum FlashVaultError<E> {
    Flash(E),
    StoreFull,
    Misaligned,
    OutOfBounds,
    Corrupt,
}

struct Record {
    label: IdentityLabel,
    secret: IdentitySecretKey,
}

impl<F: NorFlash, const SLOTS: usize> FlashVault<F, SLOTS> {
    pub fn new(flash: F, offset: u32) -> Self {
        Self {
            flash: RefCell::new(flash),
            offset,
        }
    }

    pub fn release(self) -> F {
        self.flash.into_inner()
    }
}

impl<F: NorFlash, const SLOTS: usize> IdentityVault for FlashVault<F, SLOTS> {
    type Error = FlashVaultError<F::Error>;

    fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
        let mut flash = self.flash.borrow_mut();
        validate::<F>(&flash, self.offset, SLOTS)?;
        for index in 0..SLOTS {
            if let Some(record) = read_slot(&mut *flash, self.offset, index)? {
                if &record.label == label {
                    return Ok(Some(record.secret));
                }
            }
        }
        Ok(None)
    }

    fn store(
        &mut self,
        label: &IdentityLabel,
        secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<(), Self::Error> {
        let flash = self.flash.get_mut();
        validate::<F>(flash, self.offset, SLOTS)?;
        let mut records = read_records::<F, SLOTS>(flash, self.offset)?;
        match records.iter_mut().find(|record| &record.label == label) {
            Some(existing) => existing.secret = Zeroizing::new(*secret),
            None => records
                .push(Record {
                    label: label.clone(),
                    secret: Zeroizing::new(*secret),
                })
                .map_err(|_| FlashVaultError::StoreFull)?,
        }
        rewrite::<F, SLOTS>(flash, self.offset, &records)
    }

    fn remove(&mut self, label: &IdentityLabel) -> Result<Removal, Self::Error> {
        let flash = self.flash.get_mut();
        validate::<F>(flash, self.offset, SLOTS)?;
        let records = read_records::<F, SLOTS>(flash, self.offset)?;
        let mut kept = heapless::Vec::<Record, SLOTS>::new();
        let mut found = false;
        for record in records {
            if &record.label == label {
                found = true;
            } else {
                kept.push(record).map_err(|_| FlashVaultError::StoreFull)?;
            }
        }
        if !found {
            return Ok(Removal::NothingStored);
        }
        rewrite::<F, SLOTS>(flash, self.offset, &kept)?;
        Ok(Removal::Removed)
    }
}

fn validate<F: NorFlash>(
    flash: &F,
    offset: u32,
    slots: usize,
) -> Result<(), FlashVaultError<F::Error>> {
    if !SLOT_LEN.is_multiple_of(F::WRITE_SIZE) || !(offset as usize).is_multiple_of(F::ERASE_SIZE) {
        return Err(FlashVaultError::Misaligned);
    }
    if offset as usize + erase_span::<F>(slots) > flash.capacity() {
        return Err(FlashVaultError::OutOfBounds);
    }
    Ok(())
}

fn erase_span<F: NorFlash>(slots: usize) -> usize {
    (slots * SLOT_LEN).div_ceil(F::ERASE_SIZE) * F::ERASE_SIZE
}

fn slot_offset(base: u32, index: usize) -> u32 {
    base + (index * SLOT_LEN) as u32
}

fn read_records<F: NorFlash, const SLOTS: usize>(
    flash: &mut F,
    base: u32,
) -> Result<heapless::Vec<Record, SLOTS>, FlashVaultError<F::Error>> {
    let mut records = heapless::Vec::<Record, SLOTS>::new();
    for index in 0..SLOTS {
        if let Some(record) = read_slot(flash, base, index)? {
            records
                .push(record)
                .map_err(|_| FlashVaultError::StoreFull)?;
        }
    }
    Ok(records)
}

fn read_slot<F: NorFlash>(
    flash: &mut F,
    base: u32,
    index: usize,
) -> Result<Option<Record>, FlashVaultError<F::Error>> {
    let mut buffer = Zeroizing::new([0u8; SLOT_LEN]);
    flash
        .read(slot_offset(base, index), &mut buffer[..])
        .map_err(FlashVaultError::Flash)?;
    match buffer[STATE_OFFSET] {
        STATE_EMPTY => Ok(None),
        STATE_OCCUPIED => Ok(Some(parse_slot(&buffer)?)),
        _ => Err(FlashVaultError::Corrupt),
    }
}

fn parse_slot<E>(buffer: &[u8; SLOT_LEN]) -> Result<Record, FlashVaultError<E>> {
    let label_len = buffer[LABEL_LEN_OFFSET] as usize;
    if label_len == 0 || label_len > LABEL_CAP {
        return Err(FlashVaultError::Corrupt);
    }
    let label_bytes = &buffer[LABEL_OFFSET..LABEL_OFFSET + label_len];
    let label_str = core::str::from_utf8(label_bytes).map_err(|_| FlashVaultError::Corrupt)?;
    let label = IdentityLabel::new(label_str).map_err(|_| FlashVaultError::Corrupt)?;
    let mut secret = Zeroizing::new([0u8; SECRET_LEN]);
    secret.copy_from_slice(&buffer[SECRET_OFFSET..SECRET_OFFSET + SECRET_LEN]);
    Ok(Record { label, secret })
}

fn rewrite<F: NorFlash, const SLOTS: usize>(
    flash: &mut F,
    base: u32,
    records: &heapless::Vec<Record, SLOTS>,
) -> Result<(), FlashVaultError<F::Error>> {
    flash
        .erase(base, base + erase_span::<F>(SLOTS) as u32)
        .map_err(FlashVaultError::Flash)?;
    for (index, record) in records.iter().enumerate() {
        write_slot(flash, base, index, record)?;
    }
    Ok(())
}

fn write_slot<F: NorFlash>(
    flash: &mut F,
    base: u32,
    index: usize,
    record: &Record,
) -> Result<(), FlashVaultError<F::Error>> {
    let mut buffer = Zeroizing::new([STATE_EMPTY; SLOT_LEN]);
    buffer[STATE_OFFSET] = STATE_OCCUPIED;
    let label = record.label.as_str().as_bytes();
    buffer[LABEL_LEN_OFFSET] = label.len() as u8;
    buffer[LABEL_OFFSET..LABEL_OFFSET + label.len()].copy_from_slice(label);
    buffer[SECRET_OFFSET..SECRET_OFFSET + SECRET_LEN].copy_from_slice(&record.secret[..]);
    flash
        .write(slot_offset(base, index), &buffer[..])
        .map_err(FlashVaultError::Flash)
}

impl<E: core::fmt::Debug> core::fmt::Display for FlashVaultError<E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FlashVaultError::Flash(error) => write!(formatter, "flash error: {error:?}"),
            FlashVaultError::StoreFull => write!(formatter, "the flash identity region is full"),
            FlashVaultError::Misaligned => write!(
                formatter,
                "the flash region offset or slot size is not aligned to the device's write/erase units"
            ),
            FlashVaultError::OutOfBounds => {
                write!(formatter, "the flash identity region exceeds the device capacity")
            }
            FlashVaultError::Corrupt => write!(formatter, "a stored identity slot is malformed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vault::{load_or_generate, IdentityOrigin};
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind, ReadNorFlash};

    const FAKE_WRITE: usize = 4;
    const FAKE_ERASE: usize = 4096;

    struct FakeFlash<const CAP: usize> {
        bytes: [u8; CAP],
    }

    #[derive(Debug)]
    enum FakeError {
        Unaligned,
        OutOfBounds,
    }

    impl<const CAP: usize> FakeFlash<CAP> {
        fn new() -> Self {
            Self {
                bytes: [STATE_EMPTY; CAP],
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
        const READ_SIZE: usize = 1;

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
                *byte = STATE_EMPTY;
            }
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
            for (index, byte) in bytes.iter().enumerate() {
                self.bytes[start + index] &= byte;
            }
            Ok(())
        }
    }

    fn label(text: &str) -> IdentityLabel {
        IdentityLabel::new(text).unwrap()
    }

    fn secret(fill: u8) -> [u8; SECRET_LEN] {
        let mut bytes = [0u8; SECRET_LEN];
        bytes[..32].fill(fill);
        bytes[32..].fill(fill.wrapping_add(1));
        bytes
    }

    #[test]
    fn a_stored_secret_round_trips() {
        let mut vault = FlashVault::<_, 4>::new(FakeFlash::<8192>::new(), 0);
        let written = secret(0xA1);
        vault.store(&label("primary"), &written).unwrap();
        assert_eq!(*vault.load(&label("primary")).unwrap().unwrap(), written);
    }

    #[test]
    fn an_empty_region_is_a_clean_miss() {
        let vault = FlashVault::<_, 4>::new(FakeFlash::<8192>::new(), 0);
        assert!(vault.load(&label("primary")).unwrap().is_none());
    }

    #[test]
    fn the_identity_survives_a_reboot_as_a_fresh_vault_over_the_same_flash() {
        let written = secret(0x5E);
        let flash = {
            let mut vault = FlashVault::<_, 4>::new(FakeFlash::<8192>::new(), 0);
            vault.store(&label("primary"), &written).unwrap();
            vault.release()
        };
        let rebooted = FlashVault::<_, 4>::new(flash, 0);
        assert_eq!(*rebooted.load(&label("primary")).unwrap().unwrap(), written);
    }

    #[test]
    fn distinct_labels_keep_distinct_secrets() {
        let mut vault = FlashVault::<_, 4>::new(FakeFlash::<8192>::new(), 0);
        vault.store(&label("transport"), &secret(0x01)).unwrap();
        vault.store(&label("lxmf"), &secret(0x80)).unwrap();
        assert_eq!(
            *vault.load(&label("transport")).unwrap().unwrap(),
            secret(0x01)
        );
        assert_eq!(*vault.load(&label("lxmf")).unwrap().unwrap(), secret(0x80));
    }

    #[test]
    fn storing_the_same_label_again_overwrites_in_place() {
        let mut vault = FlashVault::<_, 4>::new(FakeFlash::<8192>::new(), 0);
        vault.store(&label("primary"), &secret(0x11)).unwrap();
        vault.store(&label("primary"), &secret(0x22)).unwrap();
        assert_eq!(
            *vault.load(&label("primary")).unwrap().unwrap(),
            secret(0x22)
        );
    }

    #[test]
    fn a_full_region_refuses_a_new_label() {
        let mut vault = FlashVault::<_, 2>::new(FakeFlash::<8192>::new(), 0);
        vault.store(&label("a"), &secret(0x11)).unwrap();
        vault.store(&label("b"), &secret(0x22)).unwrap();
        match vault.store(&label("c"), &secret(0x33)) {
            Err(FlashVaultError::StoreFull) => {}
            other => panic!("expected StoreFull, got {other:?}"),
        }
        assert!(vault.load(&label("a")).unwrap().is_some());
        assert!(vault.load(&label("b")).unwrap().is_some());
    }

    #[test]
    fn remove_reports_presence_then_absence_and_frees_the_slot() {
        let mut vault = FlashVault::<_, 2>::new(FakeFlash::<8192>::new(), 0);
        vault.store(&label("a"), &secret(0x11)).unwrap();
        vault.store(&label("b"), &secret(0x22)).unwrap();
        assert_eq!(vault.remove(&label("a")).unwrap(), Removal::Removed);
        assert_eq!(vault.remove(&label("a")).unwrap(), Removal::NothingStored);
        assert!(vault.load(&label("a")).unwrap().is_none());
        vault.store(&label("c"), &secret(0x33)).unwrap();
        assert_eq!(*vault.load(&label("c")).unwrap().unwrap(), secret(0x33));
    }

    #[test]
    fn a_region_owned_at_a_sector_offset_works() {
        let mut vault = FlashVault::<_, 2>::new(FakeFlash::<8192>::new(), FAKE_ERASE as u32);
        vault.store(&label("primary"), &secret(0x44)).unwrap();
        assert_eq!(
            *vault.load(&label("primary")).unwrap().unwrap(),
            secret(0x44)
        );
    }

    #[test]
    fn a_misaligned_offset_is_refused() {
        let vault = FlashVault::<_, 2>::new(FakeFlash::<8192>::new(), 7);
        match vault.load(&label("primary")) {
            Err(FlashVaultError::Misaligned) => {}
            other => panic!("expected Misaligned, got {other:?}"),
        }
    }

    #[test]
    fn a_region_past_the_device_capacity_is_refused() {
        let vault = FlashVault::<_, 2>::new(FakeFlash::<4096>::new(), FAKE_ERASE as u32);
        match vault.load(&label("primary")) {
            Err(FlashVaultError::OutOfBounds) => {}
            other => panic!("expected OutOfBounds, got {other:?}"),
        }
    }

    #[test]
    fn load_or_generate_mints_once_then_persists_across_a_reboot() {
        let fill = |bytes: &mut [u8]| {
            for (offset, byte) in bytes.iter_mut().enumerate() {
                *byte = 0x40u8.wrapping_add(offset as u8);
            }
        };
        let (minted, flash) = {
            let mut vault = FlashVault::<_, 2>::new(FakeFlash::<8192>::new(), 0);
            let (minted, origin) = load_or_generate(&mut vault, &label("primary"), fill).unwrap();
            assert_eq!(origin, IdentityOrigin::Generated);
            (minted, vault.release())
        };
        let mut rebooted = FlashVault::<_, 2>::new(flash, 0);
        let (reloaded, origin) = load_or_generate(&mut rebooted, &label("primary"), fill).unwrap();
        assert_eq!(origin, IdentityOrigin::Loaded);
        assert_eq!(*minted, *reloaded);
    }

    #[test]
    fn a_corrupt_occupied_slot_surfaces_rather_than_misreading() {
        let mut flash = FakeFlash::<8192>::new();
        flash.bytes[STATE_OFFSET] = STATE_OCCUPIED;
        flash.bytes[LABEL_LEN_OFFSET] = 0;
        let vault = FlashVault::<_, 2>::new(flash, 0);
        match vault.load(&label("primary")) {
            Err(FlashVaultError::Corrupt) => {}
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }
}
