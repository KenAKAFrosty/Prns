use dioxus::prelude::*;

/// One platform "card": an optional logo (a bundled Simple Icon, tinted to the
/// chip's text color via a CSS mask so it matches the palette and brightens on
/// hover) followed by the name. Shared by the landing marquee and the
/// /platforms page so the two never drift.
///
/// - `icon`: Simple Icons slug -> /assets/logos/<slug>.svg, an explicit
///   asset filename such as dioxus.png, or None for text-only.
/// - `badge`: optional support-state tag, such as "flashable" or "bring-up".
/// - `muted`: dimmer/dashed styling for not-yet-flashable targets.
/// - `decorative`: aria-hidden (used for the marquee's duplicated second copy,
///   so screen readers read the platform list only once).
#[component]
pub fn PlatformChip(
    name: String,
    icon: Option<String>,
    badge: Option<String>,
    muted: bool,
    decorative: bool,
) -> Element {
    let class = if muted {
        "platform-chip platform-chip--muted"
    } else {
        "platform-chip"
    };
    rsx! {
        span {
            class: "{class}",
            "aria-hidden": if decorative { "true" } else { "false" },
            if let Some(slug) = icon {
                {
                    let logo = logo_asset(&slug);
                    rsx! {
                        span {
                            class: "platform-chip__icon",
                            style: "--logo: url('{logo}')",
                        }
                    }
                }
            }
            "{name}"
            if let Some(badge) = badge {
                span { class: "platform-chip__badge", "{badge}" }
            }
        }
    }
}

fn logo_asset(slug: &str) -> String {
    if slug.contains('.') {
        format!("/assets/logos/{slug}")
    } else {
        format!("/assets/logos/{slug}.svg")
    }
}
