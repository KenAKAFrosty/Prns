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
const COMMIT_LEN: usize = 4;
const LABEL_LEN_OFFSET: usize = COMMIT_LEN;
const LABEL_OFFSET: usize = LABEL_LEN_OFFSET + 1;
const SECRET_OFFSET: usize = LABEL_OFFSET + LABEL_CAP;
const SECRET_INVERSE_OFFSET: usize = SECRET_OFFSET + SECRET_LEN;
const STATE_EMPTY: u8 = 0xFF;
const STATE_OCCUPIED: u8 = 0xA5;
const STATE_BLOB: u8 = 0x3C;
const BLOB_LEN_PREFIX_LEN: usize = 2;

/// A blob shares the identity slot's frame, spending the fixed secret region on a length-prefixed body instead.
pub const FLASH_VAULT_BLOB_CAP: usize = SLOT_LEN - SECRET_OFFSET - BLOB_LEN_PREFIX_LEN;

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
    BlobTooLong { blob_len: usize },
    BlobOutgrewBuffer { blob_len: usize, buffer_len: usize },
}

struct Record {
    label: IdentityLabel,
    payload: RecordPayload,
}

enum RecordPayload {
    Identity(IdentitySecretKey),
    Blob {
        len: usize,
        bytes: Zeroizing<[u8; FLASH_VAULT_BLOB_CAP]>,
    },
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

    pub fn erase_all(&mut self) -> Result<(), FlashVaultError<F::Error>> {
        let flash = self.flash.get_mut();
        validate::<F>(flash, self.offset, SLOTS)?;
        flash
            .erase(self.offset, self.offset + erase_span::<F>(SLOTS) as u32)
            .map_err(FlashVaultError::Flash)
    }
}

impl<F: NorFlash, const SLOTS: usize> IdentityVault for FlashVault<F, SLOTS> {
    type Error = FlashVaultError<F::Error>;

    fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
        let mut flash = self.flash.borrow_mut();
        validate::<F>(&flash, self.offset, SLOTS)?;
        for index in 0..SLOTS {
            let Some(record) = read_slot(&mut *flash, self.offset, index)? else {
                continue;
            };
            if &record.label == label {
                return Ok(match record.payload {
                    RecordPayload::Identity(secret) => Some(secret),
                    RecordPayload::Blob { .. } => None,
                });
            }
        }
        Ok(None)
    }

    fn store(
        &mut self,
        label: &IdentityLabel,
        secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<(), Self::Error> {
        self.store_payload(label, RecordPayload::Identity(Zeroizing::new(*secret)))
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

    fn stored_blob_len(&self, label: &IdentityLabel) -> Result<Option<usize>, Self::Error> {
        let mut flash = self.flash.borrow_mut();
        validate::<F>(&flash, self.offset, SLOTS)?;
        for index in 0..SLOTS {
            let Some(record) = read_slot(&mut *flash, self.offset, index)? else {
                continue;
            };
            if &record.label == label {
                return Ok(match record.payload {
                    RecordPayload::Blob { len, .. } => Some(len),
                    RecordPayload::Identity(_) => None,
                });
            }
        }
        Ok(None)
    }

    fn load_blob<'b>(
        &self,
        label: &IdentityLabel,
        buf: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Self::Error> {
        let mut flash = self.flash.borrow_mut();
        validate::<F>(&flash, self.offset, SLOTS)?;
        for index in 0..SLOTS {
            let Some(record) = read_slot(&mut *flash, self.offset, index)? else {
                continue;
            };
            if &record.label == label {
                let RecordPayload::Blob { len, bytes } = record.payload else {
                    return Ok(None);
                };
                if buf.len() < len {
                    return Err(FlashVaultError::BlobOutgrewBuffer {
                        blob_len: len,
                        buffer_len: buf.len(),
                    });
                }
                buf[..len].copy_from_slice(&bytes[..len]);
                return Ok(Some(&buf[..len]));
            }
        }
        Ok(None)
    }

    fn store_blob(&mut self, label: &IdentityLabel, blob: &[u8]) -> Result<(), Self::Error> {
        if blob.len() > FLASH_VAULT_BLOB_CAP {
            return Err(FlashVaultError::BlobTooLong {
                blob_len: blob.len(),
            });
        }
        let mut bytes = Zeroizing::new([0u8; FLASH_VAULT_BLOB_CAP]);
        bytes[..blob.len()].copy_from_slice(blob);
        self.store_payload(
            label,
            RecordPayload::Blob {
                len: blob.len(),
                bytes,
            },
        )
    }
}

impl<F: NorFlash, const SLOTS: usize> FlashVault<F, SLOTS> {
    fn store_payload(
        &mut self,
        label: &IdentityLabel,
        payload: RecordPayload,
    ) -> Result<(), FlashVaultError<F::Error>> {
        let flash = self.flash.get_mut();
        validate::<F>(flash, self.offset, SLOTS)?;
        for index in 0..SLOTS {
            let Some(record) = read_slot(flash, self.offset, index)? else {
                continue;
            };
            if &record.label == label {
                let mut records = read_records::<F, SLOTS>(flash, self.offset)?;
                let existing = records
                    .iter_mut()
                    .find(|record| &record.label == label)
                    .ok_or(FlashVaultError::Corrupt)?;
                existing.payload = payload;
                return rewrite::<F, SLOTS>(flash, self.offset, &records);
            }
        }
        let index =
            find_vacant_slot::<F, SLOTS>(flash, self.offset)?.ok_or(FlashVaultError::StoreFull)?;
        write_slot(
            flash,
            self.offset,
            index,
            &Record {
                label: label.clone(),
                payload,
            },
        )
    }
}

fn validate<F: NorFlash>(
    flash: &F,
    offset: u32,
    slots: usize,
) -> Result<(), FlashVaultError<F::Error>> {
    if !SLOT_LEN.is_multiple_of(F::READ_SIZE)
        || !SLOT_LEN.is_multiple_of(F::WRITE_SIZE)
        || F::WRITE_SIZE > COMMIT_LEN
        || !COMMIT_LEN.is_multiple_of(F::WRITE_SIZE)
        || !(offset as usize).is_multiple_of(F::ERASE_SIZE)
    {
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
        STATE_OCCUPIED | STATE_BLOB => Ok(Some(parse_slot(&buffer)?)),
        _ => Err(FlashVaultError::Corrupt),
    }
}

fn find_vacant_slot<F: NorFlash, const SLOTS: usize>(
    flash: &mut F,
    base: u32,
) -> Result<Option<usize>, FlashVaultError<F::Error>> {
    for index in 0..SLOTS {
        let mut buffer = Zeroizing::new([0u8; SLOT_LEN]);
        flash
            .read(slot_offset(base, index), &mut buffer[..])
            .map_err(FlashVaultError::Flash)?;
        if buffer.iter().all(|byte| *byte == STATE_EMPTY) {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

fn parse_slot<E>(buffer: &[u8; SLOT_LEN]) -> Result<Record, FlashVaultError<E>> {
    let label_len = buffer[LABEL_LEN_OFFSET] as usize;
    if label_len == 0 || label_len > LABEL_CAP {
        return Err(FlashVaultError::Corrupt);
    }
    let label_bytes = &buffer[LABEL_OFFSET..LABEL_OFFSET + label_len];
    let label_str = core::str::from_utf8(label_bytes).map_err(|_| FlashVaultError::Corrupt)?;
    let label = IdentityLabel::new(label_str).map_err(|_| FlashVaultError::Corrupt)?;
    let payload = match buffer[STATE_OFFSET] {
        STATE_OCCUPIED => {
            let mut secret = Zeroizing::new([0u8; SECRET_LEN]);
            secret.copy_from_slice(&buffer[SECRET_OFFSET..SECRET_OFFSET + SECRET_LEN]);
            if secret
                .iter()
                .zip(&buffer[SECRET_INVERSE_OFFSET..SECRET_INVERSE_OFFSET + SECRET_LEN])
                .any(|(byte, inverse)| *byte != !*inverse)
            {
                return Err(FlashVaultError::Corrupt);
            }
            RecordPayload::Identity(secret)
        }
        STATE_BLOB => {
            let len = u16::from_le_bytes([buffer[SECRET_OFFSET], buffer[SECRET_OFFSET + 1]]);
            let len = len as usize;
            if len > FLASH_VAULT_BLOB_CAP {
                return Err(FlashVaultError::Corrupt);
            }
            let mut bytes = Zeroizing::new([0u8; FLASH_VAULT_BLOB_CAP]);
            let body_at = SECRET_OFFSET + BLOB_LEN_PREFIX_LEN;
            bytes[..len].copy_from_slice(&buffer[body_at..body_at + len]);
            RecordPayload::Blob { len, bytes }
        }
        _ => return Err(FlashVaultError::Corrupt),
    };
    Ok(Record { label, payload })
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
    let label = record.label.as_str().as_bytes();
    buffer[LABEL_LEN_OFFSET] = label.len() as u8;
    buffer[LABEL_OFFSET..LABEL_OFFSET + label.len()].copy_from_slice(label);
    let state = match &record.payload {
        RecordPayload::Identity(secret) => {
            buffer[SECRET_OFFSET..SECRET_OFFSET + SECRET_LEN].copy_from_slice(&secret[..]);
            for (inverse, byte) in buffer[SECRET_INVERSE_OFFSET..SECRET_INVERSE_OFFSET + SECRET_LEN]
                .iter_mut()
                .zip(secret.iter())
            {
                *inverse = !*byte;
            }
            STATE_OCCUPIED
        }
        RecordPayload::Blob { len, bytes } => {
            buffer[SECRET_OFFSET..SECRET_OFFSET + BLOB_LEN_PREFIX_LEN]
                .copy_from_slice(&(*len as u16).to_le_bytes());
            let body_at = SECRET_OFFSET + BLOB_LEN_PREFIX_LEN;
            buffer[body_at..body_at + len].copy_from_slice(&bytes[..*len]);
            STATE_BLOB
        }
    };
    let at = slot_offset(base, index);
    flash
        .write(at + COMMIT_LEN as u32, &buffer[COMMIT_LEN..])
        .map_err(FlashVaultError::Flash)?;
    buffer[STATE_OFFSET] = state;
    flash
        .write(at, &buffer[..COMMIT_LEN])
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
            FlashVaultError::BlobTooLong { blob_len } => write!(
                formatter,
                "a blob of {blob_len} bytes exceeds the {FLASH_VAULT_BLOB_CAP}-byte slot capacity"
            ),
            FlashVaultError::BlobOutgrewBuffer {
                blob_len,
                buffer_len,
            } => write!(
                formatter,
                "stored blob holds {blob_len} bytes, the buffer holds {buffer_len}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vault::{load_or_generate, IdentityOrigin};
    use crate::remote_control::{
        RemoteControlNodeIdentityBootstrap, REMOTE_CONTROL_IDENTITY_VAULT_SLOTS,
    };
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind, ReadNorFlash};

    const FAKE_WRITE: usize = 4;
    const FAKE_ERASE: usize = 4096;

    struct FakeFlash<const CAP: usize, const READ: usize = 1> {
        bytes: [u8; CAP],
        erase_count: usize,
        write_count: usize,
        fail_write_at: Option<usize>,
    }

    #[derive(Debug)]
    enum FakeError {
        Unaligned,
        OutOfBounds,
        Interrupted,
    }

    impl<const CAP: usize, const READ: usize> FakeFlash<CAP, READ> {
        fn new() -> Self {
            Self {
                bytes: [STATE_EMPTY; CAP],
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

    impl<const CAP: usize, const READ: usize> ErrorType for FakeFlash<CAP, READ> {
        type Error = FakeError;
    }

    impl<const CAP: usize, const READ: usize> ReadNorFlash for FakeFlash<CAP, READ> {
        const READ_SIZE: usize = READ;

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

    impl<const CAP: usize, const READ: usize> NorFlash for FakeFlash<CAP, READ> {
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
        assert_eq!(vault.release().erase_count, 0);
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
    fn a_read_granule_that_does_not_divide_a_slot_is_refused() {
        let vault = FlashVault::<_, 2>::new(FakeFlash::<8192, 96>::new(), 0);
        match vault.load(&label("primary")) {
            Err(FlashVaultError::Misaligned) => {}
            other => panic!("expected Misaligned, got {other:?}"),
        }
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
    fn remote_control_identity_pair_fits_one_erase_page_and_survives_reboot() {
        let mut fill = 0x31;
        let (flash, identities) = {
            let mut vault = FlashVault::<_, REMOTE_CONTROL_IDENTITY_VAULT_SLOTS>::new(
                FakeFlash::<FAKE_ERASE>::new(),
                0,
            );
            let bootstrap =
                RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, |bytes| {
                    bytes.fill(fill);
                    fill = fill.wrapping_add(0x11);
                    Ok::<(), core::convert::Infallible>(())
                })
                .unwrap();
            assert_eq!(
                (
                    bootstrap.origins().controller(),
                    bootstrap.origins().target(),
                ),
                (IdentityOrigin::Generated, IdentityOrigin::Generated),
            );
            (vault.release(), bootstrap.secrets().identities())
        };

        let mut vault = FlashVault::<_, REMOTE_CONTROL_IDENTITY_VAULT_SLOTS>::new(flash, 0);
        let mut entropy_calls = 0;
        let bootstrap =
            RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, |_bytes| {
                entropy_calls += 1;
                Ok::<(), core::convert::Infallible>(())
            })
            .unwrap();

        assert_eq!(entropy_calls, 0);
        assert_eq!(bootstrap.secrets().identities(), identities);
        assert_eq!(
            (
                bootstrap.origins().controller(),
                bootstrap.origins().target(),
            ),
            (IdentityOrigin::Loaded, IdentityOrigin::Loaded),
        );
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

    #[test]
    fn a_secret_with_a_corrupt_inverse_surfaces() {
        let flash = {
            let mut vault = FlashVault::<_, 2>::new(FakeFlash::<8192>::new(), 0);
            vault.store(&label("primary"), &secret(0x66)).unwrap();
            let mut flash = vault.release();
            flash.bytes[SECRET_INVERSE_OFFSET + 3] ^= 0x01;
            flash
        };
        let vault = FlashVault::<_, 2>::new(flash, 0);
        match vault.load(&label("primary")) {
            Err(FlashVaultError::Corrupt) => {}
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn an_interrupted_commit_remains_uncommitted() {
        let mut flash = FakeFlash::<8192>::new();
        flash.fail_write_at = Some(2);
        let flash = {
            let mut vault = FlashVault::<_, 2>::new(flash, 0);
            match vault.store(&label("primary"), &secret(0x71)) {
                Err(FlashVaultError::Flash(FakeError::Interrupted)) => {}
                other => panic!("expected interrupted flash write, got {other:?}"),
            }
            vault.release()
        };
        let mut rebooted = FlashVault::<_, 2>::new(flash, 0);
        assert!(rebooted.load(&label("primary")).unwrap().is_none());
        rebooted.store(&label("primary"), &secret(0x72)).unwrap();
        assert_eq!(
            *rebooted.load(&label("primary")).unwrap().unwrap(),
            secret(0x72)
        );
    }

    #[test]
    fn erase_all_clears_only_the_owned_region() {
        let mut flash = FakeFlash::<8192>::new();
        flash.bytes[FAKE_ERASE] = 0x37;
        let flash = {
            let mut vault = FlashVault::<_, 2>::new(flash, 0);
            vault.store(&label("primary"), &secret(0x81)).unwrap();
            vault.erase_all().unwrap();
            vault.release()
        };
        assert!(flash.bytes[..FAKE_ERASE]
            .iter()
            .all(|byte| *byte == STATE_EMPTY));
        assert_eq!(flash.bytes[FAKE_ERASE], 0x37);
    }

    #[test]
    fn a_blob_round_trips_beside_an_identity_and_survives_a_reboot() {
        let blob = [0x7Bu8; 113];
        let flash = {
            let mut vault = FlashVault::<_, 2>::new(FakeFlash::<8192>::new(), 0);
            vault.store(&label("primary"), &[0x42; SECRET_LEN]).unwrap();
            vault.store_blob(&label("ratchets.a1b2"), &blob).unwrap();
            vault.release()
        };
        let vault = FlashVault::<_, 2>::new(flash, 0);
        assert_eq!(
            vault.stored_blob_len(&label("ratchets.a1b2")).unwrap(),
            Some(blob.len()),
        );
        let mut buf = [0u8; FLASH_VAULT_BLOB_CAP];
        assert_eq!(
            vault.load_blob(&label("ratchets.a1b2"), &mut buf).unwrap(),
            Some(&blob[..]),
        );
        assert_eq!(
            *vault.load(&label("primary")).unwrap().unwrap(),
            [0x42; SECRET_LEN]
        );
        assert_eq!(vault.load_blob(&label("primary"), &mut buf).unwrap(), None);
        assert!(vault.load(&label("ratchets.a1b2")).unwrap().is_none());
    }

    #[test]
    fn a_blob_past_the_slot_capacity_is_refused_by_name() {
        let mut vault = FlashVault::<_, 2>::new(FakeFlash::<8192>::new(), 0);
        let oversized = [0u8; FLASH_VAULT_BLOB_CAP + 1];
        match vault.store_blob(&label("ratchets.a1b2"), &oversized) {
            Err(FlashVaultError::BlobTooLong { blob_len }) => {
                assert_eq!(blob_len, FLASH_VAULT_BLOB_CAP + 1);
            }
            other => panic!("expected BlobTooLong, got {other:?}"),
        }
    }
}
