use dioxus::prelude::*;

#[component]
pub(crate) fn EmbeddedSite() -> Element {
    rsx! {
        main { class: "min-h-screen bg-ink px-6 py-12 text-paper",
            section { class: "mx-auto max-w-2xl rounded-card border border-line/60 bg-surface/60 p-6 sm:p-10",
                div { class: "flex items-center gap-3",
                    img { src: "/assets/prns-mark.svg", alt: "Prns", class: "h-10 w-10" }
                    div {
                        p { class: "text-xs font-semibold uppercase tracking-[0.2em] text-accent", "Personal Hopspot" }
                        h1 { class: "mt-1 text-3xl font-semibold", "Device setup" }
                    }
                }
                p { class: "mt-6 leading-relaxed text-soft",
                    "This compact page is served directly by your Hopspot. Public firmware flashing is intentionally handled by the signed online flasher or the standalone CLI."
                }
                div { class: "mt-7 grid gap-3 sm:grid-cols-2",
                    a {
                        href: "https://reticulum.rs/flash",
                        class: "rounded-lg bg-accent px-4 py-3 text-center font-semibold text-ink",
                        "Open signed web flasher"
                    }
                    a {
                        href: "https://github.com/KenAKAFrosty/Prns/releases",
                        class: "rounded-lg border border-line px-4 py-3 text-center font-semibold text-paper",
                        "Get hopspot-flash CLI"
                    }
                }
                div { class: "mt-8 rounded-lg border border-line/60 bg-layer/40 p-4",
                    h2 { class: "font-semibold", "Connected to the Hopspot network?" }
                    p { class: "mt-2 text-sm leading-relaxed text-soft",
                        "Keep this page open for local setup guidance. Flashing never runs from the embedded SoftAP bundle, and no credentials are transmitted by this page."
                    }
                }
                p { class: "mt-6 font-mono text-xs text-mid",
                    "Firmware {env!(\"PRNS_BUILD_VERSION\")} · source {env!(\"PRNS_GIT_COMMIT_SHORT\")}"
                }
            }
        }
    }
}
