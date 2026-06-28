use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::PlatformChip;
use crate::flash_manifest::{
    embedded_docs_mode, flash_artifact_for_board, EmbeddedPolicy, FlashArtifactRecord,
    FlashTransport,
};
use crate::platforms::{board_target_by_slug, BoardTarget, BOARD_TARGETS};
use crate::routes::Route;

const ESP_WEB_TOOLS_SCRIPT: &str =
    "https://unpkg.com/esp-web-tools@10/dist/web/install-button.js?module";
const HOPSPOT_CONFIG_OFFSET: u32 = 0xD000;

/// Hopspot flashing entrypoint. This is intentionally separate from
/// `/platforms`: platform support is broad, flashing is board-specific.
#[component]
pub fn FlashPage() -> Element {
    rsx! {
        FlashExperience { selected_slug: None }
    }
}

#[component]
pub fn FlashBoardPage(board: String) -> Element {
    rsx! {
        FlashExperience { selected_slug: Some(board) }
    }
}

#[component]
fn FlashExperience(selected_slug: Option<String>) -> Element {
    let selected_target = selected_slug.as_deref().and_then(board_target_by_slug);
    let missing_selection = selected_slug.is_some() && selected_target.is_none();

    rsx! {
        header { class: "mb-10",
            Link {
                to: Route::PlatformsPage {},
                class: "text-sm text-soft hover:text-accent transition-colors",
                "← "
                {t!("flash-back")}
            }
            p { class: "mt-6 text-xs font-semibold tracking-[0.22em] uppercase text-accent",
                {t!("flash-kicker")}
            }
            h1 { class: "mt-3 text-3xl md:text-4xl font-semibold tracking-tight text-paper",
                {t!("flash-title")}
            }
        }

        if let Some(target) = selected_target {
            if target.is_flashable() {
                ReadyTargetPanel {
                    target,
                    artifact: flash_artifact_for_board(target.slug),
                }
            } else {
                section { class: "mt-8 rounded-card border border-line/60 bg-layer/40 p-5",
                    h2 { class: "text-xl font-semibold text-paper",
                        {t!("flash-unavailable-title")}
                    }
                    p { class: "mt-3 text-soft leading-relaxed",
                        {t!("flash-unavailable-body")}
                    }
                }
            }
        } else if missing_selection {
            section { class: "mt-8 rounded-card border border-line/60 bg-layer/40 p-5",
                h2 { class: "text-xl font-semibold text-paper",
                    {t!("flash-missing-title")}
                }
                p { class: "mt-3 text-soft leading-relaxed",
                    {t!("flash-missing-body")}
                }
            }
        }

        section { class: if selected_target.is_some() { "mt-10" } else { "mt-4" },
            if selected_target.is_some() {
                h2 { class: "text-2xl font-semibold tracking-tight text-paper",
                    {t!("flash-picker-change-title")}
                }
                p { class: "mt-3 text-soft max-w-3xl leading-relaxed",
                    {t!("flash-board-lead")}
                }
            }
            if selected_target.is_none() {
                p { class: "text-soft max-w-3xl leading-relaxed",
                    {t!("flash-board-lead")}
                }
            }
            div { class: "mt-6 grid gap-4 md:grid-cols-2",
                for board in BOARD_TARGETS.iter() {
                    BoardTargetCard {
                        key: "{board.slug}",
                        board,
                        selected: selected_target
                            .map(|target| target.slug == board.slug)
                            .unwrap_or(false),
                    }
                }
            }
        }
    }
}

#[component]
fn BoardTargetCard(board: &'static BoardTarget, selected: bool) -> Element {
    rsx! {
        div {
            class: board_card_class(board, selected),
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
                        img {
                            class: "flash-board-img",
                            src: image.data_uri,
                            alt: "",
                            loading: "lazy",
                        }
                    }
                }
                p { class: "flash-board-silicon font-mono text-xs leading-snug",
                    "{board.silicon}"
                }
            }
            if board.interfaces.is_empty() {
                p { class: "flash-interfaces-pending mt-4",
                    {t!("flash-interfaces-pending")}
                }
            } else {
                div { class: "mt-4",
                    p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-mid",
                        {t!("flash-interfaces-label")}
                    }
                    div { class: "mt-2 flex flex-wrap gap-2",
                        for interface in board.interfaces.iter() {
                            span {
                                key: "{interface}",
                                class: "flash-interface-chip",
                                "{interface}"
                            }
                        }
                    }
                }
            }
            if board.is_flashable() {
                div { class: "mt-5 flex justify-end",
                    if selected {
                        span {
                            class: "inline-flex items-center py-2.5 text-xs font-bold uppercase tracking-wider text-accent leading-none",
                            "aria-current": "true",
                            {t!("flash-card-selected")}
                        }
                    } else {
                        Link {
                            to: Route::FlashBoardPage { board: board.slug.to_string() },
                            class: "flash-card-action",
                            "aria-label": "Flash {board.name}",
                            span { {t!("flash-card-action")} }
                            span {
                                class: "flash-card-action__arrow",
                                "aria-hidden": "true",
                                "→"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ReadyTargetPanel(
    target: &'static BoardTarget,
    artifact: Option<&'static FlashArtifactRecord>,
) -> Element {
    let embedded_site = embedded_docs_mode();
    let action_enabled = artifact
        .map(|artifact| artifact.web_action_enabled(embedded_site))
        .unwrap_or(false);
    let action_label = artifact
        .map(|artifact| artifact.action_label(embedded_site).to_string())
        .unwrap_or_else(|| "Manifest missing".to_string());
    let download_path = artifact.and_then(|artifact| artifact.download_path(embedded_site));
    let esp_web_manifest_path =
        artifact.and_then(|artifact| artifact.esp_web_manifest_path(embedded_site));
    let esp_firmware_path = artifact.and_then(|artifact| artifact.artifact_path);
    let embedded_hosted_only = artifact
        .map(|artifact| {
            embedded_site && matches!(artifact.embedded_policy, EmbeddedPolicy::HostedOnly)
        })
        .unwrap_or(false);
    let download_file = artifact
        .and_then(artifact_file_name)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}.uf2", target.slug));
    let ready_title = artifact.map(flash_ready_title).unwrap_or("Web flashing");
    let configurable_wifi = !embedded_site
        && target
            .interfaces
            .iter()
            .any(|interface| *interface == "Wi-Fi Auto");

    rsx! {
        section { class: "flash-flasher-panel mt-8",
            div { class: "flash-flasher-panel__main",
                p { class: "text-xs font-semibold tracking-[0.2em] uppercase text-accent",
                    {t!("flash-ready-kicker")}
                }
                h2 { class: "mt-2 text-2xl font-semibold tracking-tight text-paper",
                    "{ready_title}"
                }
                div { class: "flash-ready-target mt-5",
                    div { class: "flash-ready-target__summary",
                        div { class: "flash-ready-target__copy",
                            PlatformChip {
                                name: target.name.to_string(),
                                icon: target.icon.map(str::to_string),
                                badge: None,
                                muted: false,
                                decorative: false,
                            }
                            p { class: "flash-board-silicon font-mono text-xs leading-snug",
                                "{target.silicon}"
                            }
                            if let Some(image) = target.image() {
                                span { class: "flash-board-slot flash-board-slot--inset flash-board-slot--hero",
                                    img {
                                        class: "flash-board-img",
                                        src: image.data_uri,
                                        alt: "",
                                        loading: "lazy",
                                    }
                                }
                            }
                        }
                    }
                    if !target.interfaces.is_empty() {
                        div { class: "mt-4",
                            p { class: "text-[0.7rem] font-bold tracking-[0.18em] uppercase text-mid",
                                {t!("flash-interfaces-label")}
                            }
                            div { class: "mt-2 flex flex-wrap gap-2",
                                for interface in target.interfaces.iter() {
                                    span {
                                        key: "{interface}",
                                        class: "flash-interface-chip",
                                        "{interface}"
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(artifact) = artifact {
                    FlashPlanPanel {
                        artifact,
                        embedded_site,
                    }
                } else {
                    div { class: "flash-artifact-panel mt-5",
                        p { class: "flash-status-chip flash-status-chip--blocked",
                            "Manifest missing"
                        }
                        p { class: "mt-3 text-sm leading-relaxed text-soft",
                            "This board is flashable in the catalog, but no artifact manifest entry exists yet."
                        }
                    }
                }
            }
            div { class: "flash-flasher-panel__action",
                if embedded_hosted_only {
                    if let Some(artifact) = artifact {
                        p { class: "flash-embedded-note",
                            "Build this repo locally and flash the board with "
                            code { class: "flash-local-command",
                                "{artifact.local_command}"
                            }
                            "."
                        }
                    }
                } else if let Some(manifest_path) = esp_web_manifest_path {
                    EspWebInstallAction {
                        manifest_path,
                        firmware_path: esp_firmware_path.map(str::to_string),
                        board_slug: target.slug.to_string(),
                        install_name: target.name.to_string(),
                        chip_family: esp_chip_family(target).map(str::to_string),
                        action_label: action_label.clone(),
                        configurable_wifi,
                    }
                } else if let Some(download_path) = download_path {
                    a {
                        href: "{download_path}",
                        class: "flash-primary-action",
                        download: "{download_file}",
                        "{action_label}"
                    }
                } else {
                    button {
                        r#type: "button",
                        disabled: !action_enabled,
                        class: "flash-primary-action",
                        "{action_label}"
                    }
                }
                if !embedded_hosted_only {
                    p { class: "mt-3 text-xs leading-relaxed text-mid",
                        if let Some(artifact) = artifact {
                            "{artifact.status_note(embedded_site)}"
                        } else {
                            "Add this board to the flash artifact manifest before enabling web actions."
                        }
                    }
                }
            }
        }

        section { class: "flash-local-panel mt-5 rounded-card border border-line/60 bg-surface/45 p-4",
            h2 { class: "text-sm font-semibold text-paper",
                {t!("flash-local-title")}
            }
            p { class: "mt-2 text-sm leading-relaxed text-soft",
                "Fully offline? Build this repo locally and flash the board"
                if let Some(artifact) = artifact {
                    " "
                    code { class: "flash-local-command",
                        "{artifact.local_command}"
                    }
                }
            }
        }
    }
}

#[component]
fn EspWebInstallAction(
    manifest_path: String,
    firmware_path: Option<String>,
    board_slug: String,
    install_name: String,
    chip_family: Option<String>,
    action_label: String,
    configurable_wifi: bool,
) -> Element {
    let manifest_path = html_escape(&manifest_path);
    let action_label = html_escape(&action_label);
    let install_id = format!("hopspot-install-{}", board_slug);
    let firmware_path = firmware_path.unwrap_or_default();
    let chip_family = chip_family.unwrap_or_default();
    let install_script =
        if configurable_wifi && !firmware_path.is_empty() && !chip_family.is_empty() {
            wifi_config_installer_script(&install_id)
        } else {
            String::new()
        };
    let installer_html = format!(
        r#"<esp-web-install-button manifest="{manifest_path}">
  <button slot="activate" type="button" class="flash-primary-action">{action_label}</button>
  <span slot="unsupported" class="flash-web-install-message">Chrome or Edge with Web Serial is required.</span>
  <span slot="not-allowed" class="flash-web-install-message">Open this page over HTTPS or localhost to use Web Serial.</span>
</esp-web-install-button>"#
    );

    rsx! {
        div {
            id: "{install_id}",
            class: "flash-web-install",
            "data-firmware-path": "{firmware_path}",
            "data-chip-family": "{chip_family}",
            "data-install-name": "{install_name}",
            "data-config-offset": "{HOPSPOT_CONFIG_OFFSET}",
            if configurable_wifi && !firmware_path.is_empty() && !chip_family.is_empty() {
                div { class: "flash-wifi-config",
                    p { class: "flash-wifi-note",
                        "Optional: write credentials if you want Wi-Fi Auto to join a local network after flashing."
                    }
                    div { class: "flash-wifi-grid",
                        label { class: "flash-wifi-field",
                            span { "Wi-Fi Auto SSID" }
                            input {
                                r#type: "text",
                                maxlength: "32",
                                autocomplete: "network-name",
                                placeholder: "Optional",
                                "data-hopspot-wifi-ssid": "true",
                            }
                        }
                        label { class: "flash-wifi-field",
                            span { "Wi-Fi password" }
                            input {
                                r#type: "password",
                                maxlength: "64",
                                autocomplete: "current-password",
                                placeholder: "Optional",
                                "data-hopspot-wifi-password": "true",
                            }
                        }
                    }
                }
            }
            script {
                r#type: "module",
                src: ESP_WEB_TOOLS_SCRIPT,
            }
            div {
                dangerous_inner_html: "{installer_html}",
            }
            if !install_script.is_empty() {
                script {
                    dangerous_inner_html: "{install_script}",
                }
            }
        }
    }
}

#[component]
fn FlashPlanPanel(artifact: &'static FlashArtifactRecord, embedded_site: bool) -> Element {
    rsx! {
        div { class: "flash-plan-panel mt-5",
            div { class: "flash-plan-panel__head",
                h3 { class: "text-sm font-semibold text-paper",
                    "Flash plan"
                }
                span { class: flash_status_class(artifact, embedded_site),
                    "{flash_status_label(artifact, embedded_site)}"
                }
            }
            ol { class: "flash-step-list mt-3",
                for (index, step) in artifact.steps.iter().enumerate() {
                    li {
                        key: "{index}",
                        span { class: "flash-step-list__index", "{index + 1}" }
                        span { "{step}" }
                    }
                }
            }
            FlashArtifactDetails {
                artifact,
            }
            if matches!(artifact.transport, FlashTransport::Uf2MassStorage) {
                p { class: "flash-uf2-note mt-4",
                    "T-Echo uses the UF2 bootloader flow: the browser downloads firmware, and the board flashes itself when you copy that file to TECHOBOOT."
                }
            }
        }
    }
}

#[component]
fn FlashArtifactDetails(artifact: &'static FlashArtifactRecord) -> Element {
    let file_name = artifact_file_name(artifact)
        .map(str::to_string)
        .unwrap_or_else(|| "Firmware artifact".to_string());
    let size = artifact
        .artifact_size
        .map(format_artifact_size)
        .unwrap_or_else(|| "Pending".to_string());
    let checksum = artifact
        .artifact_sha256
        .map(short_checksum)
        .unwrap_or_else(|| "Pending".to_string());

    rsx! {
        div { class: "flash-artifact-details mt-4",
            FlashArtifactFact {
                label: "File",
                value: file_name,
            }
            FlashArtifactFact {
                label: "Size",
                value: size,
            }
            FlashArtifactFact {
                label: "Flash method",
                value: artifact.transport.label().to_string(),
            }
            FlashArtifactFact {
                label: "SHA-256",
                value: checksum,
                mono: true,
            }
        }
    }
}

#[component]
fn FlashArtifactFact(
    label: &'static str,
    value: String,
    #[props(default = false)] mono: bool,
) -> Element {
    let value_class = if mono {
        "flash-artifact-fact__value flash-artifact-fact__value--mono"
    } else {
        "flash-artifact-fact__value"
    };

    rsx! {
        div { class: "flash-artifact-fact",
            span { class: "flash-artifact-fact__label",
                "{label}"
            }
            span { class: value_class,
                "{value}"
            }
        }
    }
}

fn flash_status_class(artifact: &FlashArtifactRecord, embedded_site: bool) -> &'static str {
    if embedded_site && matches!(artifact.embedded_policy, EmbeddedPolicy::HostedOnly) {
        "flash-status-chip flash-status-chip--blocked"
    } else if artifact.web_action_enabled(embedded_site) {
        "flash-status-chip flash-status-chip--ready"
    } else {
        "flash-status-chip flash-status-chip--pending"
    }
}

fn flash_status_label(artifact: &FlashArtifactRecord, embedded_site: bool) -> &'static str {
    if embedded_site && matches!(artifact.embedded_policy, EmbeddedPolicy::HostedOnly) {
        "Local build"
    } else {
        artifact.state.label()
    }
}

fn flash_ready_title(artifact: &FlashArtifactRecord) -> &'static str {
    match artifact.transport {
        FlashTransport::EspWebSerial => "Web flashing",
        FlashTransport::Uf2MassStorage => "Field-recover firmware",
    }
}

fn artifact_file_name(artifact: &FlashArtifactRecord) -> Option<&'static str> {
    artifact
        .artifact_path
        .and_then(|path| path.rsplit('/').next())
        .filter(|name| !name.is_empty())
}

fn short_checksum(hash: &str) -> String {
    if hash.len() > 16 {
        format!("{}…{}", &hash[..12], &hash[hash.len() - 8..])
    } else {
        hash.to_string()
    }
}

fn format_artifact_size(size: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / MIB)
    } else if size >= 1024 {
        format!("{:.0} KiB", size as f64 / KIB)
    } else {
        format!("{size} bytes")
    }
}

fn esp_chip_family(target: &BoardTarget) -> Option<&'static str> {
    if target.silicon.contains("ESP32-S3") {
        Some("ESP32-S3")
    } else if target.silicon.contains("ESP32-C6") {
        Some("ESP32-C6")
    } else {
        None
    }
}

fn wifi_config_installer_script(install_id: &str) -> String {
    let install_id = js_string_escape(install_id);
    format!(
        r#"(function() {{
  const root = document.getElementById("{install_id}");
  if (!root || root.dataset.hopspotConfigReady === "1") return;
  root.dataset.hopspotConfigReady = "1";
  const install = root.querySelector("esp-web-install-button");
  const ssidInput = root.querySelector("[data-hopspot-wifi-ssid]");
  const passwordInput = root.querySelector("[data-hopspot-wifi-password]");
  if (!install || !ssidInput || !passwordInput) return;
  const encoder = new TextEncoder();
  let configUrl = "";
  let manifestUrl = "";
  const revoke = () => {{
    if (configUrl) URL.revokeObjectURL(configUrl);
    if (manifestUrl) URL.revokeObjectURL(manifestUrl);
  }};
  const writeConfig = () => {{
    const bytes = new Uint8Array(4096);
    bytes.fill(0xff);
    bytes.set([72, 83, 80, 67, 70, 71, 49, 0], 0);
    bytes[8] = 1;
    const ssid = encoder.encode(ssidInput.value.trim()).slice(0, 32);
    const password = encoder.encode(passwordInput.value).slice(0, 64);
    bytes[10] = ssid.length;
    bytes[11] = password.length;
    bytes.set(ssid, 16);
    bytes.set(password, 48);
    return bytes;
  }};
  const refreshManifest = () => {{
    revoke();
    configUrl = URL.createObjectURL(new Blob([writeConfig()], {{ type: "application/octet-stream" }}));
    const manifest = {{
      name: root.dataset.installName || "Hopspot",
      version: "preview",
      new_install_prompt_erase: true,
      new_install_improv_wait_time: 0,
      builds: [{{
        chipFamily: root.dataset.chipFamily,
        improv: false,
        parts: [
          {{ path: new URL(root.dataset.firmwarePath, window.location.href).href, offset: 0 }},
          {{ path: configUrl, offset: Number(root.dataset.configOffset || "53248") }}
        ]
      }}]
    }};
    manifestUrl = URL.createObjectURL(new Blob([JSON.stringify(manifest)], {{ type: "application/json" }}));
    install.setAttribute("manifest", manifestUrl);
  }};
  ssidInput.addEventListener("input", refreshManifest);
  passwordInput.addEventListener("input", refreshManifest);
  window.addEventListener("pagehide", revoke, {{ once: true }});
  refreshManifest();
}})();"#
    )
}

fn js_string_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn board_card_class(board: &BoardTarget, is_selected: bool) -> String {
    let selected_class = if is_selected {
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
