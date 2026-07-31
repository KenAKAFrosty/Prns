use dioxus::prelude::*;

#[cfg(feature = "local-dev-flasher")]
use prns_flash_manifest::{BoardCatalog, ManifestError, ManifestTargetSetPolicy};

#[cfg(feature = "local-dev-flasher")]
pub(crate) const TRUST_MARKER: &str = "PRNS_LOCAL_DEV_FLASHER_TRUST_ROOT_V1";
#[cfg(feature = "local-dev-flasher")]
const BANNER_MARKER: &str = "PRNS_LOCAL_DEV_FLASHER_BANNER_V1";

pub(crate) const fn enabled() -> bool {
    cfg!(feature = "local-dev-flasher")
}

pub(crate) fn board_is_included(slug: &str) -> bool {
    #[cfg(feature = "local-dev-flasher")]
    {
        env!("PRNS_LOCAL_DEV_BOARDS")
            .split(',')
            .any(|selected| selected == slug)
    }

    #[cfg(not(feature = "local-dev-flasher"))]
    {
        let _ = slug;
        true
    }
}

#[cfg(feature = "local-dev-flasher")]
pub(crate) fn manifest_target_set_policy(
    catalog: &BoardCatalog,
) -> Result<ManifestTargetSetPolicy, ManifestError> {
    let boards = env!("PRNS_LOCAL_DEV_BOARDS").split(',').collect::<Vec<_>>();
    ManifestTargetSetPolicy::local_development(catalog, &boards)
}

#[cfg(feature = "local-dev-flasher")]
#[component]
pub(crate) fn LocalDevelopmentBanner() -> Element {
    let state = env!("PRNS_LOCAL_DEV_SOURCE_STATE").to_ascii_uppercase();
    let boards = env!("PRNS_LOCAL_DEV_BOARDS").replace(',', ", ");
    rsx! {
        aside {
            role: "alert",
            "data-prns-local-dev-banner": BANNER_MARKER,
            class: "sticky top-0 z-50 border-y border-amber-300/50 bg-amber-300/15 px-6 py-3 text-center text-amber-100 backdrop-blur",
            p { class: "text-sm font-extrabold tracking-wide",
                "LOCAL DEVELOPER FIRMWARE — EPHEMERALLY SIGNED, NOT A RELEASE"
            }
            p { class: "mt-1 break-all font-mono text-xs",
                "HEAD {env!(\"PRNS_GIT_COMMIT\")} · {state} · source {env!(\"PRNS_LOCAL_DEV_SOURCE_DIGEST\")} · boards {boards}"
            }
        }
    }
}

#[cfg(not(feature = "local-dev-flasher"))]
#[component]
pub(crate) fn LocalDevelopmentBanner() -> Element {
    rsx! {}
}
