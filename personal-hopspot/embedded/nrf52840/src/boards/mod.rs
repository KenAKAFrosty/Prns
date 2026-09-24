use embassy_nrf::nvmc::{Error as NvmcError, Nvmc};
use personal_rns::identity::vault::{FlashVault, FlashVaultError};
use personal_rns::remote_control::{
    RemoteControlNodeIdentityBootstrap, RemoteControlNodeIdentityBootstrapError,
    REMOTE_CONTROL_IDENTITY_VAULT_SLOTS,
};
use prns_core::entropy::{EntropySource, RuntimeEntropy};

#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo",
    feature = "board-rak4631"
))]
mod status_led;

#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-mesh-pocket"
))]
mod button;

#[cfg(any(feature = "board-t096", feature = "board-t114"))]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum DisplayIoError {
    Spi,
    NotInitialized,
}

#[cfg(any(feature = "board-t096", feature = "board-t114"))]
mod tft;

pub(crate) type RemoteControlIdentityBootstrapError =
    RemoteControlNodeIdentityBootstrapError<FlashVaultError<NvmcError>>;

pub(crate) struct RemoteControlIdentityFlash {
    offset: u32,
}

impl RemoteControlIdentityFlash {
    #[cfg(not(feature = "board-rak4631"))]
    pub(crate) const fn at(offset: u32) -> Self {
        Self { offset }
    }

    /// Use when a recovery UF2 replaces an application without erasing the page newly assigned to
    /// the Remote Control identity vault. This recovers only a structurally corrupt load; storage,
    /// verification, and identity-pair failures remain fatal and preserve the page for diagnosis.
    #[cfg(feature = "board-rak4631")]
    pub(crate) const fn at_with_stale_application_page_recovery(offset: u32) -> Self {
        Self { offset }
    }

    pub(crate) fn load_or_generate<S: EntropySource>(
        &self,
        nvmc: &mut Nvmc<'_>,
        entropy: &mut RuntimeEntropy<S>,
    ) -> Result<RemoteControlNodeIdentityBootstrap, RemoteControlIdentityBootstrapError> {
        let mut vault =
            FlashVault::<_, REMOTE_CONTROL_IDENTITY_VAULT_SLOTS>::new(nvmc, self.offset);
        let bootstrap = RemoteControlNodeIdentityBootstrap::load_or_generate_with_runtime_entropy(
            &mut vault, entropy,
        );
        #[cfg(feature = "board-rak4631")]
        if matches!(
            bootstrap,
            Err(RemoteControlNodeIdentityBootstrapError::ControllerLoad(
                FlashVaultError::Corrupt
            ))
        ) {
            return {
                vault
                    .erase_all()
                    .map_err(RemoteControlNodeIdentityBootstrapError::ControllerStore)?;
                RemoteControlNodeIdentityBootstrap::load_or_generate_with_runtime_entropy(
                    &mut vault, entropy,
                )
            };
        }
        bootstrap
    }
}

#[cfg(feature = "board-mesh-pocket")]
pub(crate) mod mesh_pocket;
#[cfg(feature = "board-mesh-tower-v2")]
pub(crate) mod mesh_tower_v2;
#[cfg(feature = "board-muzi-base-duo")]
pub(crate) mod muzi_base_duo;
#[cfg(feature = "board-rak4631")]
pub(crate) mod rak4631;
#[cfg(feature = "board-t096")]
pub(crate) mod t096;
#[cfg(feature = "board-t1000e")]
pub(crate) mod t1000e;
#[cfg(feature = "board-t114")]
pub(crate) mod t114;
#[cfg(feature = "board-t-echo")]
pub(crate) mod t_echo;

#[cfg(all(
    feature = "board-mesh-pocket",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2"),
    not(feature = "board-muzi-base-duo"),
    not(feature = "board-rak4631")
))]
pub(crate) use mesh_pocket as selected;

#[cfg(all(
    feature = "board-mesh-tower-v2",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-mesh-pocket"),
    not(feature = "board-t1000e"),
    not(feature = "board-muzi-base-duo"),
    not(feature = "board-rak4631")
))]
pub(crate) use mesh_tower_v2 as selected;
#[cfg(all(
    feature = "board-muzi-base-duo",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-mesh-pocket"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2"),
    not(feature = "board-rak4631")
))]
pub(crate) use muzi_base_duo as selected;
#[cfg(all(
    feature = "board-rak4631",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-mesh-pocket"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2"),
    not(feature = "board-muzi-base-duo")
))]
pub(crate) use rak4631 as selected;
#[cfg(all(
    feature = "board-t096",
    not(feature = "board-t-echo"),
    not(feature = "board-t114"),
    not(feature = "board-mesh-pocket"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2"),
    not(feature = "board-muzi-base-duo"),
    not(feature = "board-rak4631")
))]
#[allow(unused_imports)] // Reserved for the runtime once the bring-up boundary is cleared.
pub(crate) use t096 as selected;
#[cfg(all(
    feature = "board-t1000e",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-mesh-pocket"),
    not(feature = "board-mesh-tower-v2"),
    not(feature = "board-muzi-base-duo"),
    not(feature = "board-rak4631")
))]
pub(crate) use t1000e as selected;
#[cfg(all(
    feature = "board-t114",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-mesh-pocket"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2"),
    not(feature = "board-muzi-base-duo"),
    not(feature = "board-rak4631")
))]
pub(crate) use t114 as selected;
#[cfg(all(
    feature = "board-t-echo",
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-mesh-pocket"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2"),
    not(feature = "board-muzi-base-duo"),
    not(feature = "board-rak4631")
))]
pub(crate) use t_echo as selected;
