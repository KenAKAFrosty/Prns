use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::routes::Route;

#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let _ = segments;
    rsx! {
        div { class: "py-20 text-center",
            p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-mid", "404" }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold text-paper",
                {t!("not-found-title")}
            }
            p { class: "mt-3 text-soft", "The requested address does not match a published page." }
            Link {
                to: Route::Landing {},
                class: "inline-block mt-8 rounded-full bg-accent text-ink px-5 py-2.5 font-medium hover:bg-accent-strong transition-colors",
                {t!("not-found-cta")}
            }
        }
    }
}
