//! `/configure` — the headless-config webUI. Replaces the e-ink screen the
//! T-Echo exposes on T1000-E class boards (no display, no button): the device
//! enumerates a WebUSB config lane and this page drives it over the staged JS
//! module (`/assets/configure/configure.js`). See `T1000E_HEADLESS_CONFIG.md`.
//!
//! The page is intentionally leaner than the flasher: configure is a live
//! request/response device session, not a release/flash flow, so there is no
//! trust/contract/protocol machinery here — just the bridge in `bridge.rs`,
//! the snapshot render, and the LoRa profile editor + ephemeral action buttons.

mod bridge;

use dioxus::prelude::*;

use crate::routes::Route;

use bridge::{
    ConfigResult, ConfigureEvent, ConfigureRequest, ConfigureSection, ConfigureSnapshot,
    InterfaceKind,
};

/// UI phase. Driven entirely by [`ConfigureEvent`]s from the JS lane.
#[derive(Debug, Clone, PartialEq)]
enum ConfigurePhase {
    /// WebUSB is unavailable (insecure context / unsupported browser).
    Unsupported(String),
    /// Probing WebUSB availability on mount.
    Probing,
    /// WebUSB is available; no device connected yet.
    Disconnected,
    /// A Connect or action is in flight.
    Busy,
    /// A device session is live.
    Connected,
    /// The session ended with a failure the user can read.
    Failed(String),
}

#[component]
pub fn ConfigurePage() -> Element {
    let mut phase = use_signal(|| ConfigurePhase::Probing);
    let snapshot = use_signal(|| Option::<ConfigureSnapshot>::None);
    let mut status = use_signal(String::new);
    let busy = use_signal(|| false);

    // LoRa profile editor fields. Populated from the snapshot's `radioProfile`
    // section; sent back via `ApplySetLoRaProfile`.
    let mut ed_freq = use_signal(|| 915_000_000u32);
    let mut ed_sf = use_signal(|| 7u8);
    let mut ed_bw = use_signal(|| 2u8);
    let mut ed_cr = use_signal(|| 5u8);
    let mut ed_txpower = use_signal(|| 17i32);
    let mut ed_preamble = use_signal(|| 8u16);
    let mut ed_region = use_signal(|| 1u8);

    // Probe WebUSB on mount.
    use_future(move || async move {
        let event = bridge::probe_ready().await;
        match event {
            Some(ConfigureEvent::Ready { supported, reason }) => {
                if supported {
                    phase.set(ConfigurePhase::Disconnected);
                } else {
                    phase.set(ConfigurePhase::Unsupported(
                        reason.unwrap_or_else(|| "WebUSB is not available.".to_string()),
                    ));
                }
            }
            _ => phase.set(ConfigurePhase::Unsupported(
                "Could not probe WebUSB support.".to_string(),
            )),
        }
    });

    let can_act = matches!(*phase.read(), ConfigurePhase::Connected) && !*busy.read();
    let phase_label = match &*phase.read() {
        ConfigurePhase::Unsupported(_) => "WebUSB unavailable",
        ConfigurePhase::Probing => "Checking WebUSB…",
        ConfigurePhase::Disconnected => "Not connected",
        ConfigurePhase::Busy => "Working…",
        ConfigurePhase::Connected => "Connected",
        ConfigurePhase::Failed(_) => "Failed",
    };

    rsx! {
        header { class: "mb-10",
            Link {
                to: Route::Landing {},
                class: "text-sm text-soft hover:text-accent transition-colors",
                "← Back"
            }
            p { class: "mt-6 text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                "Headless config"
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                "Configure a Personal Hopspot"
            }
            p { class: "mt-4 max-w-3xl leading-relaxed text-soft",
                "For boards without a screen (like the Seeed T1000-E), this page replaces the e-ink display. \
                 Connect over WebUSB to load a live snapshot of the device — radio profile, per-interface \
                 status, BLE recovery, and the LoRa spectrum — and apply changes straight to the radio."
            }
        }

        section { class: "rounded-card border border-line/60 bg-layer/40 p-5",
            div { class: "flex items-center justify-between gap-4",
                p { class: "configure-status text-sm text-soft", "{phase_label}" }
                div { class: "flex flex-wrap gap-2",
                    match &*phase.read() {
                        ConfigurePhase::Unsupported(reason) => rsx! {
                            p { class: "text-sm text-soft", "{reason}" }
                        },
                        ConfigurePhase::Probing => rsx! { p { class: "text-sm text-soft", "" } },
                        ConfigurePhase::Disconnected | ConfigurePhase::Failed(_) => rsx! {
                            button {
                                r#type: "button",
                                class: "configure-primary-action",
                                disabled: *busy.read(),
                                onclick: move |_| {
                                    phase.set(ConfigurePhase::Busy);
                                    status.set("Requesting WebUSB device…".to_string());
                                    spawn(async move {
                                        run_action(
                                            ConfigureRequest::Connect,
                                            phase, snapshot, status, busy,
                                            ed_freq, ed_sf, ed_bw, ed_cr,
                                            ed_txpower, ed_preamble, ed_region,
                                        ).await;
                                        // On a successful connect, immediately pull a snapshot.
                                        if *phase.read() == ConfigurePhase::Connected {
                                            run_action(
                                                ConfigureRequest::Snapshot,
                                                phase, snapshot, status, busy,
                                                ed_freq, ed_sf, ed_bw, ed_cr,
                                                ed_txpower, ed_preamble, ed_region,
                                            ).await;
                                        }
                                    });
                                },
                                "Connect over WebUSB"
                            }
                        },
                        ConfigurePhase::Busy => rsx! { p { class: "text-sm text-soft", "{status()}" } },
                        ConfigurePhase::Connected => rsx! {
                            button {
                                r#type: "button",
                                class: "configure-secondary-action",
                                disabled: *busy.read(),
                                onclick: action_handler(
                                    ConfigureRequest::Snapshot, phase, snapshot, status, busy,
                                    ed_freq, ed_sf, ed_bw, ed_cr, ed_txpower, ed_preamble, ed_region,
                                ),
                                "Refresh snapshot"
                            }
                            button {
                                r#type: "button",
                                class: "configure-secondary-action",
                                disabled: *busy.read(),
                                onclick: action_handler(
                                    ConfigureRequest::Close, phase, snapshot, status, busy,
                                    ed_freq, ed_sf, ed_bw, ed_cr, ed_txpower, ed_preamble, ed_region,
                                ),
                                "Disconnect"
                            }
                        },
                    }
                }
            }
            if !status().is_empty() && !matches!(*phase.read(), ConfigurePhase::Probing | ConfigurePhase::Busy) {
                p { class: "mt-3 text-sm text-soft", "{status()}" }
            }
            if let ConfigurePhase::Failed(detail) = &*phase.read() {
                p { class: "mt-3 text-sm text-soft", "{detail}" }
            }
        }

        if let Some(snap) = snapshot() {
            section { class: "mt-8 grid gap-4 md:grid-cols-2",
                { rsx! {
                    for section in &snap.sections {
                        { render_section(section) }
                    }
                }}
            }

            section { class: "mt-8 rounded-card border border-line/60 bg-layer/40 p-5",
                h2 { class: "text-xl font-semibold text-paper", "LoRa profile" }
                p { class: "mt-2 text-sm text-soft",
                    "Editing the profile persists it to flash and applies it to the radio. \
                     Reset returns to the factory 915 MHz default."
                }
                div { class: "mt-4 grid gap-4 md:grid-cols-2",
                    label { class: "configure-field",
                        span { class: "configure-field-label", "Frequency (Hz)" }
                        input {
                            r#type: "number",
                            class: "configure-input",
                            value: "{ed_freq()}",
                            oninput: move |evt| {
                                if let Ok(v) = evt.value().parse::<u32>() { ed_freq.set(v); }
                            },
                        }
                    }
                    label { class: "configure-field",
                        span { class: "configure-field-label", "Spreading factor (5–12)" }
                        input {
                            r#type: "number",
                            min: "5", max: "12",
                            class: "configure-input",
                            value: "{ed_sf()}",
                            oninput: move |evt| {
                                if let Ok(v) = evt.value().parse::<u8>() { ed_sf.set(v); }
                            },
                        }
                    }
                    label { class: "configure-field",
                        span { class: "configure-field-label", "Bandwidth (1=125k, 2=250k, 3=500k)" }
                        input {
                            r#type: "number",
                            min: "1", max: "3",
                            class: "configure-input",
                            value: "{ed_bw()}",
                            oninput: move |evt| {
                                if let Ok(v) = evt.value().parse::<u8>() { ed_bw.set(v); }
                            },
                        }
                    }
                    label { class: "configure-field",
                        span { class: "configure-field-label", "Coding rate (5–8)" }
                        input {
                            r#type: "number",
                            min: "5", max: "8",
                            class: "configure-input",
                            value: "{ed_cr()}",
                            oninput: move |evt| {
                                if let Ok(v) = evt.value().parse::<u8>() { ed_cr.set(v); }
                            },
                        }
                    }
                    label { class: "configure-field",
                        span { class: "configure-field-label", "TX power (dBm)" }
                        input {
                            r#type: "number",
                            class: "configure-input",
                            value: "{ed_txpower()}",
                            oninput: move |evt| {
                                if let Ok(v) = evt.value().parse::<i32>() { ed_txpower.set(v); }
                            },
                        }
                    }
                    label { class: "configure-field",
                        span { class: "configure-field-label", "Preamble" }
                        input {
                            r#type: "number",
                            class: "configure-input",
                            value: "{ed_preamble()}",
                            oninput: move |evt| {
                                if let Ok(v) = evt.value().parse::<u16>() { ed_preamble.set(v); }
                            },
                        }
                    }
                    label { class: "configure-field",
                        span { class: "configure-field-label", "Region (1=US915 … 12=EU868)" }
                        input {
                            r#type: "number",
                            min: "1", max: "12",
                            class: "configure-input",
                            value: "{ed_region()}",
                            oninput: move |evt| {
                                if let Ok(v) = evt.value().parse::<u8>() { ed_region.set(v); }
                            },
                        }
                    }
                }
                div { class: "mt-5 flex flex-wrap gap-2",
                    button {
                        r#type: "button",
                        class: "configure-primary-action",
                        disabled: !can_act,
                        onclick: move |_| {
                            let request = ConfigureRequest::ApplySetLoRaProfile {
                                frequency_hz: *ed_freq.read(),
                                spreading_factor: *ed_sf.read(),
                                bandwidth: *ed_bw.read(),
                                coding_rate: *ed_cr.read(),
                                tx_power_dbm: *ed_txpower.read(),
                                preamble: *ed_preamble.read(),
                                region_code: *ed_region.read(),
                            };
                            spawn(async move {
                                run_action(
                                    request, phase, snapshot, status, busy,
                                    ed_freq, ed_sf, ed_bw, ed_cr,
                                    ed_txpower, ed_preamble, ed_region,
                                ).await;
                                run_action(
                                    ConfigureRequest::Snapshot, phase, snapshot, status, busy,
                                    ed_freq, ed_sf, ed_bw, ed_cr,
                                    ed_txpower, ed_preamble, ed_region,
                                ).await;
                            });
                        },
                        "Save & apply profile"
                    }
                    button {
                        r#type: "button",
                        class: "configure-secondary-action",
                        disabled: !can_act,
                        onclick: action_handler(
                            ConfigureRequest::ApplyResetLoRaProfile, phase, snapshot, status, busy,
                            ed_freq, ed_sf, ed_bw, ed_cr, ed_txpower, ed_preamble, ed_region,
                        ),
                        "Reset to default"
                    }
                }
            }

            section { class: "mt-8 rounded-card border border-line/60 bg-layer/40 p-5",
                h2 { class: "text-xl font-semibold text-paper", "Interfaces & ephemeral actions" }
                div { class: "mt-4 flex flex-wrap gap-2",
                    button {
                        r#type: "button", class: "configure-secondary-action", disabled: !can_act,
                        onclick: toggle_handler(InterfaceKind::Lora, phase, snapshot, status, busy,
                            ed_freq, ed_sf, ed_bw, ed_cr, ed_txpower, ed_preamble, ed_region),
                        "Toggle LoRa"
                    }
                    button {
                        r#type: "button", class: "configure-secondary-action", disabled: !can_act,
                        onclick: toggle_handler(InterfaceKind::Usb, phase, snapshot, status, busy,
                            ed_freq, ed_sf, ed_bw, ed_cr, ed_txpower, ed_preamble, ed_region),
                        "Toggle USB"
                    }
                    button {
                        r#type: "button", class: "configure-secondary-action", disabled: !can_act,
                        onclick: toggle_handler(InterfaceKind::Ble, phase, snapshot, status, busy,
                            ed_freq, ed_sf, ed_bw, ed_cr, ed_txpower, ed_preamble, ed_region),
                        "Toggle BLE"
                    }
                    button {
                        r#type: "button", class: "configure-secondary-action", disabled: !can_act,
                        onclick: action_handler(
                            ConfigureRequest::ApplySleep, phase, snapshot, status, busy,
                            ed_freq, ed_sf, ed_bw, ed_cr, ed_txpower, ed_preamble, ed_region,
                        ),
                        "Sleep"
                    }
                    button {
                        r#type: "button", class: "configure-secondary-action", disabled: !can_act,
                        onclick: action_handler(
                            ConfigureRequest::ApplyWake, phase, snapshot, status, busy,
                            ed_freq, ed_sf, ed_bw, ed_cr, ed_txpower, ed_preamble, ed_region,
                        ),
                        "Wake"
                    }
                    button {
                        r#type: "button", class: "configure-secondary-action", disabled: !can_act,
                        onclick: action_handler(
                            ConfigureRequest::ApplyAnnounce, phase, snapshot, status, busy,
                            ed_freq, ed_sf, ed_bw, ed_cr, ed_txpower, ed_preamble, ed_region,
                        ),
                        "Announce now"
                    }
                }
            }
        }
    }
}

/// Run a one-shot bridge action under the `busy` guard and route the event.
/// Signals are taken by value because `Signal<T>` is `Copy` (interior-mutable:
/// `.set()` takes `&self`), so handlers capture cheap copies without cloning.
#[allow(clippy::too_many_arguments)]
async fn run_action(
    request: ConfigureRequest,
    mut phase: Signal<ConfigurePhase>,
    snapshot: Signal<Option<ConfigureSnapshot>>,
    status: Signal<String>,
    mut busy: Signal<bool>,
    ed_freq: Signal<u32>,
    ed_sf: Signal<u8>,
    ed_bw: Signal<u8>,
    ed_cr: Signal<u8>,
    ed_txpower: Signal<i32>,
    ed_preamble: Signal<u16>,
    ed_region: Signal<u8>,
) {
    busy.set(true);
    let event = bridge::dispatch(&request).await;
    busy.set(false);
    let event = match event {
        Some(event) => event,
        None => {
            phase.set(ConfigurePhase::Failed(
                "The config engine stopped unexpectedly.".to_string(),
            ));
            return;
        }
    };
    apply_event(
        event,
        phase,
        snapshot,
        status,
        ed_freq,
        ed_sf,
        ed_bw,
        ed_cr,
        ed_txpower,
        ed_preamble,
        ed_region,
    );
}

#[allow(clippy::too_many_arguments)]
fn apply_event(
    event: ConfigureEvent,
    mut phase: Signal<ConfigurePhase>,
    mut snapshot: Signal<Option<ConfigureSnapshot>>,
    mut status: Signal<String>,
    ed_freq: Signal<u32>,
    ed_sf: Signal<u8>,
    ed_bw: Signal<u8>,
    ed_cr: Signal<u8>,
    ed_txpower: Signal<i32>,
    ed_preamble: Signal<u16>,
    ed_region: Signal<u8>,
) {
    match event {
        ConfigureEvent::Ready { supported, reason } => {
            if supported {
                phase.set(ConfigurePhase::Disconnected);
            } else {
                phase.set(ConfigurePhase::Unsupported(
                    reason.unwrap_or_else(|| "WebUSB is not available.".to_string()),
                ));
            }
        }
        ConfigureEvent::Connected => {
            phase.set(ConfigurePhase::Connected);
            status.set("Connected. Requesting snapshot…".to_string());
        }
        ConfigureEvent::ConnectFailed { code, detail } => {
            phase.set(ConfigurePhase::Disconnected);
            status.set(format!("Connect failed ({code}): {detail}"));
        }
        ConfigureEvent::Snapshot { snapshot: snap } => {
            populate_editor(
                &snap,
                ed_freq,
                ed_sf,
                ed_bw,
                ed_cr,
                ed_txpower,
                ed_preamble,
                ed_region,
            );
            snapshot.set(Some(snap));
            status.set("Snapshot loaded.".to_string());
        }
        ConfigureEvent::SnapshotFailed { code, detail } => {
            status.set(format!("Snapshot failed ({code}): {detail}"));
        }
        ConfigureEvent::Applied { result } => {
            status.set(format!("Applied: {}.", result_label(result)));
        }
        ConfigureEvent::ApplyFailed { code, detail } => {
            status.set(format!("Apply failed ({code}): {detail}"));
        }
        ConfigureEvent::Closed => {
            phase.set(ConfigurePhase::Disconnected);
            snapshot.set(None);
            status.set("Disconnected.".to_string());
        }
        ConfigureEvent::SessionFailed { code, detail } => {
            phase.set(ConfigurePhase::Failed(format!(
                "Session failed ({code}): {detail}"
            )));
            snapshot.set(None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn populate_editor(
    snap: &ConfigureSnapshot,
    mut ed_freq: Signal<u32>,
    mut ed_sf: Signal<u8>,
    mut ed_bw: Signal<u8>,
    mut ed_cr: Signal<u8>,
    mut ed_txpower: Signal<i32>,
    mut ed_preamble: Signal<u16>,
    mut ed_region: Signal<u8>,
) {
    for section in &snap.sections {
        if let ConfigureSection::RadioProfile { profile } = section {
            ed_freq.set(profile.frequency_hz);
            ed_sf.set(profile.spreading_factor);
            ed_bw.set(profile.bandwidth);
            ed_cr.set(profile.coding_rate);
            ed_txpower.set(profile.tx_power_dbm);
            ed_preamble.set(profile.preamble);
            ed_region.set(profile.region_code);
        }
    }
}

fn result_label(result: ConfigResult) -> &'static str {
    match result {
        ConfigResult::Ok => "ok",
        ConfigResult::ApplyFailed => "apply failed",
        ConfigResult::ProfileNotSaved => "profile not saved",
        ConfigResult::Rejected => "rejected",
        ConfigResult::BadPayload => "bad payload",
    }
}

/// Build an `onclick` handler that fires a no-arg action and refreshes the
/// snapshot after ephemeral actions. The returned closure is `FnMut` (a button
/// can fire more than once), so the non-`Copy` request is cloned per invocation;
/// the signals are `Copy` and captured directly.
#[allow(clippy::too_many_arguments)]
fn action_handler(
    request: ConfigureRequest,
    phase: Signal<ConfigurePhase>,
    snapshot: Signal<Option<ConfigureSnapshot>>,
    status: Signal<String>,
    busy: Signal<bool>,
    ed_freq: Signal<u32>,
    ed_sf: Signal<u8>,
    ed_bw: Signal<u8>,
    ed_cr: Signal<u8>,
    ed_txpower: Signal<i32>,
    ed_preamble: Signal<u16>,
    ed_region: Signal<u8>,
) -> impl FnMut(MouseEvent) + 'static {
    move |_| {
        // `FnMut` may fire more than once; clone the request per invocation so
        // the captured copy is never consumed.
        let request = request.clone();
        spawn(async move {
            run_action(
                request.clone(),
                phase,
                snapshot,
                status,
                busy,
                ed_freq,
                ed_sf,
                ed_bw,
                ed_cr,
                ed_txpower,
                ed_preamble,
                ed_region,
            )
            .await;
            // Refresh after an ephemeral action so the UI reflects the new state.
            if matches!(
                request,
                ConfigureRequest::ApplyResetLoRaProfile
                    | ConfigureRequest::ApplySleep
                    | ConfigureRequest::ApplyWake
                    | ConfigureRequest::ApplyToggleInterface { .. }
                    | ConfigureRequest::ApplyAnnounce
            ) {
                run_action(
                    ConfigureRequest::Snapshot,
                    phase,
                    snapshot,
                    status,
                    busy,
                    ed_freq,
                    ed_sf,
                    ed_bw,
                    ed_cr,
                    ed_txpower,
                    ed_preamble,
                    ed_region,
                )
                .await;
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn toggle_handler(
    kind: InterfaceKind,
    phase: Signal<ConfigurePhase>,
    snapshot: Signal<Option<ConfigureSnapshot>>,
    status: Signal<String>,
    busy: Signal<bool>,
    ed_freq: Signal<u32>,
    ed_sf: Signal<u8>,
    ed_bw: Signal<u8>,
    ed_cr: Signal<u8>,
    ed_txpower: Signal<i32>,
    ed_preamble: Signal<u16>,
    ed_region: Signal<u8>,
) -> impl FnMut(MouseEvent) + 'static {
    action_handler(
        ConfigureRequest::ApplyToggleInterface {
            interface_code: kind.wire_code(),
        },
        phase,
        snapshot,
        status,
        busy,
        ed_freq,
        ed_sf,
        ed_bw,
        ed_cr,
        ed_txpower,
        ed_preamble,
        ed_region,
    )
}

fn render_section(section: &ConfigureSection) -> Element {
    match section {
        ConfigureSection::DeviceInfo { version } => rsx! {
            div { class: "rounded-card border border-line/60 bg-layer/40 p-4",
                h3 { class: "text-sm font-semibold tracking-wide uppercase text-accent", "Device" }
                p { class: "mt-2 text-paper", "Firmware {version}" }
            }
        },
        ConfigureSection::Persistence { state } => rsx! {
            div { class: "rounded-card border border-line/60 bg-layer/40 p-4",
                h3 { class: "text-sm font-semibold tracking-wide uppercase text-accent", "Persistence" }
                p { class: "mt-2 text-paper", "{state:?}" }
            }
        },
        ConfigureSection::LoraStatus { status } => rsx! {
            { render_interface_status("LoRa", status) }
        },
        ConfigureSection::UsbStatus { status } => rsx! {
            { render_interface_status("USB", status) }
        },
        ConfigureSection::BleStatus {
            status,
            failure_reason,
        } => rsx! {
            div { class: "rounded-card border border-line/60 bg-layer/40 p-4",
                h3 { class: "text-sm font-semibold tracking-wide uppercase text-accent", "BLE" }
                { render_status_lines(status) }
                if !failure_reason.is_empty() {
                    p { class: "mt-1 text-sm text-soft", "Reason: {failure_reason}" }
                }
            }
        },
        ConfigureSection::BleRecovery {
            ingress_pressure,
            setup_failures,
            transport_closures,
            egress_pressure_events,
            member_count,
        } => rsx! {
            div { class: "rounded-card border border-line/60 bg-layer/40 p-4",
                h3 { class: "text-sm font-semibold tracking-wide uppercase text-accent", "BLE recovery" }
                dl { class: "mt-2 grid grid-cols-2 gap-y-1 text-sm",
                    dt { class: "text-soft", "Ingress pressure" } dd { class: "text-paper", "{ingress_pressure}" }
                    dt { class: "text-soft", "Setup failures" }   dd { class: "text-paper", "{setup_failures}" }
                    dt { class: "text-soft", "Transport closures" } dd { class: "text-paper", "{transport_closures}" }
                    dt { class: "text-soft", "Egress pressure events" } dd { class: "text-paper", "{egress_pressure_events}" }
                    dt { class: "text-soft", "Members" } dd { class: "text-paper", "{member_count}" }
                }
            }
        },
        ConfigureSection::LoraSpectrum {
            channel_busy_per_mille,
            noise_floor_dbm,
            cca_threshold_dbm,
            deferrals,
            false_preambles,
            contention_timeouts,
            duty_holds,
            duty_timeouts,
            radio_recoveries,
        } => rsx! {
            div { class: "rounded-card border border-line/60 bg-layer/40 p-4",
                h3 { class: "text-sm font-semibold tracking-wide uppercase text-accent", "LoRa spectrum" }
                dl { class: "mt-2 grid grid-cols-2 gap-y-1 text-sm",
                    dt { class: "text-soft", "Channel busy" } dd { class: "text-paper", "{channel_busy_per_mille}‰" }
                    if let Some(n) = noise_floor_dbm {
                        dt { class: "text-soft", "Noise floor" } dd { class: "text-paper", "{n} dBm" }
                    }
                    if let Some(c) = cca_threshold_dbm {
                        dt { class: "text-soft", "CCA threshold" } dd { class: "text-paper", "{c} dBm" }
                    }
                    dt { class: "text-soft", "Deferrals" } dd { class: "text-paper", "{deferrals}" }
                    dt { class: "text-soft", "False preambles" } dd { class: "text-paper", "{false_preambles}" }
                    dt { class: "text-soft", "Contention timeouts" } dd { class: "text-paper", "{contention_timeouts}" }
                    dt { class: "text-soft", "Duty holds" } dd { class: "text-paper", "{duty_holds}" }
                    dt { class: "text-soft", "Duty timeouts" } dd { class: "text-paper", "{duty_timeouts}" }
                    dt { class: "text-soft", "Radio recoveries" } dd { class: "text-paper", "{radio_recoveries}" }
                }
            }
        },
        ConfigureSection::RadioProfile { profile } => rsx! {
            div { class: "rounded-card border border-line/60 bg-layer/40 p-4",
                h3 { class: "text-sm font-semibold tracking-wide uppercase text-accent", "Active radio profile" }
                dl { class: "mt-2 grid grid-cols-2 gap-y-1 text-sm",
                    dt { class: "text-soft", "Frequency" } dd { class: "text-paper", "{profile.frequency_hz} Hz" }
                    dt { class: "text-soft", "Spreading factor" } dd { class: "text-paper", "{profile.spreading_factor}" }
                    dt { class: "text-soft", "Bandwidth" } dd { class: "text-paper", "{profile.bandwidth}" }
                    dt { class: "text-soft", "Coding rate" } dd { class: "text-paper", "{profile.coding_rate}" }
                    dt { class: "text-soft", "TX power" } dd { class: "text-paper", "{profile.tx_power_dbm} dBm" }
                    dt { class: "text-soft", "Preamble" } dd { class: "text-paper", "{profile.preamble}" }
                    dt { class: "text-soft", "Region" } dd { class: "text-paper", "{profile.region}" }
                }
            }
        },
        ConfigureSection::InterfaceCounts { counts } => rsx! {
            div { class: "rounded-card border border-line/60 bg-layer/40 p-4",
                h3 { class: "text-sm font-semibold tracking-wide uppercase text-accent", "Interface counts" }
                table { class: "mt-2 w-full text-sm",
                    thead {
                        tr {
                            th { class: "text-left text-soft", "Interface" }
                            th { class: "text-right text-soft", "Destinations" }
                            th { class: "text-right text-soft", "Links" }
                            th { class: "text-right text-soft", "Transported" }
                        }
                    }
                    tbody {
                        for count in counts {
                            tr {
                                td { class: "text-paper", "{count.kind:?}" }
                                td { class: "text-right text-paper", "{count.destinations}" }
                                td { class: "text-right text-paper", "{count.links}" }
                                td { class: "text-right text-paper", "{count.transported_links}" }
                            }
                        }
                    }
                }
            }
        },
    }
}

fn render_interface_status(name: &str, status: &bridge::InterfaceStatus) -> Element {
    rsx! {
        div { class: "rounded-card border border-line/60 bg-layer/40 p-4",
            h3 { class: "text-sm font-semibold tracking-wide uppercase text-accent", "{name}" }
            { render_status_lines(status) }
        }
    }
}

fn render_status_lines(status: &bridge::InterfaceStatus) -> Element {
    rsx! {
        dl { class: "mt-2 grid grid-cols-2 gap-y-1 text-sm",
            dt { class: "text-soft", "Enabled" }     dd { class: "text-paper", "{status.enabled}" }
            dt { class: "text-soft", "Connection" }  dd { class: "text-paper", "{status.connection:?}" }
            dt { class: "text-soft", "RX bytes" }    dd { class: "text-paper", "{status.rx_bytes}" }
            dt { class: "text-soft", "TX bytes" }    dd { class: "text-paper", "{status.tx_bytes}" }
            if let Some(rates) = status.transfer_rates {
                dt { class: "text-soft", "RX rate" } dd { class: "text-paper", "{rates.rx_bps} bps" }
                dt { class: "text-soft", "TX rate" } dd { class: "text-paper", "{rates.tx_bps} bps" }
            }
            if let Some(air) = status.airtime {
                dt { class: "text-soft", "Airtime short" } dd { class: "text-paper", "{air.short_per_mille}‰" }
                dt { class: "text-soft", "Airtime long" }  dd { class: "text-paper", "{air.long_per_mille}‰" }
            }
        }
    }
}
