use dioxus::prelude::*;

#[component]
pub fn PlatformChip(
    name: String,
    icon: Option<String>,
    badge: Option<String>,
    muted: bool,
    #[props(default)] supported: bool,
    decorative: bool,
) -> Element {
    let class = match badge.as_deref() {
        Some("bring-up") => "platform-chip platform-chip--muted platform-chip--bringup",
        Some("roadmap") => "platform-chip platform-chip--muted platform-chip--roadmap",
        _ if supported => "platform-chip platform-chip--supported",
        _ if muted => "platform-chip platform-chip--muted",
        _ => "platform-chip",
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
