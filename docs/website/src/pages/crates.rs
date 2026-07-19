use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::MarkdownBody;
use crate::routes::Route;

struct CrateMeta {
    name: &'static str,
    role_key: &'static str,
    blurb_key: &'static str,
    body: &'static str,
}

const CRATES: &[CrateMeta] = &[
    CrateMeta {
        name: "personal-rns",
        role_key: "crate-rns-role",
        blurb_key: "crate-rns-blurb",
        body: include_str!("../../content/crates/personal-rns.md"),
    },
    CrateMeta {
        name: "prnsd",
        role_key: "crate-rnsd-role",
        blurb_key: "crate-rnsd-blurb",
        body: include_str!("../../content/crates/prnsd.md"),
    },
];

#[component]
pub fn CratesIndex() -> Element {
    rsx! {
        header { class: "mb-10",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                {t!("crates-kicker")}
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                {t!("crates-title")}
            }
            p { class: "mt-4 text-soft max-w-2xl leading-relaxed",
                {t!("crates-lead")}
            }
        }

        div { class: "grid gap-4 md:grid-cols-2",
            for c in CRATES.iter() {
                Link {
                    key: "{c.name}",
                    to: Route::SingleCrate { name: c.name.to_string() },
                    class: "block rounded-card border border-line/60 bg-layer/40 p-5 hover:border-accent/40 hover:-translate-y-px transition-all",
                    p { class: "font-mono text-sm text-accent", "{c.name}" }
                    p { class: "mt-2 text-sm font-medium text-paper", {t!(c.role_key)} }
                    p { class: "mt-2 text-sm text-soft leading-relaxed", {t!(c.blurb_key)} }
                    p { class: "mt-3 text-xs text-mid", {t!("crates-card-cta")} }
                }
            }
        }
    }
}

#[component]
pub fn SingleCrate(name: String) -> Element {
    let crate_meta = CRATES.iter().find(|c| c.name == name);

    match crate_meta {
        Some(c) => rsx! {
            header { class: "mb-8",
                Link {
                    to: Route::CratesIndex {},
                    class: "text-sm text-soft hover:text-accent transition-colors",
                    {format!("← {}", t!("crates-back"))}
                }
                p { class: "mt-6 font-mono text-accent", "{c.name}" }
                h1 { class: "mt-1 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                    {t!(c.role_key)}
                }
                p { class: "mt-3 text-soft max-w-2xl leading-relaxed", {t!(c.blurb_key)} }
            }
            MarkdownBody { source: c.body.to_string() }
        },
        None => rsx! {
            h1 { class: "text-3xl font-semibold", {t!("crates-not-found")} }
            p { class: "mt-3 text-soft", "{name}" }
            Link {
                to: Route::CratesIndex {},
                class: "inline-block mt-6 text-accent hover:underline",
                {format!("← {}", t!("crates-back"))}
            }
        },
    }
}
