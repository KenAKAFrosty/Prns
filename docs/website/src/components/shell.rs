use dioxus::document;
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use dioxus_i18n::t;
use unic_langid::langid;

use super::{Footer, TopNav};

#[component]
fn EnglishDocsNotice() -> Element {
    let i18n = i18n();
    if i18n.language() == langid!("en-US") {
        return rsx! {};
    }
    rsx! {
        aside { class: "border-b border-line/60 bg-surface/60 px-6 py-2 text-center text-xs text-soft",
            {t!("site-early-english-note")}
        }
    }
}

#[component]
pub fn Shell() -> Element {
    let route = use_route::<crate::routes::Route>();
    let mut last_route = use_signal(|| None::<crate::routes::Route>);
    use_effect(use_reactive!(|route| {
        let arrived_by_navigation = last_route
            .peek()
            .as_ref()
            .is_some_and(|previous| *previous != route);
        if arrived_by_navigation {
            document::eval("window.scrollTo(0, 0);");
        }
        last_route.set(Some(route));
    }));

    rsx! {
        crate::seo::PageMetadata { key: "{route}", route }
        div { class: "min-h-screen flex flex-col bg-ink text-paper",
            EnglishDocsNotice {}
            TopNav {}
            crate::local_development::LocalDevelopmentBanner {}
            main { class: "flex-1 w-full max-w-5xl mx-auto px-6 pt-12 pb-24",
                Outlet::<crate::routes::Route> {}
            }
            Footer {}
        }
    }
}
