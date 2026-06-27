use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::PlatformChip;
use crate::platforms::{board_target_by_slug, BoardTarget, BOARD_TARGETS};
use crate::routes::Route;

/// Hopspot flashing entrypoint. This is intentionally separate from
/// `/platforms`: platform support is broad, flashing is board-specific.
#[component]
pub fn FlashPage() -> Element {
    rsx! {
        FlashExperience { selected_slug: None }
    }
}

#[component]
pub fn FlashBoardPage(board: String) -> Element {
    rsx! {
        FlashExperience { selected_slug: Some(board) }
    }
}

#[component]
fn FlashExperience(selected_slug: Option<String>) -> Element {
    let selected_target = selected_slug.as_deref().and_then(board_target_by_slug);
    let missing_selection = selected_slug.is_some() && selected_target.is_none();

    rsx! {
        header { class: "mb-10",
            Link {
                to: Route::PlatformsPage {},
                class: "text-sm text-soft hover:text-accent transition-colors",
                "← "
                {t!("flash-back")}
            }
            p { class: "mt-6 text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                {t!("flash-kicker")}
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                {t!("flash-title")}
            }
            p { class: "mt-4 text-soft max-w-3xl leading-relaxed",
                {t!("flash-lead")}
            }
            p { class: "mt-5 max-w-3xl rounded-card border border-line/60 bg-layer/40 px-4 py-3 text-sm text-mid leading-relaxed",
                {t!("flash-note")}
            }
        }

        if let Some(target) = selected_target {
            if target.is_flashable() {
                ReadyTargetPanel { target }
            } else {
                section { class: "mt-8 rounded-card border border-line/60 bg-layer/40 p-5",
                    h2 { class: "text-xl font-semibold text-paper",
                        {t!("flash-unavailable-title")}
                    }
                    p { class: "mt-3 text-soft leading-relaxed",
                        {t!("flash-unavailable-body")}
                    }
                }
            }
        } else if missing_selection {
            section { class: "mt-8 rounded-card border border-line/60 bg-layer/40 p-5",
                h2 { class: "text-xl font-semibold text-paper",
                    {t!("flash-missing-title")}
                }
                p { class: "mt-3 text-soft leading-relaxed",
                    {t!("flash-missing-body")}
                }
            }
        }

        section { class: if selected_target.is_some() { "mt-10" } else { "mt-4" },
            if selected_target.is_some() {
                h2 { class: "text-2xl font-semibold tracking-tight text-paper",
                    {t!("flash-picker-change-title")}
                }
            } else {
                h2 { class: "text-2xl font-semibold tracking-tight text-paper",
                    {t!("flash-board-title")}
                }
            }
            p { class: "mt-3 text-soft max-w-3xl leading-relaxed",
                {t!("flash-board-lead")}
            }
            div { class: "mt-6 grid gap-4 md:grid-cols-2",
                for board in BOARD_TARGETS.iter() {
                    BoardTargetCard {
                        key: "{board.slug}",
                        board,
                        selected: selected_target
                            .map(|target| target.slug == board.slug)
                            .unwrap_or(false),
                    }
                }
            }
        }
    }
}

#[component]
fn BoardTargetCard(board: &'static BoardTarget, selected: bool) -> Element {
    rsx! {
        div {
            class: board_card_class(board, selected),
            div { class: "flex flex-wrap items-start gap-3",
                PlatformChip {
                    name: board.name.to_string(),
                    icon: board.icon.map(str::to_string),
                    badge: board.tier.chip_badge().map(str::to_string),
                    muted: board.tier.muted(),
                    decorative: false,
                }
            }
            p { class: "flash-board-silicon mt-4 font-mono text-xs leading-snug",
                "{board.silicon}"
            }
            if board.interfaces.is_empty() {
                p { class: "flash-interfaces-pending mt-4",
                    {t!("flash-interfaces-pending")}
                }
            } else {
                div { class: "mt-4",
                    p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-mid",
                        {t!("flash-interfaces-label")}
                    }
                    div { class: "mt-2 flex flex-wrap gap-2",
                        for interface in board.interfaces.iter() {
                            span {
                                key: "{interface}",
                                class: "flash-interface-chip",
                                "{interface}"
                            }
                        }
                    }
                }
            }
            if board.is_flashable() {
                div { class: "mt-5 flex justify-end",
                    Link {
                        to: Route::FlashBoardPage { board: board.slug.to_string() },
                        class: if selected { "flash-card-action flash-card-action--selected" } else { "flash-card-action" },
                        "aria-label": "Flash {board.name}",
                        if selected {
                            span { {t!("flash-card-selected")} }
                        } else {
                            span { {t!("flash-card-action")} }
                            span {
                                class: "flash-card-action__arrow",
                                "aria-hidden": "true",
                                "→"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ReadyTargetPanel(target: &'static BoardTarget) -> Element {
    rsx! {
        section { class: "flash-flasher-panel mt-8",
            div { class: "flash-flasher-panel__main",
                p { class: "text-xs font-semibold tracking-[0.2em] uppercase text-accent",
                    {t!("flash-ready-kicker")}
                }
                h2 { class: "mt-2 text-2xl font-semibold tracking-tight text-paper",
                    {t!("flash-ready-title")}
                }
                p { class: "mt-3 max-w-2xl text-sm leading-relaxed text-soft",
                    {t!("flash-ready-body")}
                }
                div { class: "flash-ready-target mt-5",
                    PlatformChip {
                        name: target.name.to_string(),
                        icon: target.icon.map(str::to_string),
                        badge: None,
                        muted: false,
                        decorative: false,
                    }
                    p { class: "flash-board-silicon mt-4 font-mono text-xs leading-snug",
                        "{target.silicon}"
                    }
                    if !target.interfaces.is_empty() {
                        div { class: "mt-4",
                            p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-mid",
                                {t!("flash-interfaces-label")}
                            }
                            div { class: "mt-2 flex flex-wrap gap-2",
                                for interface in target.interfaces.iter() {
                                    span {
                                        key: "{interface}",
                                        class: "flash-interface-chip",
                                        "{interface}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "flash-flasher-panel__action",
                button {
                    r#type: "button",
                    disabled: true,
                    class: "flash-primary-action",
                    {t!("flash-ready-action")}
                }
                p { class: "mt-3 text-xs leading-relaxed text-mid",
                    {t!("flash-ready-action-pending")}
                }
            }
        }

        section { class: "mt-5 rounded-card border border-line/60 bg-surface/45 p-4",
            h2 { class: "text-sm font-semibold text-paper",
                {t!("flash-local-title")}
            }
            p { class: "mt-2 text-sm leading-relaxed text-soft",
                {t!("flash-local-body")}
            }
        }
    }
}

fn board_card_class(board: &BoardTarget, is_selected: bool) -> String {
    let selected_class = if is_selected {
        " flash-board-card--selected"
    } else {
        ""
    };
    format!(
        "flash-board-card {}{} rounded-card border border-line/60 bg-layer/40 p-5 shadow-card",
        board.tier.flash_card_class(),
        selected_class
    )
}
