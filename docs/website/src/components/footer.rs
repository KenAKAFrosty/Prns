use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Footer() -> Element {
    rsx! {
        footer { class: "border-t border-line/60 mt-auto",
            div { class: "max-w-5xl mx-auto px-6 py-10 flex flex-col md:flex-row gap-4 md:items-center justify-between text-sm text-soft",
                p { {t!("footer-tagline")} }
                p { class: "text-mid",
                    span { class: "text-soft", {t!("footer-license")} }
                    " · "
                    a {
                        href: "https://github.com/KenAKAFrosty/Prns",
                        target: "_blank",
                        rel: "noopener",
                        class: "hover:text-accent",
                        "source"
                    }
                }
            }
        }
    }
}
