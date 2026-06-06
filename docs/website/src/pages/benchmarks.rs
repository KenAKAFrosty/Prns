use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::MarkdownBody;
use crate::routes::Route;

// Generated from the result substrate by `benchmarks/render_results`; GitHub and this
// page render the same files, so the tables can't drift. The index links to a per-host
// page; add a line to HOST_PAGES when a new host's results land.
const INDEX_MD: &str = include_str!("../../../../benchmarks/RESULTS.md");
const HOST_PAGES: &[(&str, &str)] = &[
    (
        "aarch64-apple-darwin",
        include_str!("../../../../benchmarks/RESULTS-aarch64-apple-darwin.md"),
    ),
    (
        "x86_64-unknown-linux-gnu",
        include_str!("../../../../benchmarks/RESULTS-x86_64-unknown-linux-gnu.md"),
    ),
];

/// Performance index: the methodology, then the per-host results table linking out to
/// each host's own page. Linked from the "Performance" standards card on the landing page.
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

        section { class: "mt-10",
            MarkdownBody { source: index_markup() }
        }
    }
}

/// One host's results. `host` is the route segment (a target triple).
#[component]
pub fn BenchmarksHostPage(host: String) -> Element {
    let body = HOST_PAGES
        .iter()
        .find(|(slug, _)| *slug == host)
        .map(|(_, md)| host_markup(md));
    rsx! {
        header { class: "mb-8",
            Link {
                to: Route::BenchmarksPage {},
                class: "text-sm text-soft hover:text-accent transition-colors",
                "← Benchmarks"
            }
        }
        if let Some(md) = body {
            MarkdownBody { source: md }
        } else {
            h1 { class: "text-2xl font-semibold text-paper", "No results for this host" }
            p { class: "mt-3 text-soft", "Nothing recorded for \"{host}\" yet." }
        }
    }
}

/// Repoint the index's per-host `RESULTS-<host>.md` links (GitHub-relative) at the site routes.
fn index_markup() -> String {
    let mut md = INDEX_MD.to_string();
    for (slug, _) in HOST_PAGES {
        md = md.replace(
            &format!("](RESULTS-{slug}.md)"),
            &format!("](/benchmarks/{slug})"),
        );
    }
    md
}

/// Repoint a host page's back-link at the site index, and make the icon `src` absolute
/// (a relative `assets/` would resolve under `/benchmarks/<host>/` here, not `/assets/`).
fn host_markup(md: &str) -> String {
    md.replace("](RESULTS.md)", "](/benchmarks)")
        .replace("src=\"assets/", "src=\"/assets/")
}
