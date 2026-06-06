use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::routes::Route;

use super::{LanguageSwitcher, PrnsMark};

#[component]
pub fn TopNav() -> Element {
    rsx! {
        header { class: "border-b border-line/60 backdrop-blur-md sticky top-0 z-30 bg-ink/85",
            div { class: "max-w-5xl mx-auto px-6 h-16 flex items-center gap-8",
                Link {
                    to: Route::Landing {},
                    class: "flex items-center gap-2 font-semibold tracking-tight text-paper hover:text-accent transition-colors",
                    PrnsMark { size: 24 }
                    span {
                        span { class: "text-accent", "P" }
                        "rns"
                    }
                }
                nav { class: "flex items-center gap-6 text-sm text-soft",
                    Link {
                        to: Route::ContributingPage {},
                        class: "hover:text-accent transition-colors",
                        {t!("nav-contributing")}
                    }
                    Link {
                        to: Route::CratesIndex {},
                        class: "hover:text-accent transition-colors",
                        {t!("nav-crates")}
                    }
                    a {
                        href: "/api/",
                        class: "hover:text-accent transition-colors",
                        {t!("nav-api")}
                    }
                    a {
                        href: "https://github.com/KenAKAFrosty/Prns",
                        target: "_blank",
                        rel: "noopener",
                        class: "hover:text-accent transition-colors",
                        "GitHub"
                    }
                }
                div { class: "ml-auto",
                    LanguageSwitcher {}
                }
            }
        }
    }
}
