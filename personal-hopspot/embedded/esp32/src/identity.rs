use esp_hal::rng::Rng;
#[cfg(target_arch = "xtensa")]
use personal_hopspot_core::UiNotice;
use personal_hopspot_core::{
    bootstrap_flash_ble_identity, bootstrap_flash_node_identity, FlashIdentityError,
    HopspotNodeIdentity, IdentityBootstrap, IdentityPersistence,
};
use personal_rns::identity::vault::FlashVault;
use personal_rns::interfaces::bluetooth_auto::BleIdentity;

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

type Error = FlashIdentityError<EspRomFlashError>;

pub fn log_persistence(identity: &str, persistence: &IdentityPersistence<Error>) {
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
    node: &IdentityPersistence<Error>,
    bluetooth: &IdentityPersistence<Error>,
) -> Option<UiNotice> {
    if node.is_ephemeral() || bluetooth.is_ephemeral() {
        Some(UiNotice::IdentityUnstable)
    } else if node.is_recovered() || bluetooth.is_recovered() {
        Some(UiNotice::IdentityReset)
    } else {
        None
    }
}

pub fn bootstrap_node_identity() -> IdentityBootstrap<HopspotNodeIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(
        EspRomFlash::<IDENTITY_FLASH_END>::new(),
        NODE_IDENTITY_FLASH_OFFSET,
    );
    bootstrap_flash_node_identity(&mut vault, &mut hardware_entropy)
}

pub fn bootstrap_ble_identity() -> IdentityBootstrap<BleIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(
        EspRomFlash::<IDENTITY_FLASH_END>::new(),
        BLE_IDENTITY_FLASH_OFFSET,
    );
    bootstrap_flash_ble_identity(&mut vault, &mut hardware_entropy)
}

fn hardware_entropy(bytes: &mut [u8]) {
    Rng::new().read(bytes);
}
