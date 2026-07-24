use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::MarkdownBody;
use crate::routes::Route;

// Generated from the result substrate by `benchmarks/render_results`; GitHub and this
// page render the same files. Tests hold the index, host pages, and published assets
// together so a newly published host cannot leave a dead site route behind.
const INDEX_MD: &str = include_str!("../../../../benchmarks/RESULTS.md");
const HOST_PAGES: &[(&str, &str)] = &[
    (
        "aarch64-apple-darwin",
        include_str!("../../../../benchmarks/RESULTS-aarch64-apple-darwin.md"),
    ),
    (
        "x86_64-pc-windows-msvc",
        include_str!("../../../../benchmarks/RESULTS-x86_64-pc-windows-msvc.md"),
    ),
];

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
                    li { "Conformance: every operation accounted for at both ends." }
                    li { "Throughput and latency: delivered work and proof-backed round trips." }
                    li { "Memory: initiator and responder peak resident set size." }
                    li { "Energy: optional net processor energy, split by role when measurable." }
                }
            }
            div { class: "rounded-card border border-line/60 bg-layer/40 p-5",
                p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-accent",
                    "How we measure"
                }
                ul { class: "mt-3 flex flex-col gap-2 text-sm text-soft leading-relaxed",
                    li { "Three 30-second release samples per published cell." }
                    li { "The same four Prns/reference directional pairings for every scenario." }
                    li { "Compared with a verified Cython-compiled RNS 1.4.0 reference." }
                    li { "Stamped with machine, commit, toolchain, and reference provenance." }
                }
            }
        }

        section { class: "mt-10",
            MarkdownBody { source: index_markup() }
        }
    }
}

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

fn index_markup() -> String {
    let mut out = String::with_capacity(INDEX_MD.len());
    let mut rest = INDEX_MD;
    const PREFIX: &str = "](RESULTS-";
    const SUFFIX: &str = ".md)";

    while let Some(start) = rest.find(PREFIX) {
        out.push_str(&rest[..start]);
        let candidate = &rest[start + PREFIX.len()..];
        let Some(end) = candidate.find(SUFFIX) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let slug = &candidate[..end];
        out.push_str("](/benchmarks/");
        out.push_str(slug);
        out.push(')');
        rest = &candidate[end + SUFFIX.len()..];
    }
    out.push_str(rest);
    out.replacen("# Benchmark results\n\n", "", 1)
}

/// Repoint a host page's back-link at the site index, and make asset URLs absolute
/// (relative assets would resolve under `/benchmarks/<host>/`, not `/assets/`).
fn host_markup(md: &str) -> String {
    md.replace("](RESULTS.md)", "](/benchmarks)")
        .replace("src=\"assets/", "src=\"/assets/")
        .replace("srcset=\"assets/", "srcset=\"/assets/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn rewrites_results_links_to_site_routes() {
        let md = index_markup();

        assert!(md.contains("](/benchmarks/aarch64-apple-darwin)"));
        assert!(md.contains("](/benchmarks/x86_64-pc-windows-msvc)"));
        assert!(!md.contains("](RESULTS-aarch64-apple-darwin.md)"));
        assert!(!md.contains("# Benchmark results"));
    }

    #[test]
    fn includes_each_measured_host_page() {
        let mut indexed = BTreeSet::new();
        let mut rest = INDEX_MD;
        const PREFIX: &str = "](RESULTS-";
        const SUFFIX: &str = ".md)";
        while let Some(start) = rest.find(PREFIX) {
            let candidate = &rest[start + PREFIX.len()..];
            let end = candidate
                .find(SUFFIX)
                .expect("every benchmark result link has a Markdown suffix");
            indexed.insert(&candidate[..end]);
            rest = &candidate[end + SUFFIX.len()..];
        }
        let published = HOST_PAGES
            .iter()
            .map(|(host, _)| *host)
            .collect::<BTreeSet<_>>();

        assert_eq!(published, indexed);
    }

    #[test]
    fn host_assets_are_absolute_and_published() {
        let public_assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("public/assets");
        for (_, source) in HOST_PAGES {
            let markup = host_markup(source);
            assert!(!markup.contains("\"assets/"));

            for marker in ["src=\"/assets/", "srcset=\"/assets/"] {
                let mut rest = markup.as_str();
                while let Some(start) = rest.find(marker) {
                    let candidate = &rest[start + marker.len()..];
                    let end = candidate
                        .find('"')
                        .expect("benchmark asset reference is quoted");
                    assert!(
                        public_assets.join(&candidate[..end]).is_file(),
                        "missing public benchmark asset {}",
                        &candidate[..end],
                    );
                    rest = &candidate[end + 1..];
                }
            }
        }
    }
}
