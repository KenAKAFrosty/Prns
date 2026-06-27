use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::links::{BUILD_COMMIT, BUILD_COMMIT_SHORT, SOURCE_ZIP_HREF, SOURCE_ZIP_SHA256_HREF};
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
                        p { class: "mt-2 text-sm text-mid",
                            {t!("footer-license")}
                        }
                    }
                    div { class: "flex flex-col gap-4 md:items-end md:pt-1",
                        nav { class: "grid grid-cols-2 gap-x-10 gap-y-3 text-sm text-soft sm:flex sm:items-center sm:justify-end sm:gap-8",
                            a {
                                href: "https://github.com/KenAKAFrosty/Prns/blob/main/CONTRIBUTING.md",
                                target: "_blank",
                                rel: "noopener",
                                class: "hover:text-accent transition-colors",
                                {t!("nav-contributing")}
                            }
                            a {
                                href: "/api/",
                                class: "hover:text-accent transition-colors",
                                {t!("nav-api")}
                            }
                            a {
                                href: SOURCE_ZIP_HREF,
                                download: "prns-source.zip",
                                class: "font-medium text-accent hover:text-accent-strong transition-colors",
                                "Source ZIP"
                            }
                            a {
                                href: "https://github.com/KenAKAFrosty/Prns",
                                target: "_blank",
                                rel: "noopener",
                                class: "hover:text-accent transition-colors",
                                "GitHub"
                            }
                        }
                        p { class: "max-w-[22rem] text-xs leading-relaxed text-mid md:text-right",
                            {t!("footer-trademarks")}
                        }
                        p {
                            class: "max-w-[22rem] text-xs leading-relaxed text-mid md:text-right",
                            title: "Full commit {BUILD_COMMIT}",
                            "Source snapshot "
                            code { class: "font-mono text-paper", "{BUILD_COMMIT_SHORT}" }
                            " · "
                            a {
                                href: SOURCE_ZIP_SHA256_HREF,
                                download: "prns-source.zip.sha256",
                                class: "text-accent hover:text-accent-strong transition-colors",
                                "SHA-256"
                            }
                        }
                    }
                }
            }
        }
    }
}
