use dioxus::prelude::*;
use personal_rns::engine::{EngineState, RatchetPolicy};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::ProofStrategy;
use personal_rns::storage::GrowableHeap;

use crate::routes::Route;

#[component]
pub fn BrowserPlaygroundPage() -> Element {
    rsx! {
        header { class: "mb-8",
            Link {
                to: Route::Landing {},
                class: "text-sm text-soft hover:text-accent transition-colors",
                "Home"
            }
            p { class: "mt-6 text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                "Browser Node Playground"
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                "Run a Prns node playground from the browser"
            }
            p { class: "mt-4 max-w-3xl text-soft leading-relaxed",
                "This console loads the shared Rust engine as WebAssembly, opens a USB Auto device through WebUSB, and shows live interface and announce activity from the browser."
            }
            div { class: "mt-6 flex flex-wrap gap-3",
                a {
                    href: "/browser-node-playground-console/",
                    target: "_blank",
                    rel: "noopener noreferrer",
                    class: "inline-flex items-center gap-2 rounded-full bg-accent px-5 py-2.5 font-medium text-ink hover:bg-accent-strong transition-colors",
                    "Open full console"
                }
                a {
                    href: "/browser-node-playground-console/pkg/personal_rns_wasm_bg.wasm",
                    class: "inline-flex items-center gap-2 rounded-full border border-line/80 bg-layer/40 px-5 py-2.5 text-paper hover:border-accent/40 hover:text-accent transition-colors",
                    "WASM artifact"
                }
            }
        }

        section { class: "browser-playground-frame reveal rounded-card border border-line/60 bg-layer/40 p-3 shadow-card",
            iframe {
                title: "Prns Browser Node Playground console",
                src: "/browser-node-playground-console/",
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

        DioxusNativeProof {}
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

#[derive(Clone)]
struct NativeProofSnapshot {
    identity_hash: String,
    destination_hash: String,
    routes: usize,
    packets: u64,
    commands: u64,
}

#[component]
fn DioxusNativeProof() -> Element {
    match build_native_proof_snapshot() {
        Ok(snapshot) => rsx! {
            section { class: "mt-8 rounded-card border border-line/60 bg-surface/35 p-5",
                p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-accent",
                    "Dioxus Native"
                }
                h2 { class: "mt-2 text-xl font-semibold text-paper",
                    "The docs app can host Prns directly"
                }
                p { class: "mt-3 max-w-3xl text-sm leading-relaxed text-soft",
                    "This panel is rendered by Dioxus code inside the site. It constructs the core Rust engine with GrowableHeap storage and registers a browser playground destination without going through the TypeScript smoke harness."
                }
                dl { class: "mt-5 grid gap-3 sm:grid-cols-2 lg:grid-cols-5",
                    ProofMetric { label: "storage", value: "GrowableHeap".to_string() }
                    ProofMetric { label: "identity", value: snapshot.identity_hash }
                    ProofMetric { label: "destination", value: snapshot.destination_hash }
                    ProofMetric { label: "routes", value: snapshot.routes.to_string() }
                    ProofMetric {
                        label: "traffic",
                        value: format!(
                            "{} packets / {} commands",
                            snapshot.packets, snapshot.commands
                        )
                    }
                }
            }
        },
        Err(message) => rsx! {
            section { class: "mt-8 rounded-card border border-line/60 bg-surface/35 p-5",
                p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-accent",
                    "Dioxus Native"
                }
                p { class: "mt-2 text-sm leading-relaxed text-soft",
                    "Native Dioxus proof unavailable: {message}"
                }
            }
        },
    }
}

#[component]
fn ProofMetric(label: &'static str, value: String) -> Element {
    rsx! {
        div { class: "min-w-0 rounded-md border border-line/50 bg-layer/35 p-3",
            dt { class: "text-[0.65rem] font-bold tracking-[0.16em] uppercase text-soft",
                "{label}"
            }
            dd { class: "mt-1 break-all font-mono text-xs text-paper",
                "{value}"
            }
        }
    }
}

fn build_native_proof_snapshot() -> Result<NativeProofSnapshot, &'static str> {
    let secret = Zeroizing::new([0x42; IDENTITY_SECRET_KEY_LEN]);
    let mut engine = EngineState::<GrowableHeap>::new(secret);
    let identity_hash = *engine
        .held_identity_hashes()
        .first()
        .ok_or("engine did not retain its identity")?;
    let destination_hash = engine
        .register_single_destination(
            &identity_hash,
            "prns",
            &["browser", "dioxus"],
            b"dioxus-native-proof",
            ProofStrategy::ProveAll,
            RatchetPolicy::Ratcheted,
        )
        .map_err(|_| "destination registration failed")?;

    Ok(NativeProofSnapshot {
        identity_hash: short_hex(identity_hash.as_bytes()),
        destination_hash: short_hex(destination_hash.as_bytes()),
        routes: engine.route_count(),
        packets: engine.ingested_packet_count(),
        commands: engine.ingested_command_count(),
    })
}

fn short_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
