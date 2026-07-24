use dioxus::prelude::*;

use crate::components::MarkdownBody;
use crate::repository_docs::{guide, repository_markup, GUIDE_DOCUMENTS};
use crate::routes::Route;

#[component]
pub fn GuidesIndex() -> Element {
    rsx! {
        header { class: "mb-10",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                "Clone-first documentation"
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                "Guides"
            }
            p { class: "mt-4 text-soft max-w-2xl leading-relaxed",
                "These pages are compiled from the repository's canonical Markdown. The clone and the site teach the same workflows."
            }
        }
        div { class: "grid gap-4 md:grid-cols-2",
            for document in GUIDE_DOCUMENTS {
                Link {
                    key: "{document.slug}",
                    to: Route::GuidePage { slug: document.slug.to_string() },
                    class: "block rounded-card border border-line/60 bg-layer/40 p-5 hover:border-accent/40 hover:-translate-y-px transition-all",
                    h2 { class: "text-lg font-semibold text-paper", "{document.title}" }
                    p { class: "mt-2 font-mono text-xs text-mid",
                        "{document.source_path} · {document.route}"
                    }
                    p { class: "mt-3 text-sm text-accent", "Read locally →" }
                }
            }
        }
    }
}

#[component]
pub fn GuidePage(slug: String) -> Element {
    match guide(&slug) {
        Some(document) => match repository_markup(document.source_path, document.source, true) {
            Ok(markup) => rsx! {
                header { class: "mb-8",
                    Link {
                        to: Route::GuidesIndex {},
                        class: "text-sm text-soft hover:text-accent transition-colors",
                        "← Guides"
                    }
                    p { class: "mt-6 font-mono text-xs text-accent", "{document.source_path}" }
                    h1 { class: "mt-2 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                        "{document.title}"
                    }
                }
                MarkdownBody { source: markup }
            },
            Err(error) => rsx! {
                h1 { class: "text-2xl font-semibold text-paper", "Guide link error" }
                p { class: "mt-3 text-soft", "{error}" }
            },
        },
        None => rsx! {
            h1 { class: "text-2xl font-semibold text-paper", "Guide not found" }
            p { class: "mt-3 text-soft", "{slug}" }
            Link {
                to: Route::GuidesIndex {},
                class: "inline-block mt-6 text-accent hover:underline",
                "← Guides"
            }
        },
    }
}
