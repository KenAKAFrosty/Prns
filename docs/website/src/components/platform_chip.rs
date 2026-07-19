use dioxus::prelude::*;

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
