use esp_hal::rng::Rng;
#[cfg(target_arch = "xtensa")]
use personal_hopspot_core::UiNotice;
use personal_hopspot_core::{
    HopspotNodeIdentity, IdentityBootstrap, IdentityPersistence, IdentityStorageName,
    BLE_IDENTITY_STORAGE, NODE_IDENTITY_STORAGE,
};
use personal_rns::identity::vault::{
    FlashVault, FlashVaultError, IdentityLabel, IdentityLabelError, IdentityVault,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::{
    decode_persisted_ble_identity, encode_persisted_ble_identity, BleIdentity,
    PersistedBleIdentityError, BLE_IDENTITY_LEN, PERSISTED_BLE_IDENTITY_LEN,
};

use crate::flash::{EspRomFlash, EspRomFlashError};

const FLASH_SECTOR_LEN: u32 = 0x1000;
const BLE_IDENTITY_FLASH_OFFSET: u32 = 0xC000;
const HOPSPOT_CONFIG_FLASH_OFFSET: u32 = 0xD000;
const NODE_IDENTITY_FLASH_OFFSET: u32 = 0xE000;
const IDENTITY_FLASH_END: usize = 0xF000;
const VAULT_SLOTS: usize = 1;

const _: () = assert!(BLE_IDENTITY_FLASH_OFFSET + FLASH_SECTOR_LEN == HOPSPOT_CONFIG_FLASH_OFFSET);
const _: () = assert!(HOPSPOT_CONFIG_FLASH_OFFSET + FLASH_SECTOR_LEN == NODE_IDENTITY_FLASH_OFFSET);
const _: () = assert!(NODE_IDENTITY_FLASH_OFFSET + FLASH_SECTOR_LEN == IDENTITY_FLASH_END as u32);

type Vault = FlashVault<EspRomFlash<IDENTITY_FLASH_END>, VAULT_SLOTS>;
type VaultError = FlashVaultError<EspRomFlashError>;

#[derive(Debug)]
pub enum EspIdentityError {
    StorageName(IdentityLabelError),
    Vault(VaultError),
    WrongRecordKind,
    BleRecordLength(usize),
    EmptyBleRecord,
    InvalidBleRecord(PersistedBleIdentityError),
    Verification,
}

pub fn log_persistence(identity: &str, persistence: &IdentityPersistence<EspIdentityError>) {
    match persistence {
        IdentityPersistence::Loaded => {}
        IdentityPersistence::Created => log::info!("{identity} identity created"),
        IdentityPersistence::Recovered(error) => {
            log::warn!("{identity} identity recovered after corruption: {error}")
        }
        IdentityPersistence::Ephemeral(error) => {
            log::error!("{identity} identity is ephemeral: {error}")
        }
    }
}

#[cfg(target_arch = "xtensa")]
pub fn startup_notice(
    node: &IdentityPersistence<EspIdentityError>,
    bluetooth: &IdentityPersistence<EspIdentityError>,
) -> Option<UiNotice> {
    if node.is_ephemeral() || bluetooth.is_ephemeral() {
        Some(UiNotice::IdentityUnstable)
    } else if node.is_recovered() || bluetooth.is_recovered() {
        Some(UiNotice::IdentityReset)
    } else {
        None
    }
}

pub fn bootstrap_node_identity() -> IdentityBootstrap<HopspotNodeIdentity, EspIdentityError> {
    let label = match storage_label(NODE_IDENTITY_STORAGE) {
        Ok(label) => label,
        Err(error) => {
            return IdentityBootstrap::ephemeral(generate_node_identity(), error);
        }
    };
    let mut vault = Vault::new(EspRomFlash::new(), NODE_IDENTITY_FLASH_OFFSET);
    match vault.load(&label) {
        Ok(Some(secret)) => IdentityBootstrap::loaded(HopspotNodeIdentity::new(secret)),
        Ok(None) => match vault.stored_blob_len(&label) {
            Ok(Some(_)) => {
                recover_node_identity(&mut vault, &label, EspIdentityError::WrongRecordKind)
            }
            Ok(None) => create_node_identity(&mut vault, &label),
            Err(error) if recoverable(&error) => {
                recover_node_identity(&mut vault, &label, EspIdentityError::Vault(error))
            }
            Err(error) => IdentityBootstrap::ephemeral(
                generate_node_identity(),
                EspIdentityError::Vault(error),
            ),
        },
        Err(error) if recoverable(&error) => {
            recover_node_identity(&mut vault, &label, EspIdentityError::Vault(error))
        }
        Err(error) => {
            IdentityBootstrap::ephemeral(generate_node_identity(), EspIdentityError::Vault(error))
        }
    }
}

pub fn bootstrap_ble_identity() -> IdentityBootstrap<BleIdentity, EspIdentityError> {
    let label = match storage_label(BLE_IDENTITY_STORAGE) {
        Ok(label) => label,
        Err(error) => {
            return IdentityBootstrap::ephemeral(generate_ble_identity(), error);
        }
    };
    let mut vault = Vault::new(EspRomFlash::new(), BLE_IDENTITY_FLASH_OFFSET);
    match vault.load(&label) {
        Ok(Some(_)) => recover_ble_identity(&mut vault, &label, EspIdentityError::WrongRecordKind),
        Ok(None) => load_ble_blob(&mut vault, &label),
        Err(error) if recoverable(&error) => {
            recover_ble_identity(&mut vault, &label, EspIdentityError::Vault(error))
        }
        Err(error) => {
            IdentityBootstrap::ephemeral(generate_ble_identity(), EspIdentityError::Vault(error))
        }
    }
}

fn load_ble_blob(
    vault: &mut Vault,
    label: &IdentityLabel,
) -> IdentityBootstrap<BleIdentity, EspIdentityError> {
    match vault.stored_blob_len(label) {
        Ok(None) => create_ble_identity(vault, label),
        Ok(Some(len)) if len != PERSISTED_BLE_IDENTITY_LEN => {
            recover_ble_identity(vault, label, EspIdentityError::BleRecordLength(len))
        }
        Ok(Some(_)) => {
            let mut record = Zeroizing::new([0u8; PERSISTED_BLE_IDENTITY_LEN]);
            match vault.load_blob(label, &mut record[..]) {
                Ok(Some(_)) => match decode_persisted_ble_identity(&record) {
                    Ok(Some(identity)) => IdentityBootstrap::loaded(identity),
                    Ok(None) => {
                        recover_ble_identity(vault, label, EspIdentityError::EmptyBleRecord)
                    }
                    Err(error) => recover_ble_identity(
                        vault,
                        label,
                        EspIdentityError::InvalidBleRecord(error),
                    ),
                },
                Ok(None) => recover_ble_identity(vault, label, EspIdentityError::WrongRecordKind),
                Err(error) if recoverable(&error) => {
                    recover_ble_identity(vault, label, EspIdentityError::Vault(error))
                }
                Err(error) => IdentityBootstrap::ephemeral(
                    generate_ble_identity(),
                    EspIdentityError::Vault(error),
                ),
            }
        }
        Err(error) if recoverable(&error) => {
            recover_ble_identity(vault, label, EspIdentityError::Vault(error))
        }
        Err(error) => {
            IdentityBootstrap::ephemeral(generate_ble_identity(), EspIdentityError::Vault(error))
        }
    }
}

fn create_node_identity(
    vault: &mut Vault,
    label: &IdentityLabel,
) -> IdentityBootstrap<HopspotNodeIdentity, EspIdentityError> {
    let identity = generate_node_identity();
    match vault.store(label, identity.secret()) {
        Ok(()) => match verify_node_identity(vault, label, &identity) {
            Ok(()) => IdentityBootstrap::created(identity),
            Err(error) if recovery_allowed(&error) => recover_node_identity(vault, label, error),
            Err(error) => IdentityBootstrap::ephemeral(identity, error),
        },
        Err(error) if recoverable(&error) => {
            recover_node_identity(vault, label, EspIdentityError::Vault(error))
        }
        Err(error) => IdentityBootstrap::ephemeral(identity, EspIdentityError::Vault(error)),
    }
}

fn recover_node_identity(
    vault: &mut Vault,
    label: &IdentityLabel,
    cause: EspIdentityError,
) -> IdentityBootstrap<HopspotNodeIdentity, EspIdentityError> {
    if let Err(error) = vault.erase_all() {
        return IdentityBootstrap::ephemeral(
            generate_node_identity(),
            EspIdentityError::Vault(error),
        );
    }
    let identity = generate_node_identity();
    match vault.store(label, identity.secret()) {
        Ok(()) => match verify_node_identity(vault, label, &identity) {
            Ok(()) => IdentityBootstrap::recovered(identity, cause),
            Err(error) => IdentityBootstrap::ephemeral(identity, error),
        },
        Err(error) => IdentityBootstrap::ephemeral(identity, EspIdentityError::Vault(error)),
    }
}

fn create_ble_identity(
    vault: &mut Vault,
    label: &IdentityLabel,
) -> IdentityBootstrap<BleIdentity, EspIdentityError> {
    let identity = generate_ble_identity();
    let record = Zeroizing::new(encode_persisted_ble_identity(identity));
    match vault.store_blob(label, &record[..]) {
        Ok(()) => match verify_ble_identity(vault, label, identity) {
            Ok(()) => IdentityBootstrap::created(identity),
            Err(error) if recovery_allowed(&error) => recover_ble_identity(vault, label, error),
            Err(error) => IdentityBootstrap::ephemeral(identity, error),
        },
        Err(error) if recoverable(&error) => {
            recover_ble_identity(vault, label, EspIdentityError::Vault(error))
        }
        Err(error) => IdentityBootstrap::ephemeral(identity, EspIdentityError::Vault(error)),
    }
}

fn recover_ble_identity(
    vault: &mut Vault,
    label: &IdentityLabel,
    cause: EspIdentityError,
) -> IdentityBootstrap<BleIdentity, EspIdentityError> {
    if let Err(error) = vault.erase_all() {
        return IdentityBootstrap::ephemeral(
            generate_ble_identity(),
            EspIdentityError::Vault(error),
        );
    }
    let identity = generate_ble_identity();
    let record = Zeroizing::new(encode_persisted_ble_identity(identity));
    match vault.store_blob(label, &record[..]) {
        Ok(()) => match verify_ble_identity(vault, label, identity) {
            Ok(()) => IdentityBootstrap::recovered(identity, cause),
            Err(error) => IdentityBootstrap::ephemeral(identity, error),
        },
        Err(error) => IdentityBootstrap::ephemeral(identity, EspIdentityError::Vault(error)),
    }
}

fn storage_label(name: IdentityStorageName) -> Result<IdentityLabel, EspIdentityError> {
    IdentityLabel::new(name.as_str()).map_err(EspIdentityError::StorageName)
}

fn recoverable(error: &VaultError) -> bool {
    matches!(error, FlashVaultError::StoreFull | FlashVaultError::Corrupt)
}

fn recovery_allowed(error: &EspIdentityError) -> bool {
    match error {
        EspIdentityError::Vault(error) => recoverable(error),
        EspIdentityError::WrongRecordKind
        | EspIdentityError::BleRecordLength(_)
        | EspIdentityError::EmptyBleRecord
        | EspIdentityError::InvalidBleRecord(_)
        | EspIdentityError::Verification => true,
        EspIdentityError::StorageName(_) => false,
    }
}

fn verify_node_identity(
    vault: &Vault,
    label: &IdentityLabel,
    expected: &HopspotNodeIdentity,
) -> Result<(), EspIdentityError> {
    match vault.load(label).map_err(EspIdentityError::Vault)? {
        Some(stored) if &stored[..] == expected.secret() => Ok(()),
        Some(_) | None => Err(EspIdentityError::Verification),
    }
}

fn verify_ble_identity(
    vault: &Vault,
    label: &IdentityLabel,
    expected: BleIdentity,
) -> Result<(), EspIdentityError> {
    let len = vault
        .stored_blob_len(label)
        .map_err(EspIdentityError::Vault)?
        .ok_or(EspIdentityError::Verification)?;
    if len != PERSISTED_BLE_IDENTITY_LEN {
        return Err(EspIdentityError::BleRecordLength(len));
    }
    let mut record = Zeroizing::new([0u8; PERSISTED_BLE_IDENTITY_LEN]);
    vault
        .load_blob(label, &mut record[..])
        .map_err(EspIdentityError::Vault)?
        .ok_or(EspIdentityError::Verification)?;
    match decode_persisted_ble_identity(&record) {
        Ok(Some(stored)) if stored == expected => Ok(()),
        Ok(Some(_)) | Ok(None) => Err(EspIdentityError::Verification),
        Err(error) => Err(EspIdentityError::InvalidBleRecord(error)),
    }
}

fn generate_node_identity() -> HopspotNodeIdentity {
    let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    Rng::new().read(&mut secret[..]);
    HopspotNodeIdentity::new(secret)
}

fn generate_ble_identity() -> BleIdentity {
    let mut bytes = [0u8; BLE_IDENTITY_LEN];
    Rng::new().read(&mut bytes);
    BleIdentity::new(bytes)
}

impl core::fmt::Display for EspIdentityError {
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
