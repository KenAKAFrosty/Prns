use dioxus::prelude::*;

use crate::flash_manifest::embedded_docs_mode;
use crate::links::{source_zip_download_name, SOURCE_ZIP_HREF};

#[component]
pub fn BrowserPlaygroundPage() -> Element {
    if embedded_docs_mode() {
        let source_zip_download = source_zip_download_name();

        return rsx! {
            section { class: "max-w-2xl py-16",
                p { class: "text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                    "Embedded copy"
                }
                h1 { class: "mt-3 text-3xl font-semibold tracking-tight text-paper",
                    "Browser playground lives online"
                }
                p { class: "mt-4 leading-7 text-soft",
                    "This Hopspot carries the full source archive for recovery and rebuilding. The browser playground is omitted from the embedded SoftAP bundle to keep firmware flashable."
                }
                a {
                    href: SOURCE_ZIP_HREF,
                    download: "{source_zip_download}",
                    class: "mt-6 inline-flex rounded-full border border-accent/45 px-4 py-2 text-sm font-medium text-accent hover:bg-accent/10 transition-colors",
                    "Download source ZIP"
                }
            }
        };
    }

    rsx! {
        section { class: "browser-playground-frame -mx-4 md:-mx-8 lg:-mx-10",
            iframe {
                title: "Prns Browser Node Playground console",
                src: "/browser-node-playground-console/",
                allow: "usb; bluetooth",
                style: "height: calc(100vh - 8rem); min-height: 38rem;",
                class: "block w-full border-0 bg-ink",
            }
        }
    }
}
