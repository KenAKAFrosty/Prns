use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use dioxus_i18n::t;
use unic_langid::langid;

use crate::components::PlatformChip;
use crate::platforms::PLATFORMS;
use crate::routes::Route;

/// The eyebrow's last word animates. It opens on "yours" (plain), rotates
/// through seven qualities, then rests back on "yours" (underlined). The first
/// and last entries are both "yours" on purpose.
///
/// Coupled to the `kicker-rotate` keyframes in tailwind.css, authored for
/// exactly this many words (one 1.4rem step each, resting on the last). If you
/// add or remove a word, update the keyframe stops, the list's base
/// `transform: translateY(...)`, and the underline delay there (then rebuild
/// the compiled public/assets/tailwind.css with `npm run build:css`).
const KICKER_WORDS: &[&str] = &[
    "yours", "resilient", "fast", "open", "everywhere", "unstoppable",
    "off-grid", "private", "yours",
];

#[component]
pub fn Landing() -> Element {
    // Two bits of the hero are English-only: the rotating-last-word eyebrow and
    // the green "written in safe Rust" second line of the title. Other locales
    // word both phrases differently, so they get the plain kicker and title.
    let i18n = i18n();
    let is_english = i18n.language() == langid!("en-US");
    let resting_word = KICKER_WORDS.last().copied().unwrap_or("yours");

    rsx! {
        section { class: "pt-8 pb-20",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                if is_english {
                    {t!("landing-kicker-prefix")}
                    " "
                    span { class: "kicker-rotator", "aria-hidden": "true",
                        span { class: "kicker-rotator__window",
                            span { class: "kicker-rotator__list",
                                for (i, word) in KICKER_WORDS.iter().enumerate() {
                                    span {
                                        key: "{i}-{word}",
                                        class: "kicker-rotator__word",
                                        "{word}"
                                    }
                                }
                            }
                        }
                        span { class: "kicker-rotator__rule", "{resting_word}" }
                    }
                    // The animation is decorative; expose the resting word to
                    // screen readers so the phrase still reads "…for the people".
                    span { class: "kicker-sr-only", "{resting_word}" }
                } else {
                    {t!("landing-kicker")}
                }
            }
            h1 { class: "mt-4 text-4xl md:text-5xl font-semibold tracking-tight text-paper leading-[1.08]",
                if is_english {
                    {t!("landing-title-lead")}
                    br {}
                    span { class: "text-accent", {t!("landing-title-accent")} }
                } else {
                    {t!("landing-title")}
                }
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
                    to: Route::ContributingPage {},
                    class: "inline-flex items-center gap-2 rounded-full border border-line/80 bg-layer/40 px-5 py-2.5 text-paper hover:border-accent/40 hover:text-accent transition-colors",
                    {t!("landing-cta-contributing")}
                }
            }

            // The whole strip links to the dedicated, scannable platforms page.
            // A marquee is lovely but useless for "does it run on MY thing?".
            Link {
                to: Route::PlatformsPage {},
                class: "group mt-8 flex items-center gap-4",
                span { class: "text-[0.7rem] font-semibold tracking-[0.18em] uppercase text-mid group-hover:text-accent transition-colors",
                    {t!("landing-platforms-label")}
                }
                div { class: "platform-marquee flex-1",
                    div { class: "platform-marquee__track",
                        for p in PLATFORMS.iter() {
                            PlatformChip {
                                key: "{p.name}",
                                name: p.name.to_string(),
                                icon: p.icon.map(str::to_string),
                                soon: false,
                                decorative: false,
                            }
                        }
                        for p in PLATFORMS.iter() {
                            PlatformChip {
                                key: "{p.name}-dup",
                                name: p.name.to_string(),
                                icon: p.icon.map(str::to_string),
                                soon: false,
                                decorative: true,
                            }
                        }
                    }
                }
                span { class: "text-xs text-mid group-hover:text-accent transition-colors",
                    {t!("landing-platforms-cta")}
                }
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
                    label: t!("standards-safety-label"),
                    headline: t!("standards-safety-headline"),
                    body: t!("standards-safety-body"),
                }
                StandardsCard {
                    label: t!("standards-correctness-label"),
                    headline: t!("standards-correctness-headline"),
                    body: t!("standards-correctness-body"),
                }
                // Performance is the one card that goes deeper: it links to the
                // benchmarks page where the actual numbers live.
                Link {
                    to: Route::BenchmarksPage {},
                    class: "group block rounded-card border border-line/60 bg-layer/40 p-5 hover:border-accent/40 hover:-translate-y-px transition-all",
                    p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-accent",
                        {t!("standards-benchmarked-label")}
                    }
                    p { class: "mt-2 text-base font-semibold text-paper tracking-tight",
                        {t!("standards-benchmarked-headline")}
                    }
                    p { class: "mt-2 text-sm text-soft leading-relaxed",
                        {t!("standards-benchmarked-body")}
                    }
                    p { class: "mt-3 font-mono text-xs text-mid group-hover:text-accent transition-colors",
                        {t!("standards-benchmarked-cta")}
                    }
                }
            }
        }

        section { class: "mt-16 border-t border-line/60 pt-14 pb-2",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-mid",
                {t!("landing-quote-label")}
            }
            blockquote { class: "mt-3 text-lg md:text-xl font-serif leading-snug text-paper italic max-w-3xl",
                {t!("landing-quote-body")}
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
