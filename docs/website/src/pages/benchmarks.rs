use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::MarkdownBody;
use crate::routes::Route;

// Generated from the result substrate by `benchmarks/render_results`; GitHub and this
// page render the same files. Tests hold the index, host pages, and published assets
// together so a newly published host cannot leave a dead site route behind.
const INDEX_MD: &str = include_str!("../../../../benchmarks/RESULTS.md");
pub(crate) const HOST_PAGES: &[(&str, &str)] = &[
    (
        "aarch64-apple-darwin",
        include_str!("../../../../benchmarks/RESULTS-aarch64-apple-darwin.md"),
    ),
    (
        "x86_64-pc-windows-msvc",
        include_str!("../../../../benchmarks/RESULTS-x86_64-pc-windows-msvc.md"),
    ),
    (
        "x86_64-unknown-linux-gnu",
        include_str!("../../../../benchmarks/RESULTS-x86_64-unknown-linux-gnu.md"),
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

        section {
            p { class: "mb-6",
                a {
                    href: "https://github.com/KenAKAFrosty/Prns/blob/main/benchmarks/README.md",
                    target: "_blank",
                    rel: "noopener",
                    class: "text-accent hover:underline",
                    "Run and interpret the benchmarks locally →"
                }
            }
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
    rewrite_index(INDEX_MD)
}

/// Repoint the index's result links at site routes and drop its own title, which the page supplies itself.
///
/// The source is `include_str!`d from a checkout, so its newlines are whatever Git wrote: `\n` on Linux and macOS, `\r\n` on a default Windows clone.
///
/// Normalise once here rather than spelling both forms into every pattern below.
fn rewrite_index(source: &str) -> String {
    let source = source.replace("\r\n", "\n");
    let mut out = String::with_capacity(source.len());
    let mut rest = source.as_str();
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

/// Repoint a host page's back-link at the site index and make asset URLs absolute.
///
/// Relative assets would resolve under `/benchmarks/<host>/`, not `/assets/`.
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
        assert!(md.contains("](/benchmarks/x86_64-unknown-linux-gnu)"));
        assert!(!md.contains("](RESULTS-aarch64-apple-darwin.md)"));
        assert!(!md.contains("# Benchmark results"));
    }

    /// A Windows checkout hands `include_str!` CRLF newlines, which used to defeat the title strip and leave the page rendering its heading twice.
    ///
    /// Assert it directly so the guarantee does not depend on which platform ran the suite.
    #[test]
    fn rewrites_the_index_whatever_newlines_the_checkout_used() {
        const SOURCE: &str = "<!-- generated -->\n# Benchmark results\n\nSee [macOS](RESULTS-aarch64-apple-darwin.md).\n";

        for (label, source) in [
            ("lf", SOURCE.to_string()),
            ("crlf", SOURCE.replace('\n', "\r\n")),
        ] {
            let md = rewrite_index(&source);
            assert!(
                !md.contains("# Benchmark results"),
                "{label}: the page supplies its own title, so the document's must be removed"
            );
            assert!(
                md.contains("](/benchmarks/aarch64-apple-darwin)"),
                "{label}: result links must point at site routes"
            );
        }
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
