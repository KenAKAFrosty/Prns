use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use dioxus::prelude::*;

use crate::components::PlatformChip;
use crate::platforms::{BoardFlashTarget, BoardTarget, PreparationProfile, Tier};
use crate::routes::Route;
use crate::site_mode::embedded_docs_mode;

use super::bridge;
use super::contract::BridgePhase;
use super::model::{
    guided_steps, initial_status, preparation_guide, shares_serial_chip_identity, FlasherState,
    ReleaseDetails, WebSerialCapability, WifiAction,
};
use super::release;
use super::trust;

#[component]
pub(super) fn GuidedFlasher(target: &'static BoardTarget) -> Element {
    let embedded = embedded_docs_mode();
    let key_ready = trust::key_is_configured();
    let flash_target = target
        .flash_target
        .expect("the guided flasher only renders cataloged flash targets");
    let is_esp = flash_target.uses_web_serial();
    let supports_wifi = flash_target.supports_provisioning();
    let supports_tcp_client = flash_target.supports_tcp_client_provisioning();

    let mut confirmed = use_signal(|| false);
    let mut wifi_action = use_signal(|| WifiAction::Preserve);
    let mut ssid = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut tcp_enabled = use_signal(|| false);
    let mut tcp_target = use_signal(String::new);
    let phase = use_signal(|| BridgePhase::Idle);
    let mut status = use_signal(|| initial_status(flash_target).to_string());
    let progress_current = use_signal(|| 0_u64);
    let progress_total = use_signal(|| 0_u64);
    let preparation_active = use_signal(|| false);
    let preparation_generation = use_hook(|| Arc::new(AtomicU64::new(0)));
    let mut prepared = use_signal(|| false);
    let mut release_details = use_signal(|| None::<ReleaseDetails>);
    let mut web_serial = use_signal(|| WebSerialCapability::Checking);
    let state = FlasherState {
        flash_target,
        phase,
        status,
        progress_current,
        progress_total,
        preparation_active,
        preparation_generation: Arc::clone(&preparation_generation),
        prepared,
        release: release_details,
        ssid,
        password,
        tcp_enabled,
        tcp_target,
    };

    let drop_generation = Arc::clone(&preparation_generation);
    use_drop(move || {
        drop_generation.fetch_add(1, Ordering::SeqCst);
        bridge::clear_prepared();
    });

    use_effect(move || {
        if is_esp && !embedded {
            spawn(async move {
                if bridge::browser_supported().await {
                    web_serial.set(WebSerialCapability::Supported);
                } else {
                    web_serial.set(WebSerialCapability::Unavailable);
                    status.set(
                        "Web Serial is unavailable in this browser or context. Open this page in current Chrome or Edge over HTTPS, or use the standalone CLI."
                            .to_string(),
                    );
                }
            });
        }
    });

    let busy = preparation_active() || bridge::is_busy(phase());
    let device_operation_active = busy && !preparation_active();
    let browser_ready = !is_esp || web_serial().permits_esp_flash();
    let browser_checking = is_esp && web_serial() == WebSerialCapability::Checking;
    let browser_blocked = is_esp && web_serial() == WebSerialCapability::Unavailable;
    let can_prepare = confirmed()
        && !busy
        && !embedded
        && key_ready
        && browser_ready
        && (!tcp_enabled() || !tcp_target().trim().is_empty());
    let can_flash = prepared() && !busy && browser_ready;
    let action_label = match flash_target {
        BoardFlashTarget::EspSerial { .. } => "Connect and flash",
        BoardFlashTarget::Uf2MassStorage { .. } => "Download verified UF2",
    };

    rsx! {
        section { class: "flash-flasher-panel",
            BrowserTestFixtureMarker {}
            div { class: "flash-flasher-panel__main",
                p { class: "text-xs font-semibold tracking-[0.2em] uppercase text-accent",
                    "Selected target"
                }
                div { class: "flash-ready-target mt-4",
                    div { class: "flash-ready-target__summary",
                        div { class: "flash-ready-target__copy",
                            PlatformChip {
                                name: target.name.to_string(),
                                icon: target.icon.map(str::to_string),
                                badge: None,
                                muted: false,
                                decorative: false,
                            }
                            p { class: "flash-board-silicon font-mono text-xs", "{target.silicon}" }
                            if let Some(image) = target.image() {
                                span { class: "flash-board-slot flash-board-slot--inset flash-board-slot--hero",
                                    img {
                                        class: "flash-board-img",
                                        src: image.data_uri,
                                        alt: "{target.name}",
                                        loading: "eager",
                                    }
                                }
                            }
                        }
                    }
                }

                label { class: "mt-5 flex cursor-pointer items-start gap-3 rounded-lg border border-line/60 bg-surface/40 p-4 text-sm text-soft",
                    input {
                        r#type: "checkbox",
                        checked: confirmed(),
                        disabled: device_operation_active,
                        onchange: {
                            let event_state = state.clone();
                            move |event| {
                                let checked = event.checked();
                                confirmed.set(checked);
                                invalidate_preparation(
                                    event_state.clone(),
                                    if checked {
                                        "Board confirmed. Prepare and verify the signed release."
                                    } else {
                                        "Board confirmation changed. Confirm the exact board before preparing."
                                    },
                                    true,
                                );
                            }
                        },
                    }
                    span {
                        "I checked the board label and image: this is "
                        strong { class: "text-paper", "{target.name}" }
                        if shares_serial_chip_identity(target) {
                            span { class: "mt-1 block text-xs text-mid",
                                "The chip check confirms only the chip family; it cannot distinguish cataloged boards that share that family. The printed board label and photo are the final identity check."
                            }
                        }
                    }
                }

                if let Some(profile) = target.preparation_profile {
                    PreparationInstructions { profile, flash_target }
                }

                if supports_wifi {
                    fieldset { class: "flash-wifi-config mt-5",
                        legend { class: "font-semibold text-paper", "Wi-Fi configuration" }
                        p { class: "flash-wifi-note mt-2",
                            "Credentials remain in this browser and are never sent to a server. Preserve is the default."
                        }
                        div { class: "grid gap-2 text-sm text-soft",
                            for (value, label) in [
                                (WifiAction::Preserve, "Preserve existing configuration"),
                                (WifiAction::Configure, "Configure a network locally"),
                                (WifiAction::Clear, "Clear configuration explicitly"),
                            ] {
                                label { class: "flex items-center gap-2",
                                    input {
                                        r#type: "radio",
                                        name: "wifi-action",
                                        value: value.wire(),
                                        checked: wifi_action() == value,
                                        disabled: device_operation_active,
                                        onchange: {
                                            let event_state = state.clone();
                                            move |_| {
                                                wifi_action.set(value);
                                                tcp_enabled.set(false);
                                                tcp_target.set(String::new());
                                                invalidate_preparation(
                                                    event_state.clone(),
                                                    "Configuration choice changed. Prepare and verify the release again.",
                                                    true,
                                                );
                                            }
                                        },
                                    }
                                    "{label}"
                                }
                            }
                        }
                        if wifi_action() == WifiAction::Configure {
                            div { class: "flash-wifi-grid mt-4",
                                label { class: "flash-wifi-field",
                                    span { "SSID" }
                                    input {
                                        value: ssid(),
                                        maxlength: "32",
                                        autocomplete: "off",
                                        disabled: device_operation_active,
                                        oninput: {
                                            let event_state = state.clone();
                                            move |event| {
                                                ssid.set(event.value());
                                                invalidate_preparation(
                                                    event_state.clone(),
                                                    "Configuration changed. Prepare and verify the release again.",
                                                    false,
                                                );
                                            }
                                        },
                                    }
                                }
                                label { class: "flash-wifi-field",
                                    span { "Password" }
                                    input {
                                        r#type: "password",
                                        value: password(),
                                        maxlength: "64",
                                        autocomplete: "new-password",
                                        disabled: device_operation_active,
                                        oninput: {
                                            let event_state = state.clone();
                                            move |event| {
                                                password.set(event.value());
                                                invalidate_preparation(
                                                    event_state.clone(),
                                                    "Configuration changed. Prepare and verify the release again.",
                                                    false,
                                                );
                                            }
                                        },
                                    }
                                }
                            }
                            if supports_tcp_client {
                                label { class: "mt-4 flex items-start gap-3 rounded-lg border border-line/60 bg-surface/40 p-4 text-sm text-soft",
                                    input {
                                        r#type: "checkbox",
                                        checked: tcp_enabled(),
                                        disabled: device_operation_active,
                                        onchange: {
                                            let event_state = state.clone();
                                            move |event| {
                                                let enabled = event.checked();
                                                tcp_enabled.set(enabled);
                                                if !enabled {
                                                    tcp_target.set(String::new());
                                                }
                                                invalidate_preparation(
                                                    event_state.clone(),
                                                    "TCP client configuration changed. Prepare and verify the release again.",
                                                    false,
                                                );
                                            }
                                        },
                                    }
                                    span {
                                        "Connect one outbound Reticulum TCP client"
                                        span { class: "mt-1 block text-xs text-mid",
                                            "Use an IPv4 address, DNS hostname, or URL. The S3 resolves hostnames with the Wi-Fi network's DHCP-provided DNS server and refreshes them when reconnecting."
                                        }
                                    }
                                }
                                if tcp_enabled() {
                                    label { class: "flash-wifi-field mt-4",
                                        span { "TCP target" }
                                        input {
                                            value: tcp_target(),
                                            maxlength: "512",
                                            autocomplete: "off",
                                            placeholder: "node.example:4242",
                                            disabled: device_operation_active,
                                            oninput: {
                                                let event_state = state.clone();
                                                move |event| {
                                                    tcp_target.set(event.value());
                                                    invalidate_preparation(
                                                        event_state.clone(),
                                                        "TCP client configuration changed. Prepare and verify the release again.",
                                                        false,
                                                    );
                                                }
                                            },
                                        }
                                        span { class: "mt-1 block text-xs text-mid",
                                            "One client only. Port 4242 is used when omitted; IPv6 literals are not supported in this embedded profile."
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "flash-plan-panel mt-5",
                    div { class: "flash-plan-panel__head",
                        h3 { class: "font-semibold text-paper", "Review and verify" }
                        span { class: bridge::status_class(phase()), "{bridge::phase_label(phase())}" }
                    }
                    ol { class: "flash-step-list mt-4",
                        for (index, step) in guided_steps(flash_target).iter().enumerate() {
                            li {
                                span { class: "flash-step-list__index", "{index + 1}" }
                                span { "{step}" }
                            }
                        }
                    }
                    p { class: "mt-4 text-sm font-semibold text-accent",
                        "No full-chip erase. Every published byte is signature- and hash-verified before device access."
                    }
                }

                if embedded {
                    div { class: "flash-embedded-note mt-5",
                        "The SoftAP copy intentionally excludes the hosted serial engine. Open "
                        a { href: "https://reticulum.rs/flash/{target.slug}", class: "text-accent", "the online flasher" }
                        " or use "
                        code { class: "flash-local-command", "hopspot-flash flash {target.slug}" }
                        "."
                    }
                } else if !key_ready {
                    div { class: "flash-web-install-message mt-5",
                        "Release signing custody is not configured yet. The flasher fails closed until the offline Minisign public key is pinned."
                    }
                } else if browser_checking {
                    div { class: "flash-web-install-message mt-5",
                        "Checking this browser for secure Web Serial support before release preparation is enabled."
                    }
                } else if browser_blocked {
                    div { class: "flash-web-install-message mt-5",
                        "Direct ESP flashing requires a secure current Chrome or Edge browser with Web Serial. The standalone CLI provides the same verified release path."
                    }
                }

                div {
                    id: "flash-status",
                    class: "mt-5 rounded-lg border border-line/60 bg-surface/50 p-4",
                    role: "status",
                    "aria-live": "polite",
                    "aria-atomic": "true",
                    tabindex: "-1",
                    p { class: "text-sm font-semibold text-paper", "{status}" }
                    if progress_total() > 0 {
                        progress {
                            class: "mt-3 h-2 w-full accent-[var(--color-accent)]",
                            max: "{progress_total}",
                            value: "{progress_current}",
                        }
                        p { class: "mt-2 font-mono text-xs text-mid",
                            "{progress_current} / {progress_total} bytes"
                        }
                    }
                }

                if device_operation_active {
                    p {
                        class: "mt-3 rounded-lg border border-amber-300/40 bg-amber-300/10 p-3 text-sm font-semibold text-amber-100",
                        role: "alert",
                        "Internal navigation is blocked while the device operation owns the serial port. Keep this page open until completion or safe cancellation."
                    }
                }

                if let Some(details) = release_details() {
                    details { class: "flash-artifact-panel mt-5",
                        summary { class: "cursor-pointer font-semibold text-soft", "Verified artifact details" }
                        div { class: "flash-artifact-details mt-4",
                            FlashFact { label: "Version", value: details.version.clone(), mono: false }
                            FlashFact { label: "Channel", value: details.channel.clone(), mono: false }
                            FlashFact { label: "Total", value: format!("{} bytes", details.total), mono: true }
                            for part in details.parts {
                                FlashFact {
                                    key: "{part.kind}",
                                    label: part.kind,
                                    value: format!("{} bytes · {}", part.size, part.sha256),
                                    mono: true,
                                }
                            }
                        }
                    }
                }
            }

            div { class: "flash-flasher-panel__action grid gap-3",
                button {
                    r#type: "button",
                    class: "flash-primary-action",
                    disabled: !can_prepare,
                    onclick: {
                        let event_state = state.clone();
                        move |_| {
                            let target_slug = target.slug.to_string();
                            let selected_action = wifi_action();
                            let selected_ssid = ssid();
                            let selected_password = password();
                            let selected_tcp_target = if tcp_enabled() {
                                Some(tcp_target())
                            } else {
                                None
                            };
                            let mut preparation_state = event_state.clone();
                            let generation = preparation_state.begin_preparation();
                            prepared.set(false);
                            release_details.set(None);
                            spawn(async move {
                                release::prepare_release(
                                    target_slug,
                                    selected_action,
                                    selected_ssid,
                                    selected_password,
                                    selected_tcp_target,
                                    preparation_state,
                                    generation,
                                )
                                .await;
                            });
                        }
                    },
                    if prepared() { "Re-verify release" } else { "Prepare and verify release" }
                }
                button {
                    r#type: "button",
                    class: "flash-primary-action",
                    disabled: !can_flash,
                    onclick: {
                        let event_state = state.clone();
                        move |_| {
                            let flash_state = event_state.clone();
                            spawn(async move {
                                bridge::run_flash(flash_state).await;
                            });
                        }
                    },
                    "{action_label}"
                }
                if busy {
                    button {
                        r#type: "button",
                        class: "rounded-lg border border-line px-4 py-3 text-sm font-semibold text-soft",
                        onclick: {
                            let event_state = state.clone();
                            move |_| {
                                let was_preparing = preparation_active();
                                invalidate_preparation(
                                    event_state.clone(),
                                    if was_preparing {
                                        "Release preparation cancelled. Review the selection before retrying."
                                    } else {
                                        "Cancellation requested; an active write will finish its safe operation before stopping."
                                    },
                                    true,
                                );
                                if !was_preparing {
                                    status.set("Cancellation requested; an active write will finish its safe operation before stopping.".to_string());
                                }
                                bridge::focus_status();
                            }
                        },
                        "Cancel safely"
                    }
                }
            }
        }
    }
}

fn invalidate_preparation(mut state: FlasherState, message: &str, clear_credentials: bool) {
    let was_preparing = (state.preparation_active)();
    state.invalidate_preparation();
    state.prepared.set(false);
    state.release.set(None);
    state.progress_current.set(0);
    state.progress_total.set(0);
    if was_preparing || !bridge::is_busy((state.phase)()) {
        state.phase.set(BridgePhase::Idle);
    }
    state.status.set(message.to_string());
    if clear_credentials {
        state.ssid.set(String::new());
        state.password.set(String::new());
        state.tcp_enabled.set(false);
        state.tcp_target.set(String::new());
    }
    bridge::clear_prepared();
}

#[cfg(feature = "browser-test-fixture")]
#[component]
fn BrowserTestFixtureMarker() -> Element {
    rsx! {
        span {
            hidden: true,
            "data-prns-browser-test-fixture": trust::BROWSER_TEST_MARKER,
        }
    }
}

#[cfg(not(feature = "browser-test-fixture"))]
#[component]
fn BrowserTestFixtureMarker() -> Element {
    rsx! {}
}

#[component]
fn PreparationInstructions(profile: PreparationProfile, flash_target: BoardFlashTarget) -> Element {
    let guide = preparation_guide(profile, flash_target);

    rsx! {
        section {
            class: "flash-preparation mt-5",
            "aria-labelledby": "flash-preparation-title",
            h3 { id: "flash-preparation-title", class: "font-semibold text-paper", "Prepare the board" }
            p { class: "mt-2 text-sm leading-relaxed text-soft", "{guide.lead}" }
            ol { class: "flash-preparation__steps mt-3",
                for (index, step) in guide.steps.iter().enumerate() {
                    li {
                        span { class: "flash-preparation__index", "{index + 1}" }
                        span { "{step}" }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn BoardTargetCard(board: &'static BoardTarget, selected: bool) -> Element {
    rsx! {
        div { class: board_card_class(board, selected),
            div { class: "flex flex-wrap items-start gap-3",
                PlatformChip {
                    name: board.name.to_string(),
                    icon: board.icon.map(str::to_string),
                    badge: board.tier.chip_badge().map(str::to_string),
                    muted: board.tier.muted(),
                    decorative: false,
                }
            }
            div { class: "mt-4 flex items-center gap-4",
                if let Some(image) = board.image() {
                    span { class: "flash-board-slot flash-board-slot--inset",
                        img { class: "flash-board-img", src: image.data_uri, alt: "", loading: "lazy" }
                    }
                }
                p { class: "flash-board-silicon font-mono text-xs", "{board.silicon}" }
            }
            if board.is_flashable() {
                div { class: "mt-5 flex justify-end",
                    if selected {
                        span { class: "py-2.5 text-xs font-bold uppercase tracking-wider text-accent", "Selected" }
                    } else {
                        Link {
                            to: Route::FlashBoardPage { board: board.slug.to_string() },
                            class: "flash-card-action",
                            "Flash "
                            span { class: "flash-card-action__arrow", "→" }
                        }
                    }
                }
            } else {
                p { class: "flash-interfaces-pending mt-4",
                    match board.tier {
                        Tier::BringUp => "Bring-up in progress",
                        Tier::Roadmap => "Planned",
                        Tier::Shipping | Tier::Flashable => "Coming later",
                    }
                }
            }
        }
    }
}

#[component]
fn FlashFact(label: &'static str, value: String, mono: bool) -> Element {
    rsx! {
        div { class: "flash-artifact-fact",
            span { class: "flash-artifact-fact__label", "{label}" }
            span {
                class: if mono { "flash-artifact-fact__value flash-artifact-fact__value--mono" } else { "flash-artifact-fact__value" },
                "{value}"
            }
        }
    }
}

#[component]
pub(super) fn UnavailablePanel() -> Element {
    rsx! {
        section { class: "rounded-card border border-line/60 bg-layer/40 p-5",
            h2 { class: "text-xl font-semibold text-paper", "Not flashable yet" }
            p { class: "mt-3 text-soft", "This target is still in bring-up or roadmap tracking." }
        }
    }
}

fn board_card_class(board: &BoardTarget, selected: bool) -> String {
    let selected_class = if selected {
        " flash-board-card--selected"
    } else {
        ""
    };
    format!(
        "flash-board-card {}{} rounded-card border border-line/60 bg-layer/40 p-5 shadow-card",
        board.tier.flash_card_class(),
        selected_class
    )
}
