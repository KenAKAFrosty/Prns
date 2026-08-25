use embassy_nrf::nvmc::{Error as NvmcError, Nvmc};
use personal_hopspot_core::{
    bootstrap_flash_node_identity, FlashIdentityError, HopspotNodeIdentity, IdentityBootstrap,
    T1000E_NODE_IDENTITY_FLASH_OFFSET,
};
use personal_rns::identity::vault::FlashVault;

const VAULT_SLOTS: usize = 1;

pub(crate) type Error = FlashIdentityError<NvmcError>;

pub(crate) fn bootstrap_node_identity(
    nvmc: &mut Nvmc<'_>,
    fill_entropy: &mut impl FnMut(&mut [u8]),
) -> IdentityBootstrap<HopspotNodeIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(nvmc, T1000E_NODE_IDENTITY_FLASH_OFFSET);
    bootstrap_flash_node_identity(&mut vault, fill_entropy)
}
