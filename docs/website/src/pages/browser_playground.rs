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
        section {
            class: "w-full max-w-5xl",
            style: "width: 100%; max-width: 64rem; min-width: 0; overflow-x: hidden;",
            div { class: "mb-8",
                a { class: "text-sm text-soft hover:text-accent", href: "/", "Home" }
                p { class: "mt-8 text-xs font-bold uppercase tracking-[0.2em] text-accent",
                    "Browser Node Playground"
                }
                h1 { class: "mt-3 text-3xl md:text-4xl font-bold leading-[1.08] tracking-tight text-paper",
                    "Run a Prns node playground from the browser"
                }
                p { class: "mt-5 max-w-3xl text-base leading-relaxed text-soft",
                    "This console loads the shared Rust engine as WebAssembly, opens USB Auto hardware through WebUSB, and shows live interface and announce activity from the browser."
                }
            }

            div {
                class: "w-full rounded-card border border-line/70 bg-surface/35 p-5 md:p-7 shadow-card",
                style: "width: 100%; max-width: 100%; min-width: 0; overflow-x: hidden; box-sizing: border-box;",
                h2 { class: "text-xl font-bold text-paper", "Prns Browser Node Playground" }
                div { class: "mt-5 flex flex-wrap gap-3",
                    button {
                        id: "connect",
                        r#type: "button",
                        disabled: true,
                        class: "min-h-[2.4rem] rounded-md border border-accent/45 bg-accent/10 px-4 text-sm font-semibold text-accent transition-colors hover:bg-accent hover:text-ink disabled:border-line/70 disabled:bg-layer/40 disabled:text-mid",
                        "Connect USB"
                    }
                    button {
                        id: "announce",
                        r#type: "button",
                        disabled: true,
                        class: "min-h-[2.4rem] rounded-md border border-accent/45 bg-accent/10 px-4 text-sm font-semibold text-accent transition-colors hover:bg-accent hover:text-ink disabled:border-line/70 disabled:bg-layer/40 disabled:text-mid",
                        "Send Announce"
                    }
                    button {
                        id: "close",
                        r#type: "button",
                        disabled: true,
                        class: "min-h-[2.4rem] rounded-md border border-accent/45 bg-accent/10 px-4 text-sm font-semibold text-accent transition-colors hover:bg-accent hover:text-ink disabled:border-line/70 disabled:bg-layer/40 disabled:text-mid",
                        "Close USB"
                    }
                }

                dl {
                    class: "mt-6 grid gap-x-4 gap-y-3 text-sm sm:grid-cols-[max-content_minmax(0,1fr)]",
                    style: "min-width: 0;",
                    dt { class: "text-soft", "runtime" }
                    dd {
                        id: "runtime",
                        class: "text-paper",
                        style: "min-width: 0; overflow-wrap: anywhere;",
                        "starting"
                    }
                    dt { class: "text-soft", "usb" }
                    dd {
                        id: "usb",
                        class: "text-paper",
                        style: "min-width: 0; overflow-wrap: anywhere;",
                        "idle"
                    }
                    dt { class: "text-soft", "snapshot" }
                    dd {
                        id: "snapshot",
                        class: "text-paper",
                        style: "min-width: 0; overflow-wrap: anywhere;",
                        "none"
                    }
                    dt { class: "text-soft", "interfaces" }
                    dd { style: "min-width: 0;",
                        pre {
                            id: "interfaces",
                            class: "mini block w-full rounded-md border border-line/70 bg-ink p-3 font-mono text-xs leading-snug text-accent",
                            style: "display: block; width: 100%; max-width: 100%; min-width: 0; min-height: 4rem; box-sizing: border-box; overflow-x: hidden; overflow-y: auto; white-space: pre-wrap; overflow-wrap: anywhere; word-break: break-word;",
                            "none"
                        }
                    }
                }

                pre {
                    id: "status",
                    class: "mt-5 block w-full rounded-md border border-line/70 bg-ink p-4 font-mono text-xs leading-snug text-accent",
                    style: "display: block; width: 100%; max-width: 100%; min-width: 0; min-height: 14rem; box-sizing: border-box; overflow-x: hidden; overflow-y: auto; white-space: pre-wrap; overflow-wrap: anywhere; word-break: break-word;",
                    "running"
                }
            }
        }
        script {
            r#type: "module",
            src: "/browser-node-playground-console/dist/smoke/smoke.js"
        }
    }
}
