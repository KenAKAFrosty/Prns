use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::links::{
    source_archive_available, source_zip_download_name, BUILD_COMMIT_SHORT, BUILD_VERSION,
    SOURCE_ZIP_HREF,
};
use crate::repository_docs::REPOSITORY_BLOB_BASE;
use crate::routes::Route;

use super::{LanguageSwitcher, PrnsMark};

#[component]
pub fn TopNav() -> Element {
    let source_zip_download = source_zip_download_name();
    let source_archive_available = source_archive_available();

    rsx! {
        header { class: "border-b border-line/60 backdrop-blur-md sticky top-0 z-30 bg-ink/85",
            div { class: "max-w-5xl mx-auto px-6 py-3 flex flex-wrap items-center gap-x-8 sm:h-16 sm:flex-nowrap sm:py-0",
                Link {
                    to: Route::Landing {},
                    class: "flex items-center gap-2 font-semibold tracking-tight text-paper hover:text-accent transition-colors",
                    PrnsMark { size: 24 }
                    span {
                        span { class: "text-accent", "P" }
                        "rns"
                    }
                }
                nav { class: "order-last mt-3 flex w-full flex-wrap items-center gap-x-5 gap-y-2 text-sm text-soft sm:order-none sm:mt-0 sm:w-auto sm:gap-6",
                    Link {
                        to: Route::BenchmarksPage {},
                        class: "hover:text-accent transition-colors",
                        {t!("nav-benchmarks")}
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
                            title: "Download Prns {BUILD_VERSION} source snapshot {BUILD_COMMIT_SHORT}",
                            class: "hover:text-accent transition-colors",
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
                div { class: "ml-auto",
                    LanguageSwitcher {}
                }
            }
        }
    }
}
