use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::PlatformChip;
use crate::platforms::{Tier, GROUPS, PLATFORMS};
use crate::routes::Route;

/// The dedicated "what it runs on" page: the marquee's contents, but static and
/// scannable (Ctrl-F friendly), grouped by kind, with shipping vs. roadmap
/// called out. Reached from the hero marquee, which links here.
#[component]
pub fn PlatformsPage() -> Element {
    rsx! {
        header { class: "mb-10",
            Link {
                to: Route::Landing {},
                class: "text-sm text-soft hover:text-accent transition-colors",
                "← Home"
            }
            p { class: "mt-6 text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                {t!("landing-platforms-label")}
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                {t!("platforms-title")}
            }
            p { class: "mt-4 text-soft max-w-2xl leading-relaxed",
                {t!("platforms-lead")}
            }
            div { class: "mt-6 flex flex-wrap items-center gap-5 text-xs text-mid",
                span { class: "inline-flex items-center gap-2",
                    PlatformChip { name: "ESP32-C6".to_string(), icon: Some("espressif".to_string()), soon: false, decorative: true }
                    {t!("platforms-legend-shipping")}
                }
                span { class: "inline-flex items-center gap-2",
                    PlatformChip { name: "ESP32-S3".to_string(), icon: Some("espressif".to_string()), soon: true, decorative: true }
                    {t!("platforms-legend-roadmap")}
                }
            }
        }

        div { class: "flex flex-col gap-8",
            for group in GROUPS.iter() {
                div { key: "{group.label()}",
                    p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-mid",
                        "{group.label()}"
                    }
                    div { class: "mt-3 flex flex-wrap gap-2",
                        for p in PLATFORMS.iter().filter(|p| p.group == *group) {
                            PlatformChip {
                                key: "{p.name}",
                                name: p.name.to_string(),
                                icon: p.icon.map(str::to_string),
                                soon: p.tier == Tier::Roadmap,
                                decorative: false,
                            }
                        }
                    }
                }
            }
        }
    }
}
