use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::PlatformChip;
use crate::flash_manifest::{
    embedded_docs_mode, flash_artifact_for_board, EmbeddedPolicy, FlashArtifactRecord,
};
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
                ReadyTargetPanel {
                    target,
                    artifact: flash_artifact_for_board(target.slug),
                }
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
            div { class: "mt-4 flex items-center gap-4",
                if let Some(image) = board.image() {
                    span { class: "flash-board-slot flash-board-slot--inset",
                        img {
                            class: "flash-board-img",
                            src: image.data_uri,
                            alt: "",
                            loading: "lazy",
                        }
                    }
                }
                p { class: "flash-board-silicon font-mono text-xs leading-snug",
                    "{board.silicon}"
                }
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
                    if selected {
                        span {
                            class: "inline-flex items-center py-2.5 text-xs font-bold uppercase tracking-wider text-accent leading-none",
                            "aria-current": "true",
                            {t!("flash-card-selected")}
                        }
                    } else {
                        Link {
                            to: Route::FlashBoardPage { board: board.slug.to_string() },
                            class: "flash-card-action",
                            "aria-label": "Flash {board.name}",
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
fn ReadyTargetPanel(
    target: &'static BoardTarget,
    artifact: Option<&'static FlashArtifactRecord>,
) -> Element {
    let embedded_site = embedded_docs_mode();
    let action_enabled = artifact
        .map(|artifact| artifact.web_action_enabled(embedded_site))
        .unwrap_or(false);
    let action_label = artifact
        .map(|artifact| artifact.action_label(embedded_site))
        .unwrap_or("Manifest missing");
    let download_path = artifact.and_then(|artifact| artifact.download_path(embedded_site));
    let online_url = format!("https://prns.dev/flash/{}", target.slug);

    rsx! {
        section { class: "flash-flasher-panel mt-8",
            div { class: "flash-flasher-panel__main",
                p { class: "text-xs font-semibold tracking-[0.2em] uppercase text-accent",
                    {t!("flash-ready-kicker")}
                }
                h2 { class: "mt-2 text-2xl font-semibold tracking-tight text-paper",
                    {t!("flash-ready-title")}
                }
                div { class: "flash-ready-target mt-5",
                    div { class: "flash-ready-target__summary",
                        div { class: "flash-ready-target__copy",
                            PlatformChip {
                                name: target.name.to_string(),
                                icon: target.icon.map(str::to_string),
                                badge: None,
                                muted: false,
                                decorative: false,
                            }
                            p { class: "flash-board-silicon font-mono text-xs leading-snug",
                                "{target.silicon}"
                            }
                        }
                        if let Some(image) = target.image() {
                            span { class: "flash-board-slot flash-board-slot--inset flash-board-slot--hero",
                                img {
                                    class: "flash-board-img",
                                    src: image.data_uri,
                                    alt: "",
                                    loading: "lazy",
                                }
                            }
                        }
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

                if let Some(artifact) = artifact {
                    FlashPlanPanel {
                        artifact,
                        embedded_site,
                    }
                } else {
                    div { class: "flash-artifact-panel mt-5",
                        p { class: "flash-status-chip flash-status-chip--blocked",
                            "Manifest missing"
                        }
                        p { class: "mt-3 text-sm leading-relaxed text-soft",
                            "This board is flashable in the catalog, but no artifact manifest entry exists yet."
                        }
                    }
                }
            }
            div { class: "flash-flasher-panel__action",
                if embedded_site && artifact.map(|artifact| matches!(artifact.embedded_policy, EmbeddedPolicy::HostedOnly)).unwrap_or(false) {
                    a {
                        href: "{online_url}",
                        class: "flash-primary-action",
                        "{action_label}"
                    }
                } else if let Some(download_path) = download_path {
                    a {
                        href: "{download_path}",
                        class: "flash-primary-action",
                        download: "{target.slug}.uf2",
                        "{action_label}"
                    }
                } else {
                    button {
                        r#type: "button",
                        disabled: !action_enabled,
                        class: "flash-primary-action",
                        "{action_label}"
                    }
                }
                p { class: "mt-3 text-xs leading-relaxed text-mid",
                    if let Some(artifact) = artifact {
                        "{artifact.status_note(embedded_site)}"
                    } else {
                        "Add this board to the flash artifact manifest before enabling web actions."
                    }
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

#[component]
fn FlashPlanPanel(artifact: &'static FlashArtifactRecord, embedded_site: bool) -> Element {
    rsx! {
        div { class: "flash-plan-panel mt-5",
            div { class: "flash-plan-panel__head",
                h3 { class: "text-sm font-semibold text-paper",
                    "Flash plan"
                }
                span { class: flash_status_class(artifact, embedded_site),
                    "{artifact.state.label()}"
                }
            }
            ol { class: "flash-step-list mt-3",
                for (index, step) in artifact.steps.iter().enumerate() {
                    li {
                        key: "{index}",
                        span { class: "flash-step-list__index", "{index + 1}" }
                        span { "{step}" }
                    }
                }
            }
        }
    }
}

fn flash_status_class(artifact: &FlashArtifactRecord, embedded_site: bool) -> &'static str {
    if embedded_site && matches!(artifact.embedded_policy, EmbeddedPolicy::HostedOnly) {
        "flash-status-chip flash-status-chip--blocked"
    } else if artifact.web_action_enabled(embedded_site) {
        "flash-status-chip flash-status-chip--ready"
    } else {
        "flash-status-chip flash-status-chip--pending"
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
