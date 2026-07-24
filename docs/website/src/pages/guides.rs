use dioxus::prelude::*;

use crate::components::MarkdownBody;
use crate::repository_docs::{guide, repository_markup, GuideSection, GUIDE_DOCUMENTS};
use crate::routes::Route;

#[component]
pub fn GuidesIndex() -> Element {
    let sections = [
        (
            GuideSection::Start,
            "Start and build",
            "The shortest paths from a source checkout to a useful result.",
        ),
        (
            GuideSection::Operate,
            "Operate nodes",
            "Configuration, utilities, observability, and supported node platforms.",
        ),
        (
            GuideSection::Contribute,
            "Contribute",
            "Tools for changing, documenting, profiling, and verifying Prns.",
        ),
        (
            GuideSection::Maintain,
            "Maintain releases",
            "Deeper evidence, qualification, and release-custody references.",
        ),
    ];

    rsx! {
        header { class: "mb-10",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                "Practical documentation"
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                "Guides"
            }
            p { class: "mt-4 text-soft max-w-2xl leading-relaxed",
                "Choose the result you want. These guides are available both here and in the source repository, with the same commands and content."
            }
        }
        for (section, title, introduction) in sections {
            section { class: "mt-12",
                h2 { class: "text-2xl font-semibold text-paper", "{title}" }
                p { class: "mt-2 text-sm text-soft max-w-2xl", "{introduction}" }
                div { class: "mt-5 grid gap-4 md:grid-cols-2",
                    if section == GuideSection::Start {
                        Link {
                            to: Route::SingleCrate { name: "prnsd".to_string() },
                            class: "block rounded-card border border-line/60 bg-layer/40 p-5 hover:border-accent/40 hover:-translate-y-px transition-all",
                            h3 { class: "text-lg font-semibold text-paper", "Run and inspect a node" }
                            p { class: "mt-2 text-sm leading-relaxed text-soft",
                                "Start an isolated prnsd node, inspect its interfaces, attach to logs, and stop it cleanly."
                            }
                            p { class: "mt-3 text-sm text-accent", "Open the prnsd guide →" }
                        }
                        Link {
                            to: Route::SingleCrate { name: "personal-rns".to_string() },
                            class: "block rounded-card border border-line/60 bg-layer/40 p-5 hover:border-accent/40 hover:-translate-y-px transition-all",
                            h3 { class: "text-lg font-semibold text-paper", "Build a Rust application" }
                            p { class: "mt-2 text-sm leading-relaxed text-soft",
                                "Run two real nodes, then learn the recipe, handles, events, features, and API contract."
                            }
                            p { class: "mt-3 text-sm text-accent", "Open the Rust guide →" }
                        }
                    }
                    for document in GUIDE_DOCUMENTS.iter().filter(|document| document.section == section) {
                        Link {
                            key: "{document.slug}",
                            to: Route::GuidePage { slug: document.slug.to_string() },
                            class: "block rounded-card border border-line/60 bg-layer/40 p-5 hover:border-accent/40 hover:-translate-y-px transition-all",
                            h3 { class: "text-lg font-semibold text-paper", "{document.title}" }
                            p { class: "mt-2 text-sm leading-relaxed text-soft",
                                "{document.summary}"
                            }
                            p { class: "mt-3 text-sm text-accent", "Read guide →" }
                        }
                    }
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
                    p { class: "mt-6 text-xs font-semibold tracking-[0.18em] uppercase text-accent",
                        "Canonical repository guide"
                    }
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
