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
    board_target_by_slug, Tier, SHIPPING_BOARD_TARGETS, UPCOMING_BOARD_TARGETS,
};
use crate::routes::Route;

use view::{BoardTargetCard, GuidedFlasher, LocalBuildUnavailablePanel, UnavailablePanel};

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
                "Release flasher"
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                "Flash a Personal Hopspot"
            }
            p { class: "mt-4 max-w-3xl leading-relaxed text-soft",
                "Choose the exact board and verify the signed release locally. Standard updates preserve device data; Fresh install is a separately confirmed full-chip erase for ESP targets."
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
                "The four shipping targets are first. Hardware still in bring-up remains visible, but cannot be flashed from a public release."
            }
            div { class: "mt-6 grid gap-4 md:grid-cols-2",
                for board in SHIPPING_BOARD_TARGETS.iter() {
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
        }
    }
}
