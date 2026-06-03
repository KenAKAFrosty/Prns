use dioxus::prelude::*;

/// One platform "card": an optional logo (a bundled Simple Icon, tinted to the
/// chip's text color via a CSS mask so it matches the palette and brightens on
/// hover) followed by the name. Shared by the landing marquee and the
/// /platforms page so the two never drift.
///
/// - `icon`: Simple Icons slug → /assets/logos/<slug>.svg, or None for text-only.
/// - `soon`: roadmap styling (dashed) + a trailing "soon" tag.
/// - `decorative`: aria-hidden (used for the marquee's duplicated second copy,
///   so screen readers read the platform list only once).
#[component]
pub fn PlatformChip(name: String, icon: Option<String>, soon: bool, decorative: bool) -> Element {
    let class = if soon {
        "platform-chip platform-chip--soon"
    } else {
        "platform-chip"
    };
    rsx! {
        span {
            class: "{class}",
            "aria-hidden": if decorative { "true" } else { "false" },
            if let Some(slug) = icon {
                span {
                    class: "platform-chip__icon",
                    style: "--logo: url('/assets/logos/{slug}.svg')",
                }
            }
            "{name}"
            if soon {
                span { class: "platform-chip__soon", "soon" }
            }
        }
    }
}
