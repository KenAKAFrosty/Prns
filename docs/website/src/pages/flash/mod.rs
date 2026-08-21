mod bridge;
mod contract;
mod model;
mod protocol;
mod release;
mod trust;
mod view;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::local_development;
use crate::platforms::{
    board_target_by_slug, Tier, IN_PROGRESS_BOARD_TARGETS, QUALIFICATION_BOARD_TARGETS,
    SHIPPING_BOARD_TARGETS, UPCOMING_BOARD_TARGETS,
};
use crate::routes::Route;

use view::{BoardTargetCard, GuidedFlasher, LocalBuildUnavailablePanel, UnavailablePanel};

#[cfg(any(test, feature = "browser-test-fixture"))]
fn validate_release_0_2_6_fixture(
    manifest: &[u8],
) -> Result<prns_flash_manifest::ValidatedFlashManifest, Box<dyn std::error::Error>> {
    let mut catalog = prns_flash_manifest::board_catalog()?;
    let historical_heltec = catalog
        .boards
        .iter_mut()
        .find(|board| board.slug == "heltec-v4")
        .ok_or("0.2.6 fixture board is missing from the current catalog")?;
    historical_heltec.display_name = "Heltec LoRa 32 V4".to_string();
    historical_heltec.silicon = "ESP32-S3 + SX1262".to_string();
    let historical_targets = prns_flash_manifest::ManifestTargetSetPolicy::local_development(
        &catalog,
        &["heltec-v4", "t-beam-supreme", "t-echo", "xiao-esp32-c6"],
    )?;

    Ok(
        prns_flash_manifest::ValidatedFlashManifest::from_json_with_target_set(
            manifest,
            &catalog,
            &historical_targets,
        )?,
    )
}

#[cfg(test)]
fn release_0_2_6_fixture(
) -> Result<prns_flash_manifest::ValidatedFlashManifest, Box<dyn std::error::Error>> {
    const MANIFEST: &[u8] = include_bytes!(
        "../../../web-flasher/browser/fixtures/signed-candidate/releases/0.2.6/flash-manifest.json"
    );

    validate_release_0_2_6_fixture(MANIFEST)
}

#[cfg(test)]
#[test]
fn release_0_2_6_fixture_retains_its_historical_board_set() -> Result<(), Box<dyn std::error::Error>>
{
    let manifest = release_0_2_6_fixture()?;
    let boards = manifest
        .targets()
        .iter()
        .map(|target| target.board_id().as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        boards,
        ["heltec-v4", "t-beam-supreme", "xiao-esp32-c6", "t-echo"]
    );
    Ok(())
}

#[component]
pub fn FlashPage() -> Element {
    rsx! { FlashExperience { selected_slug: None } }
}

#[component]
pub fn FlashBoardPage(board: String) -> Element {
    rsx! { FlashExperience { selected_slug: Some(board) } }
}

#[component]
fn FlashExperience(selected_slug: Option<String>) -> Element {
    let selected_target = selected_slug.as_deref().and_then(board_target_by_slug);
    let missing_selection = selected_slug.is_some() && selected_target.is_none();

    rsx! {
        header { class: "mb-10",
            Link {
                to: if selected_slug.is_some() { Route::FlashPage {} } else { Route::PlatformsPage {} },
                class: "text-sm text-soft hover:text-accent transition-colors",
                "← "
                if selected_slug.is_some() { {t!("flash-back-boards")} } else { {t!("flash-back")} }
            }
            p { class: "mt-6 text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                "Beta"
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                "Flash a Personal Hopspot"
            }
            p { class: "mt-4 max-w-3xl leading-relaxed text-soft",
                "Choose your exact board and flash a signed release straight from your browser: every byte is verified locally before it touches the device. Update keeps your device's data. Fresh install erases everything, and asks for its own confirmation first."
            }
        }

        if let Some(target) = selected_target {
            if target.is_flashable() && local_development::board_is_included(target.slug) {
                GuidedFlasher { key: "{target.slug}", target }
            } else if target.is_flashable() && local_development::enabled() {
                LocalBuildUnavailablePanel {}
            } else {
                UnavailablePanel {}
            }
        } else if missing_selection {
            section { class: "rounded-card border border-line/60 bg-layer/40 p-5",
                h2 { class: "text-xl font-semibold text-paper", "Board not found" }
                p { class: "mt-3 text-soft", "Choose one of the supported shipping boards below." }
            }
        }

        section { class: if selected_target.is_some() { "mt-12" } else { "mt-4" },
            h2 { class: "text-2xl font-semibold tracking-tight text-paper",
                if selected_target.is_some() { "Change board" } else { "Select the exact board" }
            }
            p { class: "mt-3 max-w-3xl leading-relaxed text-soft",
                "Shipping targets flash from a signed public release. Boards in hardware qualification and final bring-up sit beside them and graduate in place."
            }
            div { class: "mt-6 grid gap-4 md:grid-cols-2",
                for board in SHIPPING_BOARD_TARGETS
                    .iter()
                    .chain(QUALIFICATION_BOARD_TARGETS.iter())
                    .chain(IN_PROGRESS_BOARD_TARGETS.iter())
                {
                    BoardTargetCard {
                        key: "{board.slug}",
                        board,
                        selected: selected_target.is_some_and(|target| target.slug == board.slug),
                    }
                }
            }
            section { class: "mt-10",
                h3 { class: "text-xl font-semibold tracking-tight text-paper", "Active bring-up" }
                p { class: "mt-2 max-w-3xl text-sm leading-relaxed text-soft",
                    "These boards are actively being brought online. They are visible here for progress tracking, but are not public flash targets yet."
                }
                div { class: "mt-5 grid gap-4 md:grid-cols-2",
                    for board in UPCOMING_BOARD_TARGETS.iter().filter(|board| board.tier == Tier::BringUp) {
                        BoardTargetCard { key: "{board.slug}", board, selected: false }
                    }
                }
            }
            details { class: "mt-6 rounded-card border border-line/50 bg-layer/30 p-4",
                summary { class: "cursor-pointer font-semibold text-soft", "Roadmap" }
                div { class: "mt-4 grid gap-4 md:grid-cols-2",
                    for board in UPCOMING_BOARD_TARGETS.iter().filter(|board| board.tier == Tier::Roadmap) {
                        BoardTargetCard { key: "{board.slug}", board, selected: false }
                    }
                }
            }
            p { class: "mt-6 text-sm text-soft",
                "Not seeing a board you want supported? "
                a {
                    href: "https://github.com/KenAKAFrosty/Prns/issues/new?title=Board%20support%20request%3A%20",
                    target: "_blank",
                    rel: "noopener",
                    class: "text-accent hover:underline",
                    "Let us know in a GitHub issue"
                }
                " and help steer what comes next."
            }
        }
    }
}
