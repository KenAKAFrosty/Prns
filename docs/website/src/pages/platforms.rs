use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::PlatformChip;
use crate::platforms::{Group, Tier, GROUPS, PLATFORMS};
use crate::repository_docs::REPOSITORY_BLOB_BASE;
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
                                    supported: matches!(p.tier, Tier::Shipping | Tier::SdkPreview),
                                    decorative: false,
                                }
                            }
                        }
                        if *group == Group::Language {
                            p { class: "mt-4 max-w-3xl text-sm leading-relaxed text-soft",
                                "Rust and TypeScript/JavaScript are the paved application SDKs. SDK preview means the language adapter and live in-tree conformance journey are implemented while idiomatic public packaging remains active release work. "
                                a {
                                    href: format!("{REPOSITORY_BLOB_BASE}/docs/sdks.md"),
                                    target: "_blank",
                                    rel: "noopener",
                                    class: "font-medium text-accent hover:underline underline-offset-2",
                                    "Choose an SDK and see its exact readiness →"
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
