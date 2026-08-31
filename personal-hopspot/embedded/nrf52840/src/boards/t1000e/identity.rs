use embassy_nrf::nvmc::{Error as NvmcError, Nvmc};
use personal_hopspot_core::{
    bootstrap_flash_node_identity_with_runtime_entropy, FlashIdentityError, HopspotNodeIdentity,
    IdentityBootstrap, T1000E_NODE_IDENTITY_FLASH_OFFSET,
};
use personal_rns::identity::vault::FlashVault;
use prns_core::entropy::{EntropySource, RuntimeEntropy};

const VAULT_SLOTS: usize = 1;

pub(crate) type Error = FlashIdentityError<NvmcError>;

pub(crate) fn bootstrap_node_identity<S: EntropySource>(
    nvmc: &mut Nvmc<'_>,
    entropy: &mut RuntimeEntropy<S>,
) -> IdentityBootstrap<HopspotNodeIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(nvmc, T1000E_NODE_IDENTITY_FLASH_OFFSET);
    bootstrap_flash_node_identity_with_runtime_entropy(&mut vault, entropy)
}
