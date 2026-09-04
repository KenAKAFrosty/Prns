use embassy_nrf::nvmc::{Error as NvmcError, Nvmc};
use personal_hopspot_core::{
    bootstrap_flash_ble_identity_with_runtime_entropy,
    bootstrap_flash_node_identity_with_runtime_entropy, FlashIdentityError, HopspotNodeIdentity,
    IdentityBootstrap, IdentityPersistence, UiNotice, NRF52840_BLE_IDENTITY_FLASH_OFFSET,
    NRF52840_NODE_IDENTITY_FLASH_OFFSET,
};
use personal_rns::identity::vault::FlashVault;
use personal_rns::interfaces::bluetooth_auto::BleIdentity;
use prns_core::entropy::{EntropySource, RuntimeEntropy};

const VAULT_SLOTS: usize = 1;

pub(crate) type Error = FlashIdentityError<NvmcError>;

pub(crate) fn bootstrap_node_identity<S: EntropySource>(
    nvmc: &mut Nvmc<'_>,
    entropy: &mut RuntimeEntropy<S>,
) -> IdentityBootstrap<HopspotNodeIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(nvmc, NRF52840_NODE_IDENTITY_FLASH_OFFSET);
    bootstrap_flash_node_identity_with_runtime_entropy(&mut vault, entropy)
}

pub(crate) fn bootstrap_ble_identity<S: EntropySource>(
    nvmc: &mut Nvmc<'_>,
    entropy: &mut RuntimeEntropy<S>,
) -> IdentityBootstrap<BleIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(nvmc, NRF52840_BLE_IDENTITY_FLASH_OFFSET);
    bootstrap_flash_ble_identity_with_runtime_entropy(&mut vault, entropy)
}

pub(crate) fn startup_notice(
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
