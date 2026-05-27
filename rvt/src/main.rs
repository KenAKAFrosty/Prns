//! RVT desktop UI for the multi-node sim. Portable dioxus components (no
//! desktop-only APIs in the view), so the same `App` can serve on the web via
//! `dx serve` later — the only difference is the launcher.

use dioxus::prelude::*;
use rvt::Sim;

/// A genuine RNS 1.3.1 announce (181 bytes), injected as sample wire traffic.
const ANNOUNCE_HEX: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d20a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b917b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f974d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

fn announce_bytes() -> Vec<u8> {
    (0..ANNOUNCE_HEX.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&ANNOUNCE_HEX[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut sim = use_signal(|| Sim::new(&["alpha", "bravo", "charlie"], 100, 50));

    rsx! {
        div { style: "font-family: system-ui, sans-serif; background: #14171c; color: #e6e6e6; min-height: 100vh; padding: 24px;",
            h1 { style: "margin: 0 0 4px;", "Reticulum Visual Toolkit (RVT) — multi-node sim" }
            p { style: "color: #8aa; margin: 0 0 16px;",
                "virtual clock: {sim.read().now_ms} ms · in flight: {sim.read().in_flight.len()}"
            }
            div { style: "display: flex; gap: 16px; margin-bottom: 20px;",
                for i in 0..sim.read().nodes.len() {
                    div { style: "border: 2px solid #2e8b74; border-radius: 10px; padding: 14px; min-width: 140px; background: #1b2026;",
                        h3 { style: "margin: 0 0 8px;", "{sim.read().nodes[i].label}" }
                        div { style: "color: #9cf;", "ticks: {sim.read().nodes[i].tick_count()}" }
                        div { style: "color: #fc9;",
                            "ingested: {sim.read().nodes[i].ingested_packet_count()}"
                        }
                        button {
                            style: "margin-top: 10px; cursor: pointer;",
                            onclick: move |_| sim.write().inject(i, announce_bytes()),
                            "inject announce"
                        }
                    }
                }
            }
            button {
                style: "padding: 8px 18px; font-size: 15px; cursor: pointer;",
                onclick: move |_| sim.write().step(),
                "Step"
            }
        }
    }
}
