use embassy_nrf::nvmc::{Error as NvmcError, Nvmc};
use personal_hopspot_core::{
    bootstrap_flash_ble_identity, bootstrap_flash_node_identity, FlashIdentityError,
    HopspotNodeIdentity, IdentityBootstrap, MESH_TOWER_V2_BLE_IDENTITY_FLASH_OFFSET,
    NRF52840_NODE_IDENTITY_FLASH_OFFSET,
};
use personal_rns::identity::vault::FlashVault;
use personal_rns::interfaces::bluetooth_auto::BleIdentity;

const VAULT_SLOTS: usize = 1;

pub(crate) type Error = FlashIdentityError<NvmcError>;

pub(crate) fn bootstrap_node_identity(
    nvmc: &mut Nvmc<'_>,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<HopspotNodeIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(nvmc, NRF52840_NODE_IDENTITY_FLASH_OFFSET);
    bootstrap_flash_node_identity(&mut vault, fill_entropy)
}

pub(crate) fn bootstrap_ble_identity(
    nvmc: &mut Nvmc<'_>,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<BleIdentity, Error> {
    let mut vault =
        FlashVault::<_, VAULT_SLOTS>::new(nvmc, MESH_TOWER_V2_BLE_IDENTITY_FLASH_OFFSET);
    bootstrap_flash_ble_identity(&mut vault, fill_entropy)
}
