use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::links::{
    source_archive_available, source_zip_download_name, source_zip_sha256_download_name,
    BUILD_COMMIT, BUILD_COMMIT_SHORT, BUILD_VERSION, SOURCE_ZIP_HREF, SOURCE_ZIP_SHA256_HREF,
};
use crate::repository_docs::REPOSITORY_BLOB_BASE;
use crate::routes::Route;
use crate::site_mode::embedded_docs_mode;

use super::PrnsMark;

#[component]
pub fn Footer() -> Element {
    let source_zip_download = source_zip_download_name();
    let source_zip_sha256_download = source_zip_sha256_download_name();
    let source_archive_available = source_archive_available();
    let embedded_docs = embedded_docs_mode();

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
                        p { class: "mt-3 text-xs text-mid",
                            a {
                                href: "https://prns.dev/",
                                class: "hover:text-accent transition-colors",
                                "prns.dev"
                            }
                            span {
                                class: "text-soft/60",
                                style: "display:inline-block;padding:0 0.7rem;",
                                "·"
                            }
                            a {
                                href: "https://reticulum.rs/",
                                class: "hover:text-accent transition-colors",
                                "reticulum.rs"
                            }
                        }
                    }
                    div { class: "flex flex-col gap-4 md:items-end md:pt-1",
                        nav { class: "grid grid-cols-2 gap-x-10 gap-y-3 text-sm text-soft sm:flex sm:items-center sm:justify-end sm:gap-8",
                            Link {
                                to: Route::FlashPage {},
                                class: "hover:text-accent transition-colors",
                                {t!("footer-flash")}
                            }
                            if !embedded_docs {
                                a {
                                    href: "/browser-node-playground-console/",
                                    target: "_blank",
                                    rel: "noopener",
                                    class: "hover:text-accent transition-colors",
                                    {t!("footer-playground")}
                                }
                            }
                            a {
                                href: format!("{REPOSITORY_BLOB_BASE}/CONTRIBUTING.md"),
                                target: "_blank",
                                rel: "noopener",
                                class: "hover:text-accent transition-colors",
                                {t!("nav-contributing")}
                            }
                            if source_archive_available {
                                a {
                                    href: SOURCE_ZIP_HREF,
                                    download: "{source_zip_download}",
                                    class: "font-medium text-accent hover:text-accent-strong transition-colors",
                                    "Source ZIP"
                                }
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
                            title: "Prns {BUILD_VERSION} from commit {BUILD_COMMIT}",
                            "Prns "
                            code { class: "font-mono text-paper", "{BUILD_VERSION}" }
                            " · commit "
                            code { class: "font-mono text-paper", "{BUILD_COMMIT_SHORT}" }
                            if source_archive_available {
                                " · "
                                a {
                                    href: SOURCE_ZIP_SHA256_HREF,
                                    download: "{source_zip_sha256_download}",
                                    class: "text-accent hover:text-accent-strong transition-colors",
                                    "Source SHA-256"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
