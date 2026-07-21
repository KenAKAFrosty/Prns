use dioxus::prelude::*;
use dioxus_i18n::t;
use prns_flash_manifest::{
    ChannelDescriptor, FlashManifest, FlashPartKind, PINNED_MINISIGN_PUBLIC_KEY,
    ProvisioningAction, ReleaseChannel, TargetManifest, Transport, WifiCredentials,
    board_catalog, pinned_key_id, pinned_key_is_configured, provisioning_image, sha256_hex,
    verify_minisign,
};
use serde::{Deserialize, Serialize};

use crate::components::PlatformChip;
use crate::platforms::{
    BoardTarget, ROADMAP_BOARD_TARGETS, SHIPPING_BOARD_TARGETS, board_target_by_slug,
};
use crate::routes::Route;
use crate::site_mode::embedded_docs_mode;

const RELEASE_CHANNEL: &str = env!("PRNS_BUILD_CHANNEL");

const FETCH_CHANNEL_SCRIPT: &str = r#"
const channel = '__PRNS_RELEASE_CHANNEL__';
const descriptor = await fetch(`/releases/channels/${channel}.json`, { cache: 'no-store', credentials: 'omit', redirect: 'error' });
const signature = await fetch(`/releases/channels/${channel}.json.minisig`, { cache: 'no-store', credentials: 'omit', redirect: 'error' });
if (!descriptor.ok || !signature.ok) throw new Error('release channel documents unavailable');
dioxus.send({ descriptor: await descriptor.text(), signature: await signature.text() });
"#;

const FETCH_MANIFEST_SCRIPT: &str = r#"
const manifestUrl = await dioxus.recv();
const immutable = new URL(manifestUrl);
const localQualification = ['localhost', '127.0.0.1', '::1'].includes(location.hostname);
const resolvedUrl = localQualification ? immutable.pathname : immutable.href;
const manifest = await fetch(resolvedUrl, { cache: 'no-store', credentials: 'omit', redirect: 'error' });
const signature = await fetch(`${resolvedUrl}.minisig`, { cache: 'no-store', credentials: 'omit', redirect: 'error' });
if (!manifest.ok || !signature.ok) throw new Error('immutable release documents unavailable');
dioxus.send({ manifest: await manifest.text(), signature: await signature.text() });
"#;

const PREPARE_SCRIPT: &str = r#"
const request = await dioxus.recv();
window.__prnsFlash = window.__prnsFlash || await import('/assets/flasher/prns-flash.js');
try {
  await window.__prnsFlash.prepare(request, event => dioxus.send(event));
} catch (_) {}
"#;

const FLASH_SCRIPT: &str = r#"
try {
  await window.__prnsFlash.flash(event => dioxus.send(event));
} catch (_) {}
"#;

const BROWSER_SUPPORT_SCRIPT: &str =
    "return Boolean(window.isSecureContext && navigator.serial && navigator.serial.requestPort);";

const FOCUS_STATUS_SCRIPT: &str =
    "document.getElementById('flash-status')?.focus({ preventScroll: false });";

#[component]
pub fn FlashPage() -> Element {
    rsx! { FlashExperience { selected_slug: None } }
}

#[component]
pub fn FlashBoardPage(board: String) -> Element {
    rsx! { FlashExperience { selected_slug: Some(board) } }
}

#[component]
fn FlashExperience(selected_slug: Option<String>) -> Element {
    let selected_target = selected_slug.as_deref().and_then(board_target_by_slug);
    let missing_selection = selected_slug.is_some() && selected_target.is_none();

    rsx! {
        header { class: "mb-10",
            Link {
                to: if selected_slug.is_some() { Route::FlashPage {} } else { Route::PlatformsPage {} },
                class: "text-sm text-soft hover:text-accent transition-colors",
                "← "
                if selected_slug.is_some() { {t!("flash-back-boards")} } else { {t!("flash-back")} }
            }
            p { class: "mt-6 text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                "Release flasher"
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                "Flash a Personal Hopspot"
            }
            p { class: "mt-4 max-w-3xl leading-relaxed text-soft",
                "Choose the exact board, verify the signed release locally, then write only its sparse firmware parts. Existing Wi-Fi is preserved unless you explicitly change it."
            }
        }

        if let Some(target) = selected_target {
            if target.is_flashable() {
                GuidedFlasher { target }
            } else {
                UnavailablePanel {}
            }
        } else if missing_selection {
            section { class: "rounded-card border border-line/60 bg-layer/40 p-5",
                h2 { class: "text-xl font-semibold text-paper", "Board not found" }
                p { class: "mt-3 text-soft", "Choose one of the supported shipping boards below." }
            }
        }

        section { class: if selected_target.is_some() { "mt-12" } else { "mt-4" },
            h2 { class: "text-2xl font-semibold tracking-tight text-paper",
                if selected_target.is_some() { "Change board" } else { "Select the exact board" }
            }
            p { class: "mt-3 max-w-3xl leading-relaxed text-soft",
                "The four shipping targets are first. Hardware still in bring-up remains visible, but cannot be flashed from a public release."
            }
            div { class: "mt-6 grid gap-4 md:grid-cols-2",
                for board in SHIPPING_BOARD_TARGETS.iter() {
                    BoardTargetCard {
                        key: "{board.slug}",
                        board,
                        selected: selected_target.is_some_and(|target| target.slug == board.slug),
                    }
                }
            }
            details { class: "mt-6 rounded-card border border-line/50 bg-layer/30 p-4",
                summary { class: "cursor-pointer font-semibold text-soft", "Coming later" }
                div { class: "mt-4 grid gap-4 md:grid-cols-2",
                    for board in ROADMAP_BOARD_TARGETS.iter() {
                        BoardTargetCard { key: "{board.slug}", board, selected: false }
                    }
                }
            }
        }
    }
}

#[component]
fn GuidedFlasher(target: &'static BoardTarget) -> Element {
    let embedded = embedded_docs_mode();
    let key_ready = pinned_key_is_configured();
    let is_esp = matches!(target.slug, "heltec-v4" | "t-beam-supreme" | "xiao-esp32-c6");
    let supports_wifi = matches!(target.slug, "heltec-v4" | "t-beam-supreme");
    let is_uf2 = target.slug == "t-echo";

    let mut confirmed = use_signal(|| false);
    let mut wifi_action = use_signal(|| "preserve".to_string());
    let mut ssid = use_signal(String::new);
    let mut password = use_signal(String::new);
    let phase = use_signal(|| "idle".to_string());
    let mut status = use_signal(|| "Confirm the exact board to begin.".to_string());
    let progress_current = use_signal(|| 0_u64);
    let progress_total = use_signal(|| 0_u64);
    let mut prepared = use_signal(|| false);
    let mut release = use_signal(|| None::<ReleaseDetails>);
    let mut web_serial = use_signal(|| None::<bool>);

    use_effect(move || {
        if is_esp && !embedded {
            spawn(async move {
                if let Ok(supported) = document::eval(BROWSER_SUPPORT_SCRIPT).join::<bool>().await {
                    web_serial.set(Some(supported));
                }
            });
        }
    });

    let busy = is_busy(&phase());
    let browser_blocked = is_esp && web_serial() == Some(false);
    let can_prepare = confirmed() && !busy && !embedded && key_ready && !browser_blocked;
    let can_flash = prepared() && !busy;
    let action_label = if is_uf2 {
        "Download verified UF2"
    } else {
        "Connect and flash"
    };

    rsx! {
        section { class: "flash-flasher-panel",
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
                        onchange: move |event| {
                            confirmed.set(event.checked());
                            prepared.set(false);
                            document::eval("window.__prnsFlash?.clearPrepared();");
                        },
                    }
                    span {
                        "I checked the board label and image: this is "
                        strong { class: "text-paper", "{target.name}" }
                        if target.slug == "heltec-v4" || target.slug == "t-beam-supreme" {
                            span { class: "mt-1 block text-xs text-mid",
                                "Heltec V4 and T-Beam Supreme share ESP32-S3 silicon, so software cannot distinguish the exact model."
                            }
                        }
                    }
                }

                if supports_wifi {
                    fieldset { class: "flash-wifi-config mt-5",
                        legend { class: "font-semibold text-paper", "Wi-Fi configuration" }
                        p { class: "flash-wifi-note mt-2",
                            "Credentials remain in this browser and are never sent to a server. Preserve is the default."
                        }
                        div { class: "grid gap-2 text-sm text-soft",
                            for (value, label) in [
                                ("preserve", "Preserve existing configuration"),
                                ("configure", "Configure a network locally"),
                                ("clear", "Clear configuration explicitly"),
                            ] {
                                label { class: "flex items-center gap-2",
                                    input {
                                        r#type: "radio",
                                        name: "wifi-action",
                                        value,
                                        checked: wifi_action() == value,
                                        onchange: move |_| {
                                            wifi_action.set(value.to_string());
                                            prepared.set(false);
                                            document::eval("window.__prnsFlash?.clearPrepared();");
                                        },
                                    }
                                    "{label}"
                                }
                            }
                        }
                        if wifi_action() == "configure" {
                            div { class: "flash-wifi-grid mt-4",
                                label { class: "flash-wifi-field",
                                    span { "SSID" }
                                    input {
                                        value: ssid(),
                                        maxlength: "32",
                                        autocomplete: "off",
                                        oninput: move |event| {
                                            ssid.set(event.value());
                                            prepared.set(false);
                                            document::eval("window.__prnsFlash?.clearPrepared();");
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
                                        oninput: move |event| {
                                            password.set(event.value());
                                            prepared.set(false);
                                            document::eval("window.__prnsFlash?.clearPrepared();");
                                        },
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "flash-plan-panel mt-5",
                    div { class: "flash-plan-panel__head",
                        h3 { class: "font-semibold text-paper", "Review and verify" }
                        span { class: status_class(&phase()), "{phase_label(&phase())}" }
                    }
                    ol { class: "flash-step-list mt-4",
                        for (index, step) in guided_steps(is_uf2).iter().enumerate() {
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

                if let Some(details) = release() {
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
                    onclick: move |_| {
                        let target_slug = target.slug.to_string();
                        let selected_action = wifi_action();
                        let selected_ssid = ssid();
                        let selected_password = password();
                        prepared.set(false);
                        release.set(None);
                        spawn(async move {
                            prepare_release(
                                target_slug,
                                selected_action,
                                selected_ssid,
                                selected_password,
                                phase,
                                status,
                                progress_current,
                                progress_total,
                                prepared,
                                release,
                                ssid,
                                password,
                            )
                            .await;
                        });
                    },
                    if prepared() { "Re-verify release" } else { "Prepare and verify release" }
                }
                button {
                    r#type: "button",
                    class: "flash-primary-action",
                    disabled: !can_flash,
                    onclick: move |_| {
                        spawn(async move {
                            run_flash(
                                phase,
                                status,
                                progress_current,
                                progress_total,
                                prepared,
                                ssid,
                                password,
                            )
                            .await;
                        });
                    },
                    "{action_label}"
                }
                if busy {
                    button {
                        r#type: "button",
                        class: "rounded-lg border border-line px-4 py-3 text-sm font-semibold text-soft",
                        onclick: move |_| {
                            document::eval("window.__prnsFlash?.cancel();");
                            status.set("Cancellation requested; an active write will finish its safe operation before stopping.".to_string());
                        },
                        "Cancel safely"
                    }
                }
            }
        }
    }
}

#[component]
fn BoardTargetCard(board: &'static BoardTarget, selected: bool) -> Element {
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
                p { class: "flash-interfaces-pending mt-4", "Coming later" }
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
fn UnavailablePanel() -> Element {
    rsx! {
        section { class: "rounded-card border border-line/60 bg-layer/40 p-5",
            h2 { class: "text-xl font-semibold text-paper", "Not flashable yet" }
            p { class: "mt-3 text-soft", "This target is still in bring-up or roadmap tracking." }
        }
    }
}

#[derive(Deserialize)]
struct ChannelDocuments {
    descriptor: String,
    signature: String,
}

#[derive(Deserialize)]
struct ManifestDocuments {
    manifest: String,
    signature: String,
}

#[derive(Clone)]
struct ReleaseDetails {
    version: String,
    channel: String,
    total: u64,
    parts: Vec<PartDetails>,
}

#[derive(Clone)]
struct PartDetails {
    kind: &'static str,
    size: u64,
    sha256: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeRequest {
    schema: u8,
    board_slug: String,
    display_name: String,
    transport: Transport,
    expected_chip: Option<String>,
    flash_size: Option<u32>,
    flash_mode: Option<String>,
    flash_frequency: Option<String>,
    before_reset: Option<String>,
    after_reset: Option<String>,
    provisioning: Option<BridgeProvisioning>,
    parts: Vec<BridgePart>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgePart {
    kind: &'static str,
    path: String,
    url: String,
    offset: Option<u32>,
    size: u64,
    sha256: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeProvisioning {
    action: String,
    offset: u32,
    size: u32,
    ssid: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeEvent {
    phase: String,
    code: Option<String>,
    message: Option<String>,
    current: Option<u64>,
    total: Option<u64>,
    part: Option<String>,
    part_index: Option<usize>,
    part_count: Option<usize>,
    detected_chip: Option<String>,
    bytes: Option<u64>,
}

#[allow(clippy::too_many_arguments)]
async fn prepare_release(
    board_slug: String,
    selected_action: String,
    ssid_value: String,
    password_value: String,
    mut phase: Signal<String>,
    mut status: Signal<String>,
    mut progress_current: Signal<u64>,
    mut progress_total: Signal<u64>,
    mut prepared: Signal<bool>,
    mut release: Signal<Option<ReleaseDetails>>,
    mut ssid_input: Signal<String>,
    mut password: Signal<String>,
) {
    phase.set("validating_manifest".to_string());
    status.set("Downloading and verifying the signed release manifest…".to_string());
    progress_current.set(0);
    progress_total.set(0);

    let result = async {
        if !pinned_key_is_configured() {
            return Err("Release signing custody is not configured.".to_string());
        }
        let channel_script =
            FETCH_CHANNEL_SCRIPT.replace("__PRNS_RELEASE_CHANNEL__", RELEASE_CHANNEL);
        let mut channel_eval = document::eval(&channel_script);
        let channel_documents = channel_eval
            .recv::<ChannelDocuments>()
            .await
            .map_err(|_| format!("The signed {RELEASE_CHANNEL} channel is unavailable."))?;
        verify_minisign(
            channel_documents.descriptor.as_bytes(),
            &channel_documents.signature,
            PINNED_MINISIGN_PUBLIC_KEY,
        )
        .map_err(|error| error.to_string())?;
        let descriptor = ChannelDescriptor::from_json(
            channel_documents.descriptor.as_bytes(),
            configured_release_channel(),
        )
        .map_err(|error| error.to_string())?;

        let mut manifest_eval = document::eval(FETCH_MANIFEST_SCRIPT);
        manifest_eval
            .send(descriptor.manifest_url.clone())
            .map_err(|_| "Could not request the immutable release manifest.".to_string())?;
        let documents = manifest_eval
            .recv::<ManifestDocuments>()
            .await
            .map_err(|_| "The immutable signed release is unavailable.".to_string())?;
        if sha256_hex(documents.manifest.as_bytes()) != descriptor.manifest_sha256 {
            return Err("The manifest does not match the signed release channel.".to_string());
        }
        verify_minisign(
            documents.manifest.as_bytes(),
            &documents.signature,
            PINNED_MINISIGN_PUBLIC_KEY,
        )
        .map_err(|error| error.to_string())?;
        let catalog = board_catalog().map_err(|error| error.to_string())?;
        let manifest = FlashManifest::from_json(documents.manifest.as_bytes(), &catalog)
            .map_err(|error| error.to_string())?;
        let expected_key_id = pinned_key_id()
            .ok_or_else(|| "The pinned release key has no canonical key ID.".to_string())?;
        if !manifest.signing.key_id.eq_ignore_ascii_case(&expected_key_id) {
            return Err("The signed manifest names a different release key.".to_string());
        }
        if manifest.release.version != descriptor.version
            || manifest.release.channel != descriptor.channel
        {
            return Err("The signed channel and manifest release identity disagree.".to_string());
        }
        let target = manifest
            .targets
            .iter()
            .find(|target| target.board_slug == board_slug)
            .cloned()
            .ok_or_else(|| "The signed release does not contain this board.".to_string())?;
        let provisioning = bridge_provisioning(
            &target,
            &selected_action,
            ssid_value,
            password_value,
        )?;
        let request = bridge_request(&target, &descriptor.manifest_url, provisioning)?;
        let details = ReleaseDetails {
            version: manifest.release.version,
            channel: match manifest.release.channel {
                prns_flash_manifest::ReleaseChannel::Stable => "stable".to_string(),
                prns_flash_manifest::ReleaseChannel::Preview => "preview".to_string(),
            },
            total: target.parts.iter().map(|part| part.size).sum(),
            parts: target
                .parts
                .iter()
                .map(|part| PartDetails {
                    kind: part_kind(part.kind),
                    size: part.size,
                    sha256: part.sha256.clone(),
                })
                .collect(),
        };

        let mut bridge = document::eval(PREPARE_SCRIPT);
        bridge
            .send(request)
            .map_err(|_| "Could not start the local flasher engine.".to_string())?;
        loop {
            let event = bridge
                .recv::<BridgeEvent>()
                .await
                .map_err(|_| "The local flasher engine stopped unexpectedly.".to_string())?;
            let terminal = apply_event(
                &event,
                &mut phase,
                &mut status,
                &mut progress_current,
                &mut progress_total,
            );
            if terminal {
                if event.phase == "ready" {
                    release.set(Some(details));
                    prepared.set(true);
                    password.set(String::new());
                    return Ok(());
                }
                return Err(event
                    .message
                    .unwrap_or_else(|| "Release preparation failed safely.".to_string()));
            }
        }
    }
    .await;

    if let Err(message) = result {
        phase.set("failed".to_string());
        status.set(message);
        prepared.set(false);
        ssid_input.set(String::new());
        password.set(String::new());
        focus_status();
    }
}

fn configured_release_channel() -> ReleaseChannel {
    match RELEASE_CHANNEL {
        "stable" => ReleaseChannel::Stable,
        "preview" => ReleaseChannel::Preview,
        _ => panic!("unsupported compiled release channel"),
    }
}

async fn run_flash(
    mut phase: Signal<String>,
    mut status: Signal<String>,
    mut progress_current: Signal<u64>,
    mut progress_total: Signal<u64>,
    mut prepared: Signal<bool>,
    mut ssid: Signal<String>,
    mut password: Signal<String>,
) {
    phase.set("requesting_port".to_string());
    status.set("Waiting for the browser's device picker…".to_string());
    let mut bridge = document::eval(FLASH_SCRIPT);
    loop {
        let event = match bridge.recv::<BridgeEvent>().await {
            Ok(event) => event,
            Err(_) => {
                phase.set("failed".to_string());
                status.set("The local device engine stopped unexpectedly. No success was reported.".to_string());
                ssid.set(String::new());
                password.set(String::new());
                focus_status();
                return;
            }
        };
        let terminal = apply_event(
            &event,
            &mut phase,
            &mut status,
            &mut progress_current,
            &mut progress_total,
        );
        if terminal {
            prepared.set(false);
            ssid.set(String::new());
            password.set(String::new());
            focus_status();
            return;
        }
    }
}

fn bridge_provisioning(
    target: &TargetManifest,
    action: &str,
    ssid: String,
    password: String,
) -> Result<Option<BridgeProvisioning>, String> {
    let Some(slot) = &target.provisioning else {
        return Ok(None);
    };
    let provisioning_action = match action {
        "preserve" => ProvisioningAction::Preserve,
        "clear" => ProvisioningAction::Clear,
        "configure" => ProvisioningAction::Configure(WifiCredentials {
            ssid: ssid.clone(),
            password: password.clone(),
        }),
        _ => return Err("Unknown provisioning action.".to_string()),
    };
    provisioning_image(&provisioning_action).map_err(|error| error.to_string())?;
    Ok(Some(BridgeProvisioning {
        action: action.to_string(),
        offset: slot.offset,
        size: slot.size,
        ssid: if action == "configure" { ssid } else { String::new() },
        password: if action == "configure" {
            password
        } else {
            String::new()
        },
    }))
}

fn bridge_request(
    target: &TargetManifest,
    manifest_url: &str,
    provisioning: Option<BridgeProvisioning>,
) -> Result<BridgeRequest, String> {
    let (base_url, _) = manifest_url
        .rsplit_once('/')
        .ok_or_else(|| "The immutable manifest URL has no release directory.".to_string())?;
    Ok(BridgeRequest {
        schema: 1,
        board_slug: target.board_slug.clone(),
        display_name: target.display_name.clone(),
        transport: target.transport,
        expected_chip: target.expected_chip.clone(),
        flash_size: target.flash_size,
        flash_mode: target.flash_mode.clone(),
        flash_frequency: target.flash_frequency.clone(),
        before_reset: target.before_reset.clone(),
        after_reset: target.after_reset.clone(),
        provisioning,
        parts: target
            .parts
            .iter()
            .map(|part| BridgePart {
                kind: part_kind(part.kind),
                path: part.path.clone(),
                url: format!("{base_url}/{}", part.path),
                offset: part.offset,
                size: part.size,
                sha256: part.sha256.clone(),
            })
            .collect(),
    })
}

fn apply_event(
    event: &BridgeEvent,
    phase: &mut Signal<String>,
    status: &mut Signal<String>,
    current: &mut Signal<u64>,
    total: &mut Signal<u64>,
) -> bool {
    phase.set(event.phase.clone());
    if let Some(value) = event.current {
        current.set(value);
    }
    if let Some(value) = event.total {
        total.set(value);
    }
    status.set(
        event
            .message
            .clone()
            .unwrap_or_else(|| event_message(event)),
    );
    matches!(event.phase.as_str(), "ready" | "success" | "failed" | "cancelled")
}

fn event_message(event: &BridgeEvent) -> String {
    match event.phase.as_str() {
        "validating_manifest" => "Validating the signed sparse flash plan…".to_string(),
        "downloading" => format!("Downloading verified {} bytes…", event.total.unwrap_or_default()),
        "verifying_artifacts" => "Checking exact size and SHA-256 locally…".to_string(),
        "ready" => format!(
            "Release ready: {} local bytes verified. Device access has not started.",
            event.bytes.unwrap_or_default()
        ),
        "requesting_port" => "Choose the board's USB serial port in the browser dialog.".to_string(),
        "connecting" => "Connecting to the Espressif ROM bootloader…".to_string(),
        "verifying_target" => format!(
            "Detected {} and matched the selected chip family.",
            event.detected_chip.as_deref().unwrap_or("the expected chip")
        ),
        "writing" => format!(
            "Writing{}{} without a full erase…",
            event.part.as_deref().map(|part| format!(" {part}")).unwrap_or_default(),
            match (event.part_index, event.part_count) {
                (Some(index), Some(count)) => format!(" (part {} of {count})", index + 1),
                _ => String::new(),
            }
        ),
        "verifying_flash" => "Performing device-side MD5 verification…".to_string(),
        "resetting" => "Verification passed. Resetting into Personal Hopspot…".to_string(),
        "success" => "Verified operation complete. The device is starting Personal Hopspot.".to_string(),
        "cancelled" => "Operation cancelled; no success was reported.".to_string(),
        "failed" => format!(
            "Flashing stopped safely ({}). Follow the recovery steps and restart the complete operation.",
            event.code.as_deref().unwrap_or("unknown error")
        ),
        _ => "Working locally…".to_string(),
    }
}

fn focus_status() {
    document::eval(FOCUS_STATUS_SCRIPT);
}

const fn guided_steps(uf2: bool) -> &'static [&'static str] {
    if uf2 {
        &[
            "Confirm the exact T-Echo pictured above.",
            "Prepare the release; its Minisign signature, byte count, and SHA-256 are checked locally.",
            "Download the verified UF2, double-tap RESET, and copy it to TECHOBOOT.",
            "The bootloader drive disappears when the device reboots.",
        ]
    } else {
        &[
            "Confirm the exact board pictured above.",
            "Prepare the release; all sparse parts are downloaded and SHA-256 verified before USB access.",
            "Connect and choose the board's USB serial port.",
            "The chip family is checked before any write begins.",
            "Every part receives device-side MD5 verification before reset.",
        ]
    }
}

const fn part_kind(kind: FlashPartKind) -> &'static str {
    match kind {
        FlashPartKind::Bootloader => "bootloader",
        FlashPartKind::PartitionTable => "partition-table",
        FlashPartKind::Application => "application",
        FlashPartKind::Uf2 => "uf2",
    }
}

fn is_busy(phase: &str) -> bool {
    matches!(
        phase,
        "validating_manifest"
            | "downloading"
            | "verifying_artifacts"
            | "requesting_port"
            | "connecting"
            | "verifying_target"
            | "writing"
            | "verifying_flash"
            | "resetting"
    )
}

fn status_class(phase: &str) -> &'static str {
    match phase {
        "ready" | "success" => "flash-status-chip flash-status-chip--ready",
        "failed" | "cancelled" => "flash-status-chip flash-status-chip--blocked",
        "idle" => "flash-status-chip",
        _ => "flash-status-chip flash-status-chip--pending",
    }
}

fn phase_label(phase: &str) -> &'static str {
    match phase {
        "idle" => "Waiting",
        "ready" => "Verified",
        "success" => "Complete",
        "failed" => "Stopped",
        "cancelled" => "Cancelled",
        _ => "Working",
    }
}

fn board_card_class(board: &BoardTarget, selected: bool) -> String {
    let selected_class = if selected { " flash-board-card--selected" } else { "" };
    format!(
        "flash-board-card {}{} rounded-card border border-line/60 bg-layer/40 p-5 shadow-card",
        board.tier.flash_card_class(),
        selected_class
    )
}
