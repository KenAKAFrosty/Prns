use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::MarkdownBody;

// Single source of truth lives at docs/build-ethos.md alongside this site.
const ETHOS_MD: &str = include_str!("../../../build-ethos.md");

#[component]
pub fn EthosPage() -> Element {
    rsx! {
        header { class: "mb-10",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                {t!("ethos-kicker")}
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                {t!("ethos-title")}
            }
            p { class: "mt-4 text-soft max-w-2xl leading-relaxed",
                {t!("ethos-lead")}
            }
        }
        MarkdownBody { source: ETHOS_MD }
    }
}
