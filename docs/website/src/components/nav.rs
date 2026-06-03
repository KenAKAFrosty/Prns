use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::routes::Route;

use super::LanguageSwitcher;

#[component]
pub fn TopNav() -> Element {
    rsx! {
        header { class: "border-b border-line/60 backdrop-blur-md sticky top-0 z-30 bg-ink/85",
            div { class: "max-w-5xl mx-auto px-6 h-16 flex items-center gap-8",
                Link {
                    to: Route::Landing {},
                    class: "font-semibold tracking-tight text-paper hover:text-accent transition-colors",
                    "Prns"
                }
                nav { class: "flex items-center gap-6 text-sm text-soft",
                    Link {
                        to: Route::EthosPage {},
                        class: "hover:text-accent transition-colors",
                        {t!("nav-ethos")}
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
                        href: "https://github.com/KenAKAFrosty/personal-reticulum-suite",
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
