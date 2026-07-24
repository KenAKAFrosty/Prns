use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::MarkdownBody;

// Mirrors the repo-root CONTRIBUTING.md so the site and the repo never drift.
const CONTRIBUTING_MD: &str = include_str!("../../../../CONTRIBUTING.md");

#[component]
pub fn ContributingPage() -> Element {
    rsx! {
        header { class: "mb-10",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                {t!("contributing-kicker")}
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                {t!("contributing-title")}
            }
            p { class: "mt-4 text-soft max-w-2xl leading-relaxed",
                {t!("contributing-lead")}
            }
        }
        MarkdownBody { source: contributing_markup() }
    }
}

fn contributing_markup() -> String {
    CONTRIBUTING_MD
        .strip_prefix("# Contributing\n")
        .unwrap_or(CONTRIBUTING_MD)
        .replace(
            "](README.md#license)",
            "](https://github.com/KenAKAFrosty/Prns/blob/main/README.md#license)",
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_copy_uses_its_page_heading_and_resolves_repo_links() {
        let markup = contributing_markup();

        assert!(!markup.starts_with("# Contributing"));
        assert!(!markup.contains("](README.md#license)"));
        assert!(
            markup.contains("](https://github.com/KenAKAFrosty/Prns/blob/main/README.md#license)")
        );
    }
}
