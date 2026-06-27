use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::routes::Route;

use super::PrnsMark;

#[component]
pub fn Footer() -> Element {
    rsx! {
        footer { class: "mt-auto border-t border-line/60 bg-surface/35",
            div { class: "max-w-5xl mx-auto px-6 py-10 sm:py-12",
                div { class: "flex flex-col gap-8 md:flex-row md:items-start md:justify-between",
                    div { class: "max-w-md",
                        Link {
                            to: Route::Landing {},
                            class: "inline-flex items-center gap-2 font-semibold tracking-tight text-paper hover:text-accent transition-colors",
                            PrnsMark { size: 24 }
                            span {
                                span { class: "text-accent", "P" }
                                "rns"
                            }
                        }
                        p { class: "mt-3 text-sm leading-6 text-soft",
                            {t!("footer-tagline")}
                        }
                    }
                    nav { class: "grid grid-cols-2 gap-x-14 gap-y-3 text-sm text-soft sm:flex sm:items-center sm:justify-end sm:gap-8 md:pt-1",
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
                }
                div { class: "mt-8 grid gap-3 border-t border-line/60 pt-5 md:grid-cols-[max-content_minmax(4rem,1fr)_minmax(0,28rem)] md:items-start",
                    p { class: "text-sm text-mid",
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
                    span { class: "hidden md:block", "aria-hidden": "true" }
                    p { class: "max-w-md text-xs leading-relaxed text-mid md:text-right",
                        {t!("footer-trademarks")}
                    }
                }
            }
        }
    }
}
