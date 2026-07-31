use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::links::{
    api_docs_available, source_archive_available, source_zip_download_name, API_DOCS_HREF,
    BUILD_COMMIT_SHORT, BUILD_VERSION, SOURCE_ZIP_HREF,
};
use crate::routes::Route;

use super::{LanguageSwitcher, PrnsMark};

#[component]
pub fn TopNav() -> Element {
    let source_zip_download = source_zip_download_name();
    let source_archive_available = source_archive_available();

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
                nav { class: "hidden items-center gap-6 text-sm text-soft sm:flex",
                    Link {
                        to: Route::GuidesIndex {},
                        class: "hover:text-accent transition-colors",
                        "Guides"
                    }
                    Link {
                        to: Route::ContributingPage {},
                        class: "hover:text-accent transition-colors",
                        {t!("nav-contributing")}
                    }
                    if api_docs_available() {
                        a {
                            href: API_DOCS_HREF,
                            class: "hover:text-accent transition-colors",
                            {t!("nav-api")}
                        }
                    }
                    if source_archive_available {
                        a {
                            href: SOURCE_ZIP_HREF,
                            download: "{source_zip_download}",
                            title: "Download Prns {BUILD_VERSION} source snapshot {BUILD_COMMIT_SHORT}",
                            class: "inline-flex items-center gap-1.5 rounded-full border border-accent/45 px-3 py-1.5 text-accent hover:bg-accent/10 transition-colors",
                            "Source ZIP"
                            span { "↓" }
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
