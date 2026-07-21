mod bridge;
mod contract;
mod model;
mod protocol;
mod release;
mod trust;
mod view;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::platforms::{board_target_by_slug, ROADMAP_BOARD_TARGETS, SHIPPING_BOARD_TARGETS};
use crate::routes::Route;

use view::{BoardTargetCard, GuidedFlasher, UnavailablePanel};

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
                "Choose the exact board, verify the signed release locally, then write only its sparse firmware parts. Existing Wi-Fi is preserved unless you explicitly change it."
            }
        }

        if let Some(target) = selected_target {
            if target.is_flashable() {
                GuidedFlasher { key: "{target.slug}", target }
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
            details { class: "mt-6 rounded-card border border-line/50 bg-layer/30 p-4",
                summary { class: "cursor-pointer font-semibold text-soft", "Coming later" }
                div { class: "mt-4 grid gap-4 md:grid-cols-2",
                    for board in ROADMAP_BOARD_TARGETS.iter() {
                        BoardTargetCard { key: "{board.slug}", board, selected: false }
                    }
                }
            }
        }
    }
}
