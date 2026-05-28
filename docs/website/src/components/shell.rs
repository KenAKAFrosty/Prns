use dioxus::prelude::*;

use super::{Footer, TopNav};

#[component]
pub fn Shell() -> Element {
    rsx! {
        div { class: "min-h-screen flex flex-col bg-ink text-paper",
            TopNav {}
            main { class: "flex-1 w-full max-w-5xl mx-auto px-6 pt-12 pb-24",
                Outlet::<crate::routes::Route> {}
            }
            Footer {}
        }
    }
}
