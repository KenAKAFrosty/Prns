use embassy_nrf::nvmc::{Error as NvmcError, Nvmc};
use personal_rns::identity::vault::{FlashVault, FlashVaultError};
use personal_rns::remote_control::{
    load_factory_controller_grant, RemoteControlControllerGrant, RemoteControlControllerGrants,
    RemoteControlInitialControllerGrants, RemoteControlNodeIdentityBootstrap,
    RemoteControlNodeIdentityBootstrapError, REMOTE_CONTROL_IDENTITY_VAULT_SLOTS,
};
use prns_core::entropy::{EntropySource, RuntimeEntropy};

#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2",
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

pub(crate) struct RemoteControlIdentityLoad {
    pub bootstrap: RemoteControlNodeIdentityBootstrap,
    pub factory_grant: Option<RemoteControlControllerGrant>,
}

impl RemoteControlIdentityFlash {
    pub(crate) const fn at(offset: u32) -> Self {
        Self { offset }
    }

    pub(crate) fn load_or_generate<S: EntropySource>(
        &self,
        nvmc: &mut Nvmc<'_>,
        entropy: &mut RuntimeEntropy<S>,
    ) -> Result<RemoteControlIdentityLoad, RemoteControlIdentityBootstrapError> {
        let mut vault =
            FlashVault::<_, REMOTE_CONTROL_IDENTITY_VAULT_SLOTS>::new(nvmc, self.offset);
        let bootstrap =
            match RemoteControlNodeIdentityBootstrap::load_or_generate_with_runtime_entropy(
                &mut vault, entropy,
            ) {
                Ok(bootstrap) => bootstrap,
                Err(_) => {
                    // Leftover factory/Meshtastic bytes in this page make load() return Corrupt,
                    // which used to panic via expect() before USB came up.
                    vault
                        .erase_all()
                        .map_err(RemoteControlNodeIdentityBootstrapError::ControllerStore)?;
                    RemoteControlNodeIdentityBootstrap::load_or_generate_with_runtime_entropy(
                        &mut vault, entropy,
                    )?
                }
            };
        let factory_grant = load_factory_controller_grant(&vault).ok().flatten();
        Ok(RemoteControlIdentityLoad {
            bootstrap,
            factory_grant,
        })
    }
}

pub(crate) fn initial_controller_grants(
    factory_grant: Option<RemoteControlControllerGrant>,
    storage: &mut Option<[RemoteControlControllerGrant; 1]>,
) -> RemoteControlInitialControllerGrants<'_> {
    let Some(grant) = factory_grant else {
        return RemoteControlInitialControllerGrants::Nobody;
    };
    *storage = Some([grant]);
    let Some(grants) = storage.as_ref() else {
        return RemoteControlInitialControllerGrants::Nobody;
    };
    match RemoteControlControllerGrants::try_from(grants.as_slice()) {
        Ok(grants) => RemoteControlInitialControllerGrants::Grants(grants),
        Err(_) => RemoteControlInitialControllerGrants::Nobody,
    }
}

#[cfg(feature = "board-mesh-pocket")]
pub(crate) mod mesh_pocket;
#[cfg(feature = "board-mesh-tower-v2")]
pub(crate) mod mesh_tower_v2;
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
    not(feature = "board-rak4631")
))]
pub(crate) use mesh_tower_v2 as selected;
#[cfg(all(
    feature = "board-rak4631",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-mesh-pocket"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2")
))]
pub(crate) use rak4631 as selected;
#[cfg(all(
    feature = "board-t096",
    not(feature = "board-t-echo"),
    not(feature = "board-t114"),
    not(feature = "board-mesh-pocket"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2"),
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
    not(feature = "board-rak4631")
))]
pub(crate) use t_echo as selected;
