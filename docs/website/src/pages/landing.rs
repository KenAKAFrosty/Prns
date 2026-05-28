use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::routes::Route;

#[component]
pub fn Landing() -> Element {
    rsx! {
        section { class: "pt-8 pb-20",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                {t!("landing-kicker")}
            }
            h1 { class: "mt-4 text-4xl md:text-5xl font-semibold tracking-tight text-paper leading-[1.08]",
                {t!("landing-title")}
            }
            p { class: "mt-6 text-lg text-soft max-w-2xl leading-relaxed",
                {t!("landing-subtitle")}
            }
            div { class: "mt-10 flex flex-wrap gap-3",
                Link {
                    to: Route::CratesIndex {},
                    class: "inline-flex items-center gap-2 rounded-full bg-accent text-ink px-5 py-2.5 font-medium hover:bg-accent-strong transition-colors",
                    {t!("landing-cta-ethos")}
                    span { "→" }
                }
                Link {
                    to: Route::EthosPage {},
                    class: "inline-flex items-center gap-2 rounded-full border border-line/80 bg-layer/40 px-5 py-2.5 text-paper hover:border-accent/40 hover:text-accent transition-colors",
                    {t!("landing-cta-crates")}
                }
            }
        }

        section { class: "border-t border-line/60 pt-14 pb-2",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-mid",
                {t!("landing-quote-label")}
            }
            blockquote { class: "mt-3 text-xl md:text-2xl font-serif leading-snug text-paper italic max-w-3xl",
                {t!("landing-quote-body")}
            }
        }

        section { class: "mt-16 border-t border-line/60 pt-12",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-mid",
                {t!("standards-section-label")}
            }
            h2 { class: "mt-3 text-2xl md:text-3xl font-semibold tracking-tight text-paper",
                {t!("standards-section-title")}
            }
            div { class: "mt-8 grid gap-5 md:grid-cols-2 lg:grid-cols-4",
                StandardsCard {
                    label: t!("standards-license-label"),
                    headline: t!("standards-license-headline"),
                    body: t!("standards-license-body"),
                }
                StandardsCard {
                    label: t!("standards-coverage-label"),
                    headline: t!("standards-coverage-headline"),
                    body: t!("standards-coverage-body"),
                }
                StandardsCard {
                    label: t!("standards-core-label"),
                    headline: t!("standards-core-headline"),
                    body: t!("standards-core-body"),
                }
                StandardsCard {
                    label: t!("standards-verification-label"),
                    headline: t!("standards-verification-headline"),
                    body: t!("standards-verification-body"),
                }
            }
        }

        section { class: "mt-16",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                {t!("start-section-label")}
            }
            h2 { class: "mt-3 text-2xl md:text-3xl font-semibold tracking-tight text-paper",
                {t!("start-section-title")}
            }
            p { class: "mt-3 text-soft max-w-2xl leading-relaxed",
                {t!("start-section-lead")}
            }
            div { class: "mt-8 grid gap-4 md:grid-cols-2 lg:grid-cols-3",
                UseCaseCard {
                    headline: t!("start-daemon-headline"),
                    body: t!("start-daemon-body"),
                    code: t!("start-daemon-code"),
                    target_label: t!("start-daemon-target"),
                    crate_name: "personal-rnsd",
                }
                UseCaseCard {
                    headline: t!("start-mobile-headline"),
                    body: t!("start-mobile-body"),
                    code: t!("start-mobile-code"),
                    target_label: t!("start-mobile-target"),
                    crate_name: "personal-rns-ffi",
                }
                UseCaseCard {
                    headline: t!("start-game-headline"),
                    body: t!("start-game-body"),
                    code: t!("start-game-code"),
                    target_label: t!("start-game-target"),
                    crate_name: "personal-rns-ffi",
                }
                UseCaseCard {
                    headline: t!("start-embedded-headline"),
                    body: t!("start-embedded-body"),
                    code: t!("start-embedded-code"),
                    target_label: t!("start-embedded-target"),
                    crate_name: "personal-rns",
                }
                UseCaseCard {
                    headline: t!("start-web-headline"),
                    body: t!("start-web-body"),
                    code: t!("start-web-code"),
                    target_label: t!("start-web-target"),
                    crate_name: "personal-rns",
                }
                UseCaseCard {
                    headline: t!("start-rust-headline"),
                    body: t!("start-rust-body"),
                    code: t!("start-rust-code"),
                    target_label: t!("start-rust-target"),
                    crate_name: "personal-rnsd",
                }
                UseCaseCard {
                    headline: t!("start-lxmf-headline"),
                    body: t!("start-lxmf-body"),
                    code: t!("start-lxmf-code"),
                    target_label: t!("start-lxmf-target"),
                    crate_name: "personal-lxmf",
                }
            }
        }
    }
}

#[component]
fn StandardsCard(label: String, headline: String, body: String) -> Element {
    rsx! {
        div { class: "rounded-card border border-line/60 bg-layer/40 p-5",
            p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-accent",
                "{label}"
            }
            p { class: "mt-2 text-base font-semibold text-paper tracking-tight",
                "{headline}"
            }
            p { class: "mt-2 text-sm text-soft leading-relaxed",
                "{body}"
            }
        }
    }
}

#[component]
fn UseCaseCard(
    headline: String,
    body: String,
    code: String,
    target_label: String,
    crate_name: &'static str,
) -> Element {
    let trimmed = code.trim().to_string();
    rsx! {
        Link {
            to: Route::SingleCrate { name: crate_name.to_string() },
            class: "group block rounded-card border border-line/60 bg-layer/40 p-5 hover:border-accent/40 hover:-translate-y-px transition-all",
            p { class: "text-base font-semibold text-paper leading-snug",
                "{headline}"
            }
            p { class: "mt-2 text-sm text-soft leading-relaxed",
                "{body}"
            }
            if !trimmed.is_empty() {
                pre { class: "mt-3 bg-surface/80 border border-line/40 rounded-md px-3 py-2 text-xs font-mono text-paper overflow-x-auto whitespace-pre",
                    code { "{trimmed}" }
                }
            }
            p { class: "mt-4 font-mono text-xs text-mid group-hover:text-accent transition-colors",
                "→ {target_label}"
            }
        }
    }
}
