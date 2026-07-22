use embedded_storage::nor_flash::NorFlash;
use personal_rns::identity::vault::{
    FlashVault, FlashVaultError, IdentityLabel, IdentityLabelError, IdentityVault,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::{
    decode_persisted_ble_identity, encode_persisted_ble_identity, BleIdentity,
    PersistedBleIdentityError, BLE_IDENTITY_LEN, PERSISTED_BLE_IDENTITY_LEN,
};

use crate::{
    HopspotNodeIdentity, IdentityBootstrap, IdentityStorageName, BLE_IDENTITY_STORAGE,
    NODE_IDENTITY_STORAGE,
};

#[derive(Debug)]
pub enum FlashIdentityError<E> {
    StorageName(IdentityLabelError),
    Vault(FlashVaultError<E>),
    WrongRecordKind,
    BleRecordLength(usize),
    EmptyBleRecord,
    InvalidBleRecord(PersistedBleIdentityError),
    Verification,
}

pub fn bootstrap_flash_node_identity<F: NorFlash, const SLOTS: usize>(
    vault: &mut FlashVault<F, SLOTS>,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<HopspotNodeIdentity, FlashIdentityError<F::Error>> {
    let label = match storage_label(NODE_IDENTITY_STORAGE) {
        Ok(label) => label,
        Err(error) => {
            return IdentityBootstrap::ephemeral(generate_node_identity(fill_entropy), error);
        }
    };
    match vault.load(&label) {
        Ok(Some(secret)) => IdentityBootstrap::loaded(HopspotNodeIdentity::new(secret)),
        Ok(None) => match vault.stored_blob_len(&label) {
            Ok(Some(_)) => recover_node_identity(
                vault,
                &label,
                fill_entropy,
                FlashIdentityError::WrongRecordKind,
            ),
            Ok(None) => create_node_identity(vault, &label, fill_entropy),
            Err(error) if recoverable(&error) => recover_node_identity(
                vault,
                &label,
                fill_entropy,
                FlashIdentityError::Vault(error),
            ),
            Err(error) => IdentityBootstrap::ephemeral(
                generate_node_identity(fill_entropy),
                FlashIdentityError::Vault(error),
            ),
        },
        Err(error) if recoverable(&error) => recover_node_identity(
            vault,
            &label,
            fill_entropy,
            FlashIdentityError::Vault(error),
        ),
        Err(error) => IdentityBootstrap::ephemeral(
            generate_node_identity(fill_entropy),
            FlashIdentityError::Vault(error),
        ),
    }
}

pub fn bootstrap_flash_ble_identity<F: NorFlash, const SLOTS: usize>(
    vault: &mut FlashVault<F, SLOTS>,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<BleIdentity, FlashIdentityError<F::Error>> {
    let label = match storage_label(BLE_IDENTITY_STORAGE) {
        Ok(label) => label,
        Err(error) => {
            return IdentityBootstrap::ephemeral(generate_ble_identity(fill_entropy), error);
        }
    };
    match vault.load(&label) {
        Ok(Some(_)) => recover_ble_identity(
            vault,
            &label,
            fill_entropy,
            FlashIdentityError::WrongRecordKind,
        ),
        Ok(None) => load_ble_blob(vault, &label, fill_entropy),
        Err(error) if recoverable(&error) => recover_ble_identity(
            vault,
            &label,
            fill_entropy,
            FlashIdentityError::Vault(error),
        ),
        Err(error) => IdentityBootstrap::ephemeral(
            generate_ble_identity(fill_entropy),
            FlashIdentityError::Vault(error),
        ),
    }
}

fn load_ble_blob<F: NorFlash, const SLOTS: usize>(
    vault: &mut FlashVault<F, SLOTS>,
    label: &IdentityLabel,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<BleIdentity, FlashIdentityError<F::Error>> {
    match vault.stored_blob_len(label) {
        Ok(None) => create_ble_identity(vault, label, fill_entropy),
        Ok(Some(len)) if len != PERSISTED_BLE_IDENTITY_LEN => recover_ble_identity(
            vault,
            label,
            fill_entropy,
            FlashIdentityError::BleRecordLength(len),
        ),
        Ok(Some(_)) => {
            let mut record = Zeroizing::new([0u8; PERSISTED_BLE_IDENTITY_LEN]);
            match vault.load_blob(label, &mut record[..]) {
                Ok(Some(_)) => match decode_persisted_ble_identity(&record) {
                    Ok(Some(identity)) => IdentityBootstrap::loaded(identity),
                    Ok(None) => recover_ble_identity(
                        vault,
                        label,
                        fill_entropy,
                        FlashIdentityError::EmptyBleRecord,
                    ),
                    Err(error) => recover_ble_identity(
                        vault,
                        label,
                        fill_entropy,
                        FlashIdentityError::InvalidBleRecord(error),
                    ),
                },
                Ok(None) => recover_ble_identity(
                    vault,
                    label,
                    fill_entropy,
                    FlashIdentityError::WrongRecordKind,
                ),
                Err(error) if recoverable(&error) => recover_ble_identity(
                    vault,
                    label,
                    fill_entropy,
                    FlashIdentityError::Vault(error),
                ),
                Err(error) => IdentityBootstrap::ephemeral(
                    generate_ble_identity(fill_entropy),
                    FlashIdentityError::Vault(error),
                ),
            }
        }
        Err(error) if recoverable(&error) => {
            recover_ble_identity(vault, label, fill_entropy, FlashIdentityError::Vault(error))
        }
        Err(error) => IdentityBootstrap::ephemeral(
            generate_ble_identity(fill_entropy),
            FlashIdentityError::Vault(error),
        ),
    }
}

fn create_node_identity<F: NorFlash, const SLOTS: usize>(
    vault: &mut FlashVault<F, SLOTS>,
    label: &IdentityLabel,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<HopspotNodeIdentity, FlashIdentityError<F::Error>> {
    let identity = generate_node_identity(fill_entropy);
    match vault.store(label, identity.secret()) {
        Ok(()) => match verify_node_identity(vault, label, &identity) {
            Ok(()) => IdentityBootstrap::created(identity),
            Err(error) if recovery_allowed(&error) => {
                recover_node_identity(vault, label, fill_entropy, error)
            }
            Err(error) => IdentityBootstrap::ephemeral(identity, error),
        },
        Err(error) if recoverable(&error) => {
            recover_node_identity(vault, label, fill_entropy, FlashIdentityError::Vault(error))
        }
        Err(error) => IdentityBootstrap::ephemeral(identity, FlashIdentityError::Vault(error)),
    }
}

fn recover_node_identity<F: NorFlash, const SLOTS: usize>(
    vault: &mut FlashVault<F, SLOTS>,
    label: &IdentityLabel,
    fill_entropy: &mut impl FnMut(&mut [u8]),
    cause: FlashIdentityError<F::Error>,
) -> IdentityBootstrap<HopspotNodeIdentity, FlashIdentityError<F::Error>> {
    if let Err(error) = vault.erase_all() {
        return IdentityBootstrap::ephemeral(
            generate_node_identity(fill_entropy),
            FlashIdentityError::Vault(error),
        );
    }
    let identity = generate_node_identity(fill_entropy);
    match vault.store(label, identity.secret()) {
        Ok(()) => match verify_node_identity(vault, label, &identity) {
            Ok(()) => IdentityBootstrap::recovered(identity, cause),
            Err(error) => IdentityBootstrap::ephemeral(identity, error),
        },
        Err(error) => IdentityBootstrap::ephemeral(identity, FlashIdentityError::Vault(error)),
    }
}

fn create_ble_identity<F: NorFlash, const SLOTS: usize>(
    vault: &mut FlashVault<F, SLOTS>,
    label: &IdentityLabel,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<BleIdentity, FlashIdentityError<F::Error>> {
    let identity = generate_ble_identity(fill_entropy);
    let record = Zeroizing::new(encode_persisted_ble_identity(identity));
    match vault.store_blob(label, &record[..]) {
        Ok(()) => match verify_ble_identity(vault, label, identity) {
            Ok(()) => IdentityBootstrap::created(identity),
            Err(error) if recovery_allowed(&error) => {
                recover_ble_identity(vault, label, fill_entropy, error)
            }
            Err(error) => IdentityBootstrap::ephemeral(identity, error),
        },
        Err(error) if recoverable(&error) => {
            recover_ble_identity(vault, label, fill_entropy, FlashIdentityError::Vault(error))
        }
        Err(error) => IdentityBootstrap::ephemeral(identity, FlashIdentityError::Vault(error)),
    }
}

fn recover_ble_identity<F: NorFlash, const SLOTS: usize>(
    vault: &mut FlashVault<F, SLOTS>,
    label: &IdentityLabel,
    fill_entropy: &mut impl FnMut(&mut [u8]),
    cause: FlashIdentityError<F::Error>,
) -> IdentityBootstrap<BleIdentity, FlashIdentityError<F::Error>> {
    if let Err(error) = vault.erase_all() {
        return IdentityBootstrap::ephemeral(
            generate_ble_identity(fill_entropy),
            FlashIdentityError::Vault(error),
        );
    }
    let identity = generate_ble_identity(fill_entropy);
    let record = Zeroizing::new(encode_persisted_ble_identity(identity));
    match vault.store_blob(label, &record[..]) {
        Ok(()) => match verify_ble_identity(vault, label, identity) {
            Ok(()) => IdentityBootstrap::recovered(identity, cause),
            Err(error) => IdentityBootstrap::ephemeral(identity, error),
        },
        Err(error) => IdentityBootstrap::ephemeral(identity, FlashIdentityError::Vault(error)),
    }
}

fn storage_label<E>(name: IdentityStorageName) -> Result<IdentityLabel, FlashIdentityError<E>> {
    IdentityLabel::new(name.as_str()).map_err(FlashIdentityError::StorageName)
}

fn recoverable<E>(error: &FlashVaultError<E>) -> bool {
    matches!(error, FlashVaultError::StoreFull | FlashVaultError::Corrupt)
}

fn recovery_allowed<E>(error: &FlashIdentityError<E>) -> bool {
    match error {
        FlashIdentityError::Vault(error) => {
            matches!(error, FlashVaultError::StoreFull | FlashVaultError::Corrupt)
        }
        FlashIdentityError::WrongRecordKind
        | FlashIdentityError::BleRecordLength(_)
        | FlashIdentityError::EmptyBleRecord
        | FlashIdentityError::InvalidBleRecord(_)
        | FlashIdentityError::Verification => true,
        FlashIdentityError::StorageName(_) => false,
    }
}

fn verify_node_identity<F: NorFlash, const SLOTS: usize>(
    vault: &FlashVault<F, SLOTS>,
    label: &IdentityLabel,
    expected: &HopspotNodeIdentity,
) -> Result<(), FlashIdentityError<F::Error>> {
    match vault.load(label).map_err(FlashIdentityError::Vault)? {
        Some(stored) if &stored[..] == expected.secret() => Ok(()),
        Some(_) | None => Err(FlashIdentityError::Verification),
    }
}

fn verify_ble_identity<F: NorFlash, const SLOTS: usize>(
    vault: &FlashVault<F, SLOTS>,
    label: &IdentityLabel,
    expected: BleIdentity,
) -> Result<(), FlashIdentityError<F::Error>> {
    let len = vault
        .stored_blob_len(label)
        .map_err(FlashIdentityError::Vault)?
        .ok_or(FlashIdentityError::Verification)?;
    if len != PERSISTED_BLE_IDENTITY_LEN {
        return Err(FlashIdentityError::BleRecordLength(len));
    }
    let mut record = Zeroizing::new([0u8; PERSISTED_BLE_IDENTITY_LEN]);
    vault
        .load_blob(label, &mut record[..])
        .map_err(FlashIdentityError::Vault)?
        .ok_or(FlashIdentityError::Verification)?;
    match decode_persisted_ble_identity(&record) {
        Ok(Some(stored)) if stored == expected => Ok(()),
        Ok(Some(_)) | Ok(None) => Err(FlashIdentityError::Verification),
        Err(error) => Err(FlashIdentityError::InvalidBleRecord(error)),
    }
}

fn generate_node_identity(fill_entropy: &mut impl FnMut(&mut [u8])) -> HopspotNodeIdentity {
    let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    fill_entropy(&mut secret[..]);
    HopspotNodeIdentity::new(secret)
}

fn generate_ble_identity(fill_entropy: &mut impl FnMut(&mut [u8])) -> BleIdentity {
    let mut bytes = [0u8; BLE_IDENTITY_LEN];
    fill_entropy(&mut bytes);
    BleIdentity::new(bytes)
}

impl<E: core::fmt::Debug> core::fmt::Display for FlashIdentityError<E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::StorageName(error) => {
                write!(formatter, "invalid identity storage name: {error:?}")
            }
            Self::Vault(error) => error.fmt(formatter),
            Self::WrongRecordKind => {
                formatter.write_str("identity storage holds the wrong record kind")
            }
            Self::BleRecordLength(len) => write!(
                formatter,
                "BLE identity record holds {len} bytes, expected {PERSISTED_BLE_IDENTITY_LEN}"
            ),
            Self::EmptyBleRecord => formatter.write_str("BLE identity record is empty"),
            Self::InvalidBleRecord(error) => error.fmt(formatter),
            Self::Verification => formatter.write_str("persisted identity verification failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind, ReadNorFlash};

    const ERASE_LEN: usize = 4096;

    struct FakeFlash {
        bytes: [u8; ERASE_LEN],
        fail_reads: bool,
        fail_writes: bool,
    }

    #[derive(Debug)]
    enum FakeError {
        Read,
        Write,
        Bounds,
        Alignment,
    }

    impl FakeFlash {
        fn new() -> Self {
            Self {
                bytes: [u8::MAX; ERASE_LEN],
                fail_reads: false,
                fail_writes: false,
            }
        }
    }

    impl NorFlashError for FakeError {
        fn kind(&self) -> NorFlashErrorKind {
            match self {
                Self::Read | Self::Write => NorFlashErrorKind::Other,
                Self::Bounds => NorFlashErrorKind::OutOfBounds,
                Self::Alignment => NorFlashErrorKind::NotAligned,
            }
        }
    }

    impl ErrorType for FakeFlash {
        type Error = FakeError;
    }

    impl ReadNorFlash for FakeFlash {
        const READ_SIZE: usize = 1;

        fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            if self.fail_reads {
                return Err(FakeError::Read);
            }
            let start = offset as usize;
            let end = start + bytes.len();
            if end > self.bytes.len() {
                return Err(FakeError::Bounds);
            }
            bytes.copy_from_slice(&self.bytes[start..end]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.bytes.len()
        }
    }

    impl NorFlash for FakeFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = ERASE_LEN;

        fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            if from != 0 || to as usize != ERASE_LEN {
                return Err(FakeError::Alignment);
            }
            self.bytes.fill(u8::MAX);
            Ok(())
        }

        fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            if self.fail_writes {
                return Err(FakeError::Write);
            }
            let start = offset as usize;
            let end = start + bytes.len();
            if !start.is_multiple_of(Self::WRITE_SIZE)
                || !bytes.len().is_multiple_of(Self::WRITE_SIZE)
                || end > self.bytes.len()
            {
                return Err(FakeError::Alignment);
            }
            for (stored, incoming) in self.bytes[start..end].iter_mut().zip(bytes) {
                *stored &= *incoming;
            }
            Ok(())
        }
    }

    fn fill(byte: u8) -> impl FnMut(&mut [u8]) {
        move |bytes| bytes.fill(byte)
    }

    #[test]
    fn node_identity_is_created_then_loaded_from_the_same_vault() {
        let flash = {
            let mut vault = FlashVault::<_, 1>::new(FakeFlash::new(), 0);
            let created = bootstrap_flash_node_identity(&mut vault, &mut fill(0x31));
            assert!(matches!(
                created.persistence(),
                crate::IdentityPersistence::Created
            ));
            vault.release()
        };
        let mut vault = FlashVault::<_, 1>::new(flash, 0);
        let loaded = bootstrap_flash_node_identity(&mut vault, &mut fill(0x92));
        assert!(matches!(
            loaded.persistence(),
            crate::IdentityPersistence::Loaded
        ));
        assert_eq!(
            loaded.into_identity().secret(),
            &[0x31; IDENTITY_SECRET_KEY_LEN]
        );
    }

    #[test]
    fn corrupt_node_identity_is_reseeded() {
        let mut flash = {
            let mut vault = FlashVault::<_, 1>::new(FakeFlash::new(), 0);
            let _ = bootstrap_flash_node_identity(&mut vault, &mut fill(0x41));
            vault.release()
        };
        flash.bytes[100] ^= 0x01;
        let mut vault = FlashVault::<_, 1>::new(flash, 0);
        let recovered = bootstrap_flash_node_identity(&mut vault, &mut fill(0x42));
        assert!(matches!(
            recovered.persistence(),
            crate::IdentityPersistence::Recovered(_)
        ));
        assert_eq!(
            recovered.into_identity().secret(),
            &[0x42; IDENTITY_SECRET_KEY_LEN]
        );
    }

    #[test]
    fn flash_read_failure_returns_an_ephemeral_identity() {
        let mut flash = FakeFlash::new();
        flash.fail_reads = true;
        let mut vault = FlashVault::<_, 1>::new(flash, 0);
        let ephemeral = bootstrap_flash_node_identity(&mut vault, &mut fill(0x53));
        assert!(matches!(
            ephemeral.persistence(),
            crate::IdentityPersistence::Ephemeral(_)
        ));
        assert_eq!(
            ephemeral.into_identity().secret(),
            &[0x53; IDENTITY_SECRET_KEY_LEN]
        );
    }

    #[test]
    fn flash_write_failure_returns_an_ephemeral_identity() {
        let mut flash = FakeFlash::new();
        flash.fail_writes = true;
        let mut vault = FlashVault::<_, 1>::new(flash, 0);
        let ephemeral = bootstrap_flash_node_identity(&mut vault, &mut fill(0x54));
        assert!(matches!(
            ephemeral.persistence(),
            crate::IdentityPersistence::Ephemeral(_)
        ));
        assert_eq!(
            ephemeral.into_identity().secret(),
            &[0x54; IDENTITY_SECRET_KEY_LEN]
        );
    }

    #[test]
    fn ble_identity_uses_the_protected_blob_record() {
        let flash = {
            let mut vault = FlashVault::<_, 1>::new(FakeFlash::new(), 0);
            let created = bootstrap_flash_ble_identity(&mut vault, &mut fill(0x64));
            assert!(matches!(
                created.persistence(),
                crate::IdentityPersistence::Created
            ));
            vault.release()
        };
        let mut vault = FlashVault::<_, 1>::new(flash, 0);
        let loaded = bootstrap_flash_ble_identity(&mut vault, &mut fill(0x65));
        assert!(matches!(
            loaded.persistence(),
            crate::IdentityPersistence::Loaded
        ));
        assert_eq!(
            loaded.into_identity(),
            BleIdentity::new([0x64; BLE_IDENTITY_LEN])
        );
    }

    #[test]
    fn corrupt_ble_identity_is_reseeded() {
        let mut flash = {
            let mut vault = FlashVault::<_, 1>::new(FakeFlash::new(), 0);
            let _ = bootstrap_flash_ble_identity(&mut vault, &mut fill(0x71));
            vault.release()
        };
        flash.bytes[100] ^= 0x01;
        let mut vault = FlashVault::<_, 1>::new(flash, 0);
        let recovered = bootstrap_flash_ble_identity(&mut vault, &mut fill(0x72));
        assert!(matches!(
            recovered.persistence(),
            crate::IdentityPersistence::Recovered(_)
        ));
        assert_eq!(
            recovered.into_identity(),
            BleIdentity::new([0x72; BLE_IDENTITY_LEN])
        );
    }
}
