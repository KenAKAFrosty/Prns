use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::PlatformChip;
use crate::platforms::{Group, Tier, GROUPS, PLATFORMS};
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
        }

        section {
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
                                    badge: p.tier.chip_badge().map(str::to_string),
                                    muted: p.tier.muted(),
                                    supported: p.tier == Tier::Shipping,
                                    decorative: false,
                                }
                            }
                        }
                        if *group == Group::Microcontroller {
                            Link {
                                to: Route::FlashPage {},
                                class: "platform-board-link mt-4 inline-flex text-sm font-medium",
                                {t!("platforms-board-support-link")}
                            }
                        }
                    }
                }
            }
        }
    }
}
