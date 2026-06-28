use dioxus::prelude::*;

use crate::routes::Route;

#[component]
pub fn BrowserHopspotPage() -> Element {
    rsx! {
        header { class: "mb-8",
            Link {
                to: Route::Landing {},
                class: "text-sm text-soft hover:text-accent transition-colors",
                "Home"
            }
            p { class: "mt-6 text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                "Browser Hopspot"
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                "Run Prns from the browser"
            }
            p { class: "mt-4 max-w-3xl text-soft leading-relaxed",
                "This console loads the shared Rust engine as WebAssembly, opens a Hopspot USB Auto device through WebUSB, and shows live interface and announce activity from the browser."
            }
            div { class: "mt-6 flex flex-wrap gap-3",
                a {
                    href: "/browser-hopspot-console/",
                    target: "_blank",
                    rel: "noopener noreferrer",
                    class: "inline-flex items-center gap-2 rounded-full bg-accent px-5 py-2.5 font-medium text-ink hover:bg-accent-strong transition-colors",
                    "Open full console"
                }
                a {
                    href: "/browser-hopspot-console/pkg/personal_rns_wasm_bg.wasm",
                    class: "inline-flex items-center gap-2 rounded-full border border-line/80 bg-layer/40 px-5 py-2.5 text-paper hover:border-accent/40 hover:text-accent transition-colors",
                    "WASM artifact"
                }
            }
        }

        section { class: "browser-hopspot-frame reveal rounded-card border border-line/60 bg-layer/40 p-3 shadow-card",
            iframe {
                title: "Prns Browser Hopspot console",
                src: "/browser-hopspot-console/",
                allow: "usb; bluetooth",
                style: "height: 42rem;",
                class: "w-full rounded-md border border-line/70 bg-ink",
            }
        }

        section { class: "mt-8 grid gap-4 md:grid-cols-3",
            Capability {
                label: "Runtime",
                body: "The engine state, routes, packet dedupe, and destinations are the same GrowableHeap Rust runtime compiled to WASM."
            }
            Capability {
                label: "USB Auto",
                body: "The browser asks for the shared Prns WebUSB VID/PID exported from the Rust USB Auto core."
            }
            Capability {
                label: "Local only",
                body: "The demo is client-side. USB permission stays with the browser session and the page does not require a backend."
            }
        }
    }
}

#[component]
fn Capability(label: &'static str, body: &'static str) -> Element {
    rsx! {
        div { class: "rounded-card border border-line/60 bg-surface/35 p-5",
            p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-accent",
                "{label}"
            }
            p { class: "mt-2 text-sm leading-relaxed text-soft",
                "{body}"
            }
        }
    }
}
