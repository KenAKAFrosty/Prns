use embassy_nrf::nvmc::{Error as NvmcError, Nvmc};
use personal_hopspot_core::{
    bootstrap_flash_ble_identity, bootstrap_flash_node_identity, FlashIdentityError,
    HopspotNodeIdentity, IdentityBootstrap, IdentityPersistence, UiNotice,
};
use personal_rns::identity::vault::FlashVault;
use personal_rns::interfaces::bluetooth_auto::BleIdentity;

const FLASH_PAGE_LEN: u32 = 0x1000;
const NODE_IDENTITY_FLASH_OFFSET: u32 = 0xEB000;
const BLE_IDENTITY_FLASH_OFFSET: u32 = 0xEC000;
const RESERVED_FLASH_END: u32 = 0xED000;
const VAULT_SLOTS: usize = 1;

const _: () = assert!(NODE_IDENTITY_FLASH_OFFSET + FLASH_PAGE_LEN == BLE_IDENTITY_FLASH_OFFSET);
const _: () = assert!(BLE_IDENTITY_FLASH_OFFSET + FLASH_PAGE_LEN == RESERVED_FLASH_END);

pub(super) type Error = FlashIdentityError<NvmcError>;

pub(super) fn bootstrap_node_identity(
    nvmc: &mut Nvmc<'_>,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<HopspotNodeIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(nvmc, NODE_IDENTITY_FLASH_OFFSET);
    bootstrap_flash_node_identity(&mut vault, fill_entropy)
}

pub(super) fn bootstrap_ble_identity(
    nvmc: &mut Nvmc<'_>,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<BleIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(nvmc, BLE_IDENTITY_FLASH_OFFSET);
    bootstrap_flash_ble_identity(&mut vault, fill_entropy)
}

pub(super) fn startup_notice(
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
