use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::PlatformChip;
use crate::platforms::{Group, GROUPS, PLATFORMS};
use crate::routes::Route;

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
                    PlatformChip { name: "Runtime".to_string(), icon: None, badge: None, muted: false, decorative: true }
                    {t!("platforms-legend-runtime")}
                }
                span { class: "inline-flex items-center gap-2",
                    PlatformChip { name: "ESP32-C6".to_string(), icon: Some("espressif".to_string()), badge: Some("bring-up".to_string()), muted: true, decorative: true }
                    {t!("platforms-legend-bringup")}
                }
                span { class: "inline-flex items-center gap-2",
                    PlatformChip { name: "RP2040".to_string(), icon: Some("raspberrypi".to_string()), badge: Some("roadmap".to_string()), muted: true, decorative: true }
                    {t!("platforms-legend-roadmap")}
                }
            }
        }

        section { class: "mt-10",
            h2 { class: "text-2xl font-semibold tracking-tight text-paper",
                {t!("platforms-runtime-title")}
            }
            p { class: "mt-3 text-soft max-w-3xl leading-relaxed",
                {t!("platforms-runtime-lead")}
            }

            div { class: "mt-8 flex flex-col gap-8",
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
                                    badge: p.tier.chip_badge().map(str::to_string),
                                    muted: p.tier.muted(),
                                    decorative: false,
                                }
                            }
                        }
                        if *group == Group::Microcontroller {
                            Link {
                                to: Route::FlashPage {},
                                class: "mt-4 inline-flex items-center gap-2 rounded-full border border-accent/40 bg-layer/40 px-4 py-2 text-sm font-medium text-accent hover:border-accent hover:bg-accent/10 transition-colors",
                                {t!("platforms-board-support-link")}
                            }
                        }
                    }
                }
            }
        }
    }
}
