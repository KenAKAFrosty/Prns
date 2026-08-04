use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use dioxus::prelude::*;

use crate::components::PlatformChip;
use crate::local_development;
use crate::platforms::{BoardFlashTarget, BoardTarget, PreparationProfile, Tier};
use crate::routes::Route;
use crate::site_mode::embedded_docs_mode;

use super::bridge;
use super::contract::BridgePhase;
use super::model::{
    guided_steps, initial_status, preparation_guide, shares_serial_chip_identity,
    DestructiveConfirmation, FlasherState, InstallMode, ReleaseDetails, WebSerialCapability,
    WifiAction,
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
    let mut install_mode = use_signal(|| InstallMode::PreserveData);
    let mut destructive_confirmation = use_signal(|| DestructiveConfirmation::Unconfirmed);
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
        install_mode,
        destructive_confirmation,
    };

    let drop_generation = Arc::clone(&preparation_generation);
    use_drop(move || {
        drop_generation.fetch_add(1, Ordering::SeqCst);
        bridge::clear_prepared();
    });

    use_effect(move || {
        if is_esp && !embedded {
            spawn(async move {
                let capability = bridge::web_serial_capability().await;
                web_serial.set(capability);
                if let Some(explanation) = capability.blocked_explanation() {
                    status.set(explanation.to_string());
                }
            });
        }
    });

    let busy = preparation_active() || bridge::is_busy(phase());
    let device_operation_active = busy && !preparation_active();
    let browser_ready = !is_esp || web_serial().permits_esp_flash();
    let browser_checking = is_esp && web_serial() == WebSerialCapability::Checking;
    let browser_android = is_esp && web_serial() == WebSerialCapability::AndroidBluetoothOnly;
    let browser_blocked = is_esp && web_serial() == WebSerialCapability::Unavailable;
    let destructive_action_permitted = destructive_confirmation().permits(install_mode());
    let can_prepare = confirmed()
        && destructive_action_permitted
        && !busy
        && !embedded
        && key_ready
        && browser_ready
        && (!tcp_enabled() || !tcp_target().trim().is_empty());
    let can_flash = prepared() && !busy && browser_ready;
    let action_label = match (flash_target, install_mode()) {
        (BoardFlashTarget::EspSerial { .. }, InstallMode::PreserveData) => "Connect and flash",
        (BoardFlashTarget::EspSerial { .. }, InstallMode::EraseAll) => {
            "Connect, erase, and install"
        }
        (BoardFlashTarget::Uf2MassStorage { .. }, _) => "Download verified UF2",
    };
    let cancellation_available = preparation_active()
        || install_mode() == InstallMode::PreserveData
        || matches!(
            phase(),
            BridgePhase::RequestingPort | BridgePhase::Connecting | BridgePhase::VerifyingTarget
        );
    let wifi_choices = match install_mode() {
        InstallMode::PreserveData => [
            Some((WifiAction::Preserve, "Preserve existing configuration")),
            Some((WifiAction::Configure, "Configure a network locally")),
            Some((WifiAction::Clear, "Clear configuration explicitly")),
        ],
        InstallMode::EraseAll => [
            Some((WifiAction::Clear, "Leave Wi-Fi and TCP blank")),
            Some((WifiAction::Configure, "Configure new values locally")),
            None,
        ],
    };

    rsx! {
        section { class: "flash-flasher-panel",
            BuildTrustMarker {}
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

                if is_esp {
                    fieldset { class: "flash-wifi-config mt-5",
                        legend { class: "font-semibold text-paper", "Installation mode" }
                        div { class: "mt-3 grid gap-3 text-sm text-soft",
                            for (value, label, detail) in [
                                (
                                    InstallMode::PreserveData,
                                    "Update firmware and preserve device data",
                                    "Writes only the verified sparse firmware parts. Node identity, BLE identity, routes, ratchets, NVS, PHY calibration, and existing Wi-Fi/TCP state remain untouched.",
                                ),
                                (
                                    InstallMode::EraseAll,
                                    "Fresh install — erase all device data",
                                    "Erases the entire flash before reinstalling the verified firmware.",
                                ),
                            ] {
                                label { class: "flex cursor-pointer items-start gap-3 rounded-lg border border-line/60 bg-surface/40 p-4",
                                    input {
                                        r#type: "radio",
                                        name: "install-mode",
                                        value: value.wire(),
                                        checked: install_mode() == value,
                                        disabled: device_operation_active,
                                        onchange: {
                                            let event_state = state.clone();
                                            move |_| {
                                                install_mode.set(value);
                                                destructive_confirmation
                                                    .set(DestructiveConfirmation::Unconfirmed);
                                                wifi_action
                                                    .set(WifiAction::for_install_mode(value));
                                                tcp_enabled.set(false);
                                                invalidate_preparation(
                                                    event_state.clone(),
                                                    "Installation mode changed. Review the plan and prepare the signed release again.",
                                                );
                                            }
                                        },
                                    }
                                    span {
                                        strong { class: "block text-paper", "{label}" }
                                        span { class: "mt-1 block text-xs text-mid", "{detail}" }
                                    }
                                }
                            }
                        }
                    }
                }

                if install_mode() == InstallMode::EraseAll {
                    div {
                        class: "mt-4 rounded-lg border border-red-300/50 bg-red-300/10 p-4 text-sm text-soft",
                        role: "alert",
                        p { class: "font-semibold text-red-100",
                            "Fresh install permanently erases all mutable flash state."
                        }
                        p { class: "mt-2",
                            "Node identity, BLE identity, routes, ratchets, Wi-Fi/TCP configuration, NVS, and PHY calibration will be erased. eFuses and the factory MAC are unaffected."
                        }
                        label { class: "mt-3 flex cursor-pointer items-start gap-3 font-semibold text-paper",
                            input {
                                r#type: "checkbox",
                                checked: destructive_confirmation().is_confirmed(),
                                disabled: device_operation_active,
                                onchange: {
                                    let event_state = state.clone();
                                    move |event| {
                                        destructive_confirmation.set(if event.checked() {
                                            DestructiveConfirmation::Confirmed
                                        } else {
                                            DestructiveConfirmation::Unconfirmed
                                        });
                                        invalidate_preparation(
                                            event_state.clone(),
                                            "Destructive confirmation changed. Confirm the full-chip erase before preparing again.",
                                        );
                                    }
                                },
                            }
                            span {
                                "I understand that Fresh install erases all device data and requires a complete reinstall."
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
                            if install_mode() == InstallMode::EraseAll {
                                "Credentials remain in this browser and are never sent to a server. Fresh install leaves Wi-Fi and TCP blank unless you explicitly configure new values."
                            } else {
                                "Credentials remain in this browser and are never sent to a server. Preserve is the default."
                            }
                        }
                        div { class: "grid gap-2 text-sm text-soft",
                            for choice in wifi_choices {
                                if let Some((value, label)) = choice {
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
                                                    invalidate_preparation(
                                                        event_state.clone(),
                                                        "Configuration choice changed. Prepare and verify the release again.",
                                                    );
                                                }
                                            },
                                        }
                                        "{label}"
                                    }
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
                                        name: "username",
                                        autocomplete: "username",
                                        autocapitalize: "none",
                                        spellcheck: "false",
                                        disabled: device_operation_active,
                                        oninput: {
                                            let event_state = state.clone();
                                            move |event| {
                                                ssid.set(event.value());
                                                invalidate_preparation(
                                                    event_state.clone(),
                                                    "Configuration changed. Prepare and verify the release again.",
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
                                        name: "password",
                                        autocomplete: "current-password",
                                        disabled: device_operation_active,
                                        oninput: {
                                            let event_state = state.clone();
                                            move |event| {
                                                password.set(event.value());
                                                invalidate_preparation(
                                                    event_state.clone(),
                                                    "Configuration changed. Prepare and verify the release again.",
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
                                                invalidate_preparation(
                                                    event_state.clone(),
                                                    "TCP client configuration changed. Prepare and verify the release again.",
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
                                            name: "tcp-target",
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
                        for (index, step) in guided_steps(flash_target, install_mode()).iter().enumerate() {
                            li {
                                span { class: "flash-step-list__index", "{index + 1}" }
                                span { "{step}" }
                            }
                        }
                    }
                    p { class: "mt-4 text-sm font-semibold text-accent",
                        if install_mode() == InstallMode::EraseAll {
                            "Full-chip erase starts only after signed-artifact, chip-family, and flash-capacity verification."
                        } else {
                            "No full-chip erase. Every published byte is signature- and hash-verified before device access."
                        }
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
                } else if browser_android {
                    div { class: "flash-web-install-message mt-5",
                        "This Android browser provides Web Serial for Bluetooth serial devices only; a board connected over USB cannot be selected yet. Wired serial support waits on a new Android system API that only a limited set of devices provides. Desktop Chrome or Edge, or the standalone CLI, provides the same verified release path."
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
                            let selected_install_mode = install_mode();
                            let selected_destructive_confirmation = destructive_confirmation();
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
                                    release::ReleaseSelection {
                                        board_slug: target_slug,
                                        install_mode: selected_install_mode,
                                        destructive_confirmation:
                                            selected_destructive_confirmation,
                                        wifi_action: selected_action,
                                        ssid: selected_ssid,
                                        password: selected_password,
                                        tcp_target: selected_tcp_target,
                                    },
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
                        disabled: !cancellation_available,
                        onclick: {
                            let event_state = state.clone();
                            move |_| {
                                let was_preparing = preparation_active();
                                if install_mode() == InstallMode::EraseAll {
                                    destructive_confirmation
                                        .set(DestructiveConfirmation::Unconfirmed);
                                }
                                invalidate_preparation(
                                    event_state.clone(),
                                    if was_preparing {
                                        "Release preparation cancelled. Review the selection before retrying."
                                    } else {
                                        "Cancellation requested; an active write will finish its safe operation before stopping."
                                    },
                                );
                                if !was_preparing {
                                    status.set("Cancellation requested; an active write will finish its safe operation before stopping.".to_string());
                                }
                                bridge::focus_status();
                            }
                        },
                        if cancellation_available {
                            "Cancel safely"
                        } else {
                            "Cancellation unavailable after erase begins"
                        }
                    }
                }
            }
        }
    }
}

fn invalidate_preparation(mut state: FlasherState, message: &str) {
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
    bridge::clear_prepared();
}

#[cfg(all(feature = "browser-test-fixture", not(feature = "local-dev-flasher")))]
#[component]
fn BuildTrustMarker() -> Element {
    rsx! {
        span {
            hidden: true,
            "data-prns-browser-test-fixture": trust::BROWSER_TEST_MARKER,
        }
    }
}

#[cfg(all(feature = "local-dev-flasher", not(feature = "browser-test-fixture")))]
#[component]
fn BuildTrustMarker() -> Element {
    rsx! {
        span {
            hidden: true,
            "data-prns-local-dev-flasher": local_development::TRUST_MARKER,
        }
    }
}

#[cfg(not(any(feature = "browser-test-fixture", feature = "local-dev-flasher")))]
#[component]
fn BuildTrustMarker() -> Element {
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
    let included = local_development::board_is_included(board.slug);
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
            if board.is_flashable() && included {
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
            } else if board.is_flashable() {
                p { class: "flash-interfaces-pending mt-4",
                    "Not included in this local build"
                }
            } else {
                p { class: "flash-interfaces-pending mt-4",
                    match board.tier {
                        Tier::BringUp => "Bring-up in progress",
                        Tier::Roadmap => "Planned",
                        Tier::Shipping | Tier::SdkPreview | Tier::Flashable => "Coming later",
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

#[component]
pub(super) fn LocalBuildUnavailablePanel() -> Element {
    rsx! {
        section { class: "rounded-card border border-amber-300/40 bg-amber-300/10 p-5",
            h2 { class: "text-xl font-semibold text-amber-100", "Not included in this local build" }
            p { class: "mt-3 text-soft", "Restart the developer flasher task with this board selected to build and sign it from the current working tree." }
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
