use dioxus::prelude::*;

use crate::repository_docs::{GuideSection, GUIDE_DOCUMENTS};

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
                "Choose the result you want. Each guide lives in the source repository and opens on GitHub, with the same content a clone gives you."
            }
        }
        for (section, title, introduction) in sections {
            section { class: "mt-12",
                h2 { class: "text-2xl font-semibold text-paper", "{title}" }
                p { class: "mt-2 text-sm text-soft max-w-2xl", "{introduction}" }
                div { class: "mt-5 grid gap-4 md:grid-cols-2",
                    for document in GUIDE_DOCUMENTS.iter().filter(|document| document.section == section) {
                        a {
                            key: "{document.source_path}",
                            href: document.github_url(),
                            target: "_blank",
                            rel: "noopener",
                            class: "block rounded-card border border-line/60 bg-layer/40 p-5 hover:border-accent/40 hover:-translate-y-px transition-all",
                            h3 { class: "text-lg font-semibold text-paper", "{document.title}" }
                            p { class: "mt-2 text-sm leading-relaxed text-soft",
                                "{document.summary}"
                            }
                            p { class: "mt-3 text-sm text-accent", "Read on GitHub →" }
                        }
                    }
                }
            }
        }
    }
}
