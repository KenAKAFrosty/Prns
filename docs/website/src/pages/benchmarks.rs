use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::routes::Route;

/// Performance page. Methodology-first scaffold: it states what gets measured
/// and how, honestly, with the actual figures to drop in as the suite settles.
/// Linked from the "Performance" standards card on the landing page.
#[component]
pub fn BenchmarksPage() -> Element {
    rsx! {
        header { class: "mb-10",
            Link {
                to: Route::Landing {},
                class: "text-sm text-soft hover:text-accent transition-colors",
                "← Home"
            }
            p { class: "mt-6 text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                {t!("benchmarks-kicker")}
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                {t!("benchmarks-title")}
            }
            p { class: "mt-4 text-soft max-w-2xl leading-relaxed",
                {t!("benchmarks-lead")}
            }
        }

        div { class: "grid gap-5 md:grid-cols-2",
            div { class: "rounded-card border border-line/60 bg-layer/40 p-5",
                p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-accent",
                    "What we measure"
                }
                ul { class: "mt-3 flex flex-col gap-2 text-sm text-soft leading-relaxed",
                    li { "Throughput: packets and bytes per second through the engine." }
                    li { "Latency: per-packet processing time, median and worst case." }
                    li { "Memory: peak footprint and allocation count (the core makes none)." }
                    li { "Binary size: what the engine costs on a constrained target." }
                }
            }
            div { class: "rounded-card border border-line/60 bg-layer/40 p-5",
                p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-accent",
                    "How we measure"
                }
                ul { class: "mt-3 flex flex-col gap-2 text-sm text-soft leading-relaxed",
                    li { "A deterministic harness in the repo, runnable on any machine." }
                    li { "Compared against the RNS reference where the comparison is fair." }
                    li { "Run on the hardware it claims, down to the microcontroller." }
                    li { "Reported with the commit and toolchain, so a number reproduces." }
                }
            }
        }

        p { class: "mt-8 text-sm text-mid",
            "Full results are landing here as the suite stabilizes."
        }
    }
}
