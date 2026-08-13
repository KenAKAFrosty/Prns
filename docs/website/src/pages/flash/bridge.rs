use dioxus::prelude::*;
use prns_flash_manifest::{EspSerialTarget, FlashPartKind, ReleaseTarget, Transport, Uf2Target};
use serde::{Deserialize, Serialize};

use crate::platforms::BoardFlashTarget;

use super::contract::{self, BridgeErrorCode, BridgePhase};
use super::model::{
    part_kind, DestructiveConfirmation, FlasherState, InstallMode, ReleaseCompatibility,
    WebSerialCapability, WEB_SERIAL_PROBE_ANDROID_BLUETOOTH_ONLY, WEB_SERIAL_PROBE_SUPPORTED,
};
use super::protocol;

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

// Chrome for Android initially exposed Web Serial only for Bluetooth RFCOMM.
// Chromium's February 2026 PSA targeted wired support for M149 on devices with the Android Serial API, with a limited rollout estimated for 2026Q2.
//
// That estimate has passed, but `navigator.serial` still does not reveal whether a particular device can enumerate wired ports.
// Keep this conservative gate until verified device coverage justifies narrowing or removing the Android classification.
fn web_serial_probe_script() -> String {
    format!(
        r#"
if (!(window.isSecureContext && navigator.serial && navigator.serial.requestPort)) {{
  return "unavailable";
}}
const wiredPortsUnreachable = navigator.userAgentData?.platform === "Android"
  || /\bAndroid\b/.test(navigator.userAgent);
return wiredPortsUnreachable ? "{WEB_SERIAL_PROBE_ANDROID_BLUETOOTH_ONLY}" : "{WEB_SERIAL_PROBE_SUPPORTED}";
"#
    )
}

const FOCUS_STATUS_SCRIPT: &str =
    "document.getElementById('flash-status')?.focus({ preventScroll: true });";

const FAIL_CLOSED_SCRIPT: &str = r#"
const bridge = window.__prnsFlash;
if (!bridge) return false;
bridge.cancel?.();
bridge.clearPrepared?.();
return true;
"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BridgeRequest {
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
    mount_label: Option<String>,
    uf2_compatibility: Option<BridgeUf2Compatibility>,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_mode: Option<InstallMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    erase_confirmed: Option<bool>,
    provisioning: Option<BridgeProvisioning>,
    parts: Vec<BridgePart>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeUf2Compatibility {
    softdevice_family: String,
    softdevice_version: String,
    fwid: u16,
    application_base: u32,
    family_id: u32,
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
pub(super) struct BridgeProvisioning {
    pub(super) action: String,
    pub(super) offset: u32,
    pub(super) size: u32,
    pub(super) ssid: String,
    pub(super) password: String,
    pub(super) tcp_client: Option<BridgeTcpClient>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BridgeTcpClient {
    pub(super) host_kind: &'static str,
    pub(super) host: String,
    pub(super) port: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BridgeEvent {
    schema: u8,
    phase: BridgePhase,
    code: Option<BridgeErrorCode>,
    message: Option<String>,
    current: Option<u64>,
    total: Option<u64>,
    part: Option<String>,
    part_index: Option<usize>,
    part_count: Option<usize>,
    detected_chip: Option<String>,
    bytes: Option<u64>,
}

impl BridgeRequest {
    pub(super) fn from_target(
        target: &ReleaseTarget,
        manifest_url: &str,
        install_mode: InstallMode,
        destructive_confirmation: DestructiveConfirmation,
        provisioning: Option<BridgeProvisioning>,
        catalog_target: BoardFlashTarget,
        compatibility: &ReleaseCompatibility,
    ) -> Result<Self, String> {
        let base_url = same_origin_release_base(manifest_url)?;
        match (target, catalog_target, compatibility) {
            (
                ReleaseTarget::EspSerial(esp),
                BoardFlashTarget::EspSerial { expected_chip, .. },
                ReleaseCompatibility::Esp,
            ) if esp.expected_chip().as_str() == expected_chip => Self::from_esp_target(
                target.board_id().as_str(),
                target.display_name(),
                esp,
                base_url,
                install_mode,
                destructive_confirmation,
                provisioning,
            ),
            (
                ReleaseTarget::Uf2(uf2),
                BoardFlashTarget::Uf2MassStorage { mount_label, .. },
                ReleaseCompatibility::Uf2(softdevice),
            ) => {
                if install_mode != InstallMode::PreserveData
                    || destructive_confirmation != DestructiveConfirmation::Unconfirmed
                {
                    return Err(
                        "A UF2 release cannot request destructive ESP installation.".to_string()
                    );
                }
                Self::from_uf2_target(
                    target.board_id().as_str(),
                    target.display_name(),
                    uf2,
                    base_url,
                    provisioning,
                    mount_label,
                    softdevice,
                )
            }
            _ => Err(
                "The signed target disagrees with the cataloged board transport or chip family."
                    .to_string(),
            ),
        }
    }

    fn from_esp_target(
        board_slug: &str,
        display_name: &str,
        target: &EspSerialTarget,
        base_url: &str,
        install_mode: InstallMode,
        destructive_confirmation: DestructiveConfirmation,
        provisioning: Option<BridgeProvisioning>,
    ) -> Result<Self, String> {
        if !destructive_confirmation.permits(install_mode) {
            return Err(
                "The ESP install mode does not have the required confirmation state.".to_string(),
            );
        }
        match (&provisioning, target.provisioning()) {
            (Some(request), Some(slot))
                if request.offset == slot.offset()
                    && request.size == slot.size()
                    && (request.tcp_client.is_none() || slot.tcp_client().is_some()) => {}
            (Some(_), Some(_)) => {
                return Err(
                    "The bridge provisioning request disagrees with the signed target.".to_string(),
                )
            }
            (Some(_), None) => {
                return Err("This target does not support Wi-Fi provisioning.".to_string())
            }
            (None, _) => {}
        }
        Ok(Self {
            schema: contract::schema(),
            board_slug: board_slug.to_string(),
            display_name: display_name.to_string(),
            transport: Transport::EspSerial,
            expected_chip: Some(target.expected_chip().as_str().to_string()),
            flash_size: Some(target.flash_size()),
            flash_mode: Some(target.flash_mode().as_str().to_string()),
            flash_frequency: Some(target.flash_frequency().as_str().to_string()),
            before_reset: Some(target.before_reset().as_str().to_string()),
            after_reset: Some(target.after_reset().as_str().to_string()),
            mount_label: None,
            uf2_compatibility: None,
            install_mode: Some(install_mode),
            erase_confirmed: Some(destructive_confirmation.is_confirmed()),
            provisioning,
            parts: target
                .parts()
                .iter()
                .map(|part| {
                    let path = part.path().as_str();
                    Ok(BridgePart {
                        kind: part_kind(part.kind()),
                        path: path.to_string(),
                        url: immutable_part_url(base_url, path)?,
                        offset: Some(part.offset()),
                        size: part.size(),
                        sha256: part.sha256().as_str().to_string(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        })
    }

    fn from_uf2_target(
        board_slug: &str,
        display_name: &str,
        target: &Uf2Target,
        base_url: &str,
        provisioning: Option<BridgeProvisioning>,
        mount_label: &str,
        softdevice: &prns_flash_manifest::SoftdeviceIdentity,
    ) -> Result<Self, String> {
        if provisioning.is_some() {
            return Err("A UF2 release cannot carry ESP provisioning data.".to_string());
        }
        let variant = target.variant_for(softdevice).ok_or_else(|| {
            format!("The signed release does not support the detected SoftDevice {softdevice}.")
        })?;
        let compatibility = variant.compatibility();
        let part = variant.part();
        let path = part.path().as_str();
        Ok(Self {
            schema: contract::schema(),
            board_slug: board_slug.to_string(),
            display_name: display_name.to_string(),
            transport: Transport::Uf2MassStorage,
            expected_chip: None,
            flash_size: None,
            flash_mode: None,
            flash_frequency: None,
            before_reset: None,
            after_reset: None,
            mount_label: Some(mount_label.to_string()),
            uf2_compatibility: Some(BridgeUf2Compatibility {
                softdevice_family: compatibility.softdevice().family().as_str().to_string(),
                softdevice_version: compatibility.softdevice().version().as_str().to_string(),
                fwid: compatibility.fwid(),
                application_base: compatibility.application_base(),
                family_id: compatibility.family_id(),
            }),
            install_mode: None,
            erase_confirmed: None,
            provisioning: None,
            parts: vec![BridgePart {
                kind: part_kind(FlashPartKind::Uf2),
                path: path.to_string(),
                url: immutable_part_url(base_url, path)?,
                offset: None,
                size: part.size(),
                sha256: part.sha256().as_str().to_string(),
            }],
        })
    }
}

fn same_origin_release_base(manifest_url: &str) -> Result<&str, String> {
    let path = manifest_url
        .strip_prefix("https://reticulum.rs")
        .ok_or_else(|| "The immutable manifest URL has an unexpected origin.".to_string())?;
    let base = path
        .strip_suffix("/flash-manifest.json")
        .ok_or_else(|| "The immutable manifest URL has no release directory.".to_string())?;
    let version = base
        .strip_prefix("/releases/")
        .ok_or_else(|| "The immutable manifest URL is not a release manifest path.".to_string())?;
    if version.is_empty()
        || version.eq_ignore_ascii_case("next")
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        || manifest_url != format!("https://reticulum.rs/releases/{version}/flash-manifest.json")
    {
        return Err("The immutable manifest URL is not a release manifest path.".to_string());
    }
    Ok(base)
}

fn immutable_part_url(base_url: &str, part_path: &str) -> Result<String, String> {
    if part_path.is_empty()
        || part_path.contains(['%', '\\', '?', '#'])
        || part_path.split('/').any(|component| {
            component.is_empty()
                || matches!(component, "." | "..")
                || !component.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+')
                })
        })
    {
        return Err("A firmware artifact path is not immutable and normalized.".to_string());
    }
    let version = base_url
        .strip_prefix("/releases/")
        .ok_or_else(|| "The immutable release base is invalid.".to_string())?;
    if version.contains('/') {
        return Err("The immutable release base is invalid.".to_string());
    }
    Ok(format!("{base_url}/{part_path}"))
}

impl BridgeEvent {
    fn validate(self, sequence: &mut protocol::EventSequence) -> Result<Self, String> {
        sequence
            .accept(protocol::EventFacts {
                schema: self.schema,
                phase: self.phase,
                code: self.code,
                message: self.message.as_deref(),
                current: self.current,
                total: self.total,
                part: self.part.as_deref(),
                part_index: self.part_index,
                part_count: self.part_count,
                detected_chip: self.detected_chip.as_deref(),
                bytes: self.bytes,
            })
            .map_err(|error| error.to_string())?;
        Ok(self)
    }
}

pub(super) async fn web_serial_capability() -> WebSerialCapability {
    document::eval(&web_serial_probe_script())
        .join::<String>()
        .await
        .map(|probe| WebSerialCapability::from_probe(&probe))
        .unwrap_or(WebSerialCapability::Unavailable)
}

pub(super) fn clear_prepared() {
    document::eval("window.__prnsFlash?.clearPrepared();");
}

pub(super) enum PreparationError {
    Stale,
    Failed(String),
}

pub(super) async fn prepare(
    request: BridgeRequest,
    mut state: FlasherState,
    generation: u64,
) -> Result<(), PreparationError> {
    if !state.preparation_is_current(generation) {
        return Err(PreparationError::Stale);
    }
    let mut bridge = document::eval(PREPARE_SCRIPT);
    let mut sequence = protocol::EventSequence::new(contract::BridgeOperation::Preparation);
    if bridge.send(request).is_err() {
        stop_local_engine().await;
        return Err(PreparationError::Failed(preparation_boundary_failure(
            "Could not start the local flasher engine.",
        )));
    }
    loop {
        let event = match bridge.recv::<BridgeEvent>().await {
            Ok(event) => match event.validate(&mut sequence) {
                Ok(event) => event,
                Err(diagnosis) => {
                    stop_local_engine().await;
                    return Err(PreparationError::Failed(preparation_boundary_failure(
                        &diagnosis,
                    )));
                }
            },
            Err(_) if !state.preparation_is_current(generation) => {
                stop_local_engine().await;
                return Err(PreparationError::Stale);
            }
            Err(_) => {
                stop_local_engine().await;
                return Err(PreparationError::Failed(
                    preparation_boundary_failure(
                        "The local flasher engine stopped unexpectedly before reporting a safe terminal state.",
                    ),
                ));
            }
        };
        if !state.preparation_is_current(generation) {
            stop_local_engine().await;
            return Err(PreparationError::Stale);
        }
        let terminal = apply_event(&event, &mut state);
        if terminal {
            if event.phase == BridgePhase::Ready {
                return Ok(());
            }
            stop_local_engine().await;
            return Err(PreparationError::Failed(event.message.unwrap_or_else(
                || "Release preparation failed safely.".to_string(),
            )));
        }
    }
}

pub(super) async fn run_flash(mut state: FlasherState) {
    state.phase.set(BridgePhase::RequestingPort);
    state.status.set(match state.flash_target {
        BoardFlashTarget::EspSerial { .. } => {
            "Waiting for the browser's serial device picker…".to_string()
        }
        BoardFlashTarget::Uf2MassStorage { .. } => {
            "Requesting the verified UF2 download from this browser…".to_string()
        }
    });
    let mut bridge = document::eval(FLASH_SCRIPT);
    let mut sequence = protocol::EventSequence::new(contract::BridgeOperation::Device);
    loop {
        let event = match bridge.recv::<BridgeEvent>().await {
            Ok(event) => match event.validate(&mut sequence) {
                Ok(event) => event,
                Err(message) => {
                    fail_closed(&mut state, message).await;
                    return;
                }
            },
            Err(_) => {
                fail_closed(
                    &mut state,
                    "The local device engine stopped unexpectedly. No success was reported."
                        .to_string(),
                )
                .await;
                return;
            }
        };
        let fresh_install = (state.install_mode)() == InstallMode::EraseAll;
        let retains_prepared_plan = !fresh_install
            && event
                .code
                .is_some_and(BridgeErrorCode::retains_prepared_plan);
        if apply_event(&event, &mut state) {
            state.prepared.set(retains_prepared_plan);
            if fresh_install {
                state
                    .destructive_confirmation
                    .set(DestructiveConfirmation::Unconfirmed);
            }
            focus_status();
            return;
        }
    }
}

async fn fail_closed(state: &mut FlasherState, message: String) {
    stop_local_engine().await;
    state.phase.set(BridgePhase::Failed);
    state.status.set(device_boundary_failure(
        &message,
        (state.install_mode)() == InstallMode::EraseAll,
    ));
    state.prepared.set(false);
    if (state.install_mode)() == InstallMode::EraseAll {
        state
            .destructive_confirmation
            .set(DestructiveConfirmation::Unconfirmed);
    }
    focus_status();
}

async fn stop_local_engine() {
    let _ = document::eval(FAIL_CLOSED_SCRIPT).join::<bool>().await;
}

fn preparation_boundary_failure(diagnosis: &str) -> String {
    format!(
        "{} Reload this page, prepare and verify the signed release again, and use the CLI if the local engine stops again. No device access has started.",
        diagnosis.trim()
    )
}

fn device_boundary_failure(diagnosis: &str, fresh_install: bool) -> String {
    let recovery = if fresh_install {
        "The device may be blank. Disconnect and reconnect it, re-enter BOOT mode, reload this page, select Fresh install, confirm the destructive action again, and restart the complete fresh-install plan."
    } else {
        "Do not assume success. Disconnect and reconnect the board, follow its BOOT/RESET recovery instructions, reload this page, and restart the complete plan; use the CLI if it repeats."
    };
    format!("{} {recovery}", diagnosis.trim())
}

fn apply_event(event: &BridgeEvent, state: &mut FlasherState) -> bool {
    state.phase.set(event.phase);
    if let Some(value) = event.current {
        state.progress_current.set(value);
    }
    if let Some(value) = event.total {
        state.progress_total.set(value);
    }
    state.status.set(
        event
            .message
            .clone()
            .unwrap_or_else(|| event_message(event, state.flash_target, (state.install_mode)())),
    );
    contract::phase(event.phase).terminal()
}

fn event_message(
    event: &BridgeEvent,
    flash_target: BoardFlashTarget,
    install_mode: InstallMode,
) -> String {
    match event.phase {
        BridgePhase::Idle => "Confirm the exact board to begin.".to_string(),
        BridgePhase::ValidatingManifest => {
            "Validating the signed sparse flash plan…".to_string()
        }
        BridgePhase::Downloading => format!(
            "Downloading verified {} bytes…",
            event.total.unwrap_or_default()
        ),
        BridgePhase::VerifyingArtifacts => {
            "Checking exact size and SHA-256 locally…".to_string()
        }
        BridgePhase::Ready => format!(
            "Release ready: {} local bytes verified. Device access has not started.",
            event.bytes.unwrap_or_default()
        ),
        BridgePhase::RequestingPort => {
            "Choose the board's USB serial port in the browser dialog.".to_string()
        }
        BridgePhase::Connecting => "Connecting to the Espressif ROM bootloader…".to_string(),
        BridgePhase::VerifyingTarget => format!(
            "Checking detected {} against the selected chip and flash-capacity plan…",
            event.detected_chip.as_deref().unwrap_or("the expected chip")
        ),
        BridgePhase::Erasing => {
            "Erasing the entire flash. Cancellation is disabled until every replacement part verifies and reset completes…".to_string()
        }
        BridgePhase::Writing => format!(
            "{}{}{}…",
            match install_mode {
                InstallMode::PreserveData => "Writing",
                InstallMode::EraseAll => "Installing",
            },
            event
                .part
                .as_deref()
                .map(|part| format!(" {part}"))
                .unwrap_or_default(),
            match (event.part_index, event.part_count) {
                (Some(index), Some(count)) => format!(" (part {} of {count})", index + 1),
                _ => String::new(),
            },
        ),
        BridgePhase::VerifyingFlash => {
            "All sparse parts passed device-side MD5 verification. Preparing the final reset…"
                .to_string()
        }
        BridgePhase::Resetting => {
            "Verification passed. Resetting into Personal Hopspot…".to_string()
        }
        BridgePhase::Success => {
            "Finished — Verified serial flash complete. The device disconnected and re-enumerated after reset; Personal Hopspot is starting. You can close this page.".to_string()
        }
        BridgePhase::DownloadRequested => match flash_target {
            BoardFlashTarget::Uf2MassStorage { mount_label, .. } => format!(
                "Verified UF2 download requested. Check the browser's downloads, then copy it to {mount_label}."
            ),
            BoardFlashTarget::EspSerial { .. } => {
                "A verified download was requested without claiming a device write.".to_string()
            }
        },
        BridgePhase::Cancelled => "Operation cancelled; no success was reported.".to_string(),
        BridgePhase::Failed => format!(
            "Flashing stopped safely ({}). Follow the recovery steps and restart the complete operation.",
            event.code.map(BridgeErrorCode::wire).unwrap_or("unknown error")
        ),
    }
}

pub(super) fn focus_status() {
    document::eval(FOCUS_STATUS_SCRIPT);
}

pub(super) fn is_busy(phase: BridgePhase) -> bool {
    contract::phase(phase).busy()
}

pub(super) fn status_class(phase: BridgePhase) -> &'static str {
    contract::phase(phase).status_class()
}

pub(super) fn phase_label(phase: BridgePhase) -> &'static str {
    contract::phase(phase).label()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::{board_target_by_slug, BoardFlashTarget};
    use std::collections::BTreeSet;

    const EVENT_FIELDS: [&str; 11] = [
        "phase",
        "code",
        "message",
        "current",
        "total",
        "part",
        "partIndex",
        "partCount",
        "detectedChip",
        "bytes",
        "schema",
    ];
    const ESP_TARGET: BoardFlashTarget = BoardFlashTarget::EspSerial {
        expected_chip: "esp32s3",
        supports_provisioning: true,
        supports_tcp_client_provisioning: true,
    };

    #[test]
    fn rust_event_shape_and_messages_cover_the_shared_contract() {
        let mut contract_fields = contract::event_fields().collect::<Vec<_>>();
        contract_fields.sort_unstable();
        let mut rust_fields = EVENT_FIELDS.to_vec();
        rust_fields.sort_unstable();
        assert_eq!(contract_fields, rust_fields);

        for phase in contract::phases() {
            let event = BridgeEvent {
                schema: contract::schema(),
                phase,
                code: None,
                message: None,
                current: None,
                total: None,
                part: None,
                part_index: None,
                part_count: None,
                detected_chip: None,
                bytes: None,
            };
            assert!(!event_message(&event, ESP_TARGET, InstallMode::PreserveData).is_empty());
        }
    }

    #[test]
    fn rust_rejects_unknown_bridge_spellings() {
        assert!(serde_json::from_str::<BridgeEvent>(r#"{"schema":1,"phase":"invented"}"#).is_err());
        assert!(serde_json::from_str::<BridgeEvent>(
            r#"{"schema":1,"phase":"failed","code":"invented"}"#
        )
        .is_err());
    }

    #[test]
    fn target_and_flash_verification_copy_matches_event_timing() {
        let target_check = event_message(
            &BridgeEvent {
                schema: contract::schema(),
                phase: BridgePhase::VerifyingTarget,
                code: None,
                message: None,
                current: None,
                total: None,
                part: None,
                part_index: None,
                part_count: None,
                detected_chip: Some("ESP32-S3".to_string()),
                bytes: None,
            },
            ESP_TARGET,
            InstallMode::PreserveData,
        );
        assert!(target_check.starts_with("Checking detected ESP32-S3"));
        assert!(!target_check.contains("matched"));
        assert!(!target_check.contains("passed"));

        let flash_check = event_message(
            &BridgeEvent {
                schema: contract::schema(),
                phase: BridgePhase::VerifyingFlash,
                code: None,
                message: None,
                current: Some(4),
                total: Some(4),
                part: None,
                part_index: None,
                part_count: None,
                detected_chip: None,
                bytes: None,
            },
            ESP_TARGET,
            InstallMode::PreserveData,
        );
        assert!(flash_check.contains("passed device-side MD5 verification"));
        assert!(!flash_check.contains("Performing"));
    }

    #[test]
    fn typed_targets_preserve_the_javascript_request_shape(
    ) -> Result<(), Box<dyn std::error::Error>> {
        const UF2_REQUEST_FIELDS: [&str; 14] = [
            "schema",
            "boardSlug",
            "displayName",
            "transport",
            "expectedChip",
            "flashSize",
            "flashMode",
            "flashFrequency",
            "beforeReset",
            "afterReset",
            "mountLabel",
            "uf2Compatibility",
            "provisioning",
            "parts",
        ];
        const ESP_REQUEST_FIELDS: [&str; 16] = [
            "schema",
            "boardSlug",
            "displayName",
            "transport",
            "expectedChip",
            "flashSize",
            "flashMode",
            "flashFrequency",
            "beforeReset",
            "afterReset",
            "mountLabel",
            "uf2Compatibility",
            "installMode",
            "eraseConfirmed",
            "provisioning",
            "parts",
        ];
        const PART_FIELDS: [&str; 6] = ["kind", "path", "url", "offset", "size", "sha256"];

        let manifest = super::super::release_0_2_6_fixture()?;
        for target in manifest.targets() {
            let manifest_url = "https://reticulum.rs/releases/0.2.6/flash-manifest.json";
            let catalog_target = board_target_by_slug(target.board_id().as_str())
                .and_then(|board| board.flash_target)
                .ok_or("missing cataloged flash target")?;
            let (compatibility, target_parts) = match target {
                ReleaseTarget::EspSerial(_) => (ReleaseCompatibility::Esp, target.parts()),
                ReleaseTarget::Uf2(target) => {
                    let variant = target.variants().last().ok_or("missing UF2 variant")?;
                    (
                        ReleaseCompatibility::Uf2(variant.compatibility().softdevice().clone()),
                        vec![prns_flash_manifest::ReleasePartRef::Uf2(variant.part())],
                    )
                }
            };
            let request = BridgeRequest::from_target(
                target,
                manifest_url,
                InstallMode::PreserveData,
                DestructiveConfirmation::Unconfirmed,
                None,
                catalog_target,
                &compatibility,
            )?;
            let wire = serde_json::to_value(request)?;
            let object = wire.as_object().ok_or("bridge request is not an object")?;
            assert_eq!(wire["schema"], contract::schema());
            assert_eq!(wire["boardSlug"], target.board_id().as_str());
            assert_eq!(wire["displayName"], target.display_name());
            assert!(wire["provisioning"].is_null());
            let wire_parts = wire["parts"].as_array().ok_or("parts are not an array")?;
            assert_eq!(wire_parts.len(), target_parts.len());
            for (part, target_part) in wire_parts.iter().zip(target_parts) {
                assert_eq!(
                    part.as_object()
                        .ok_or("bridge part is not an object")?
                        .keys()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>(),
                    PART_FIELDS.into_iter().collect()
                );
                assert_eq!(part["kind"], part_kind(target_part.kind()));
                assert_eq!(part["path"], target_part.path().as_str());
                assert_eq!(
                    part["url"],
                    format!("/releases/0.2.6/{}", target_part.path().as_str())
                );
                assert_eq!(part["offset"], serde_json::to_value(target_part.offset())?);
                assert_eq!(part["size"], target_part.size());
                assert_eq!(part["sha256"], target_part.sha256().as_str());
            }

            match target {
                ReleaseTarget::EspSerial(esp) => {
                    assert_eq!(
                        object.keys().map(String::as_str).collect::<BTreeSet<_>>(),
                        ESP_REQUEST_FIELDS.into_iter().collect()
                    );
                    assert_eq!(wire["transport"], "esp-serial");
                    assert_eq!(wire["installMode"], "preserve-data");
                    assert_eq!(wire["eraseConfirmed"], false);
                    assert_eq!(wire["expectedChip"], esp.expected_chip().as_str());
                    assert_eq!(wire["flashSize"], esp.flash_size());
                    assert_eq!(wire["flashMode"], esp.flash_mode().as_str());
                    assert_eq!(wire["flashFrequency"], esp.flash_frequency().as_str());
                    assert_eq!(wire["beforeReset"], esp.before_reset().as_str());
                    assert_eq!(wire["afterReset"], esp.after_reset().as_str());
                    assert!(wire["mountLabel"].is_null());
                    assert!(wire["uf2Compatibility"].is_null());
                    assert!(wire["parts"]
                        .as_array()
                        .expect("parts array")
                        .iter()
                        .all(|part| part["offset"].is_number()));
                }
                ReleaseTarget::Uf2(_) => {
                    assert_eq!(
                        object.keys().map(String::as_str).collect::<BTreeSet<_>>(),
                        UF2_REQUEST_FIELDS.into_iter().collect()
                    );
                    assert_eq!(wire["transport"], "uf2-mass-storage");
                    assert!(wire.get("installMode").is_none());
                    assert!(wire.get("eraseConfirmed").is_none());
                    assert_eq!(wire["mountLabel"], "TECHOBOOT");
                    assert_eq!(wire["uf2Compatibility"]["softdeviceFamily"], "s140");
                    assert_eq!(wire["uf2Compatibility"]["softdeviceVersion"], "7.3.0");
                    assert_eq!(wire["uf2Compatibility"]["fwid"], 0x0123);
                    assert_eq!(wire["uf2Compatibility"]["applicationBase"], 0x27000);
                    assert_eq!(wire["uf2Compatibility"]["familyId"], 0xada52840_u32);
                    for field in [
                        "expectedChip",
                        "flashSize",
                        "flashMode",
                        "flashFrequency",
                        "beforeReset",
                        "afterReset",
                    ] {
                        assert!(wire[field].is_null(), "UF2 field {field} must stay null");
                    }
                    let [part] = wire["parts"]
                        .as_array()
                        .ok_or("UF2 parts are not an array")?
                        .as_slice()
                    else {
                        return Err("UF2 request does not contain exactly one part".into());
                    };
                    assert_eq!(part["kind"], "uf2");
                    assert!(part["offset"].is_null());
                }
            }
        }

        let target = manifest
            .targets()
            .iter()
            .find(|target| target.board_id().as_str() == "heltec-v4")
            .ok_or("missing provisionable target")?;
        let slot = target.provisioning().ok_or("missing provisioning slot")?;
        let request = BridgeRequest::from_target(
            target,
            "https://reticulum.rs/releases/0.2.6/flash-manifest.json",
            InstallMode::EraseAll,
            DestructiveConfirmation::Confirmed,
            Some(BridgeProvisioning {
                action: "configure".to_string(),
                offset: slot.offset(),
                size: slot.size(),
                ssid: "network".to_string(),
                password: "password".to_string(),
                tcp_client: None,
            }),
            ESP_TARGET,
            &ReleaseCompatibility::Esp,
        )?;
        let wire = serde_json::to_value(request)?;
        assert_eq!(wire["installMode"], "erase-all");
        assert_eq!(wire["eraseConfirmed"], true);
        assert_eq!(
            wire["provisioning"],
            serde_json::json!({
                "action": "configure",
                "offset": slot.offset(),
                "size": slot.size(),
                "ssid": "network",
                "password": "password",
                "tcpClient": null,
            })
        );

        let target = manifest
            .targets()
            .iter()
            .find(|target| target.board_id().as_str() == "xiao-esp32-c6")
            .ok_or("missing non-provisionable target")?;
        assert!(BridgeRequest::from_target(
            target,
            "https://reticulum.rs/releases/0.2.6/flash-manifest.json",
            InstallMode::PreserveData,
            DestructiveConfirmation::Unconfirmed,
            Some(BridgeProvisioning {
                action: "clear".to_string(),
                offset: 0xd000,
                size: 0x1000,
                ssid: String::new(),
                password: String::new(),
                tcp_client: None,
            }),
            BoardFlashTarget::EspSerial {
                expected_chip: "esp32c6",
                supports_provisioning: false,
                supports_tcp_client_provisioning: false,
            },
            &ReleaseCompatibility::Esp,
        )
        .is_err());

        let target = manifest
            .targets()
            .iter()
            .find(|target| target.board_id().as_str() == "t-echo")
            .ok_or("missing UF2 target")?;
        let ReleaseTarget::Uf2(uf2_target) = target else {
            return Err("T-Echo target is not UF2".into());
        };
        let compatibility = ReleaseCompatibility::Uf2(
            uf2_target
                .variants()
                .last()
                .ok_or("missing UF2 variant")?
                .compatibility()
                .softdevice()
                .clone(),
        );
        assert!(BridgeRequest::from_target(
            target,
            "https://reticulum.rs/releases/0.2.6/flash-manifest.json",
            InstallMode::EraseAll,
            DestructiveConfirmation::Confirmed,
            None,
            BoardFlashTarget::Uf2MassStorage {
                mount_label: "TECHOBOOT",
                board_id_prefix: "nrf52840-techo-v",
            },
            &compatibility,
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn rust_boundary_failures_keep_diagnosis_and_safe_recovery() {
        let preparation = preparation_boundary_failure("bridge schema was rejected");
        assert!(preparation.contains("bridge schema was rejected"));
        assert!(preparation.contains("Reload this page"));
        assert!(preparation.contains("No device access has started"));

        let device = device_boundary_failure("local device engine stopped", false);
        assert!(device.contains("local device engine stopped"));
        assert!(device.contains("Do not assume success"));
        assert!(device.contains("BOOT/RESET"));
        assert!(device.contains("restart the complete plan"));

        let fresh = device_boundary_failure("local device engine stopped", true);
        assert!(fresh.contains("device may be blank"));
        assert!(fresh.contains("confirm the destructive action again"));
        assert!(fresh.contains("complete fresh-install plan"));

        let cancel = FAIL_CLOSED_SCRIPT
            .find("bridge.cancel?.()")
            .expect("fail-closed script must request cancellation");
        let clear = FAIL_CLOSED_SCRIPT
            .find("bridge.clearPrepared?.()")
            .expect("fail-closed script must clear the verified plan");
        assert!(
            cancel < clear,
            "active work must be cancelled before cleanup"
        );
    }

    #[test]
    fn web_serial_probe_script_speaks_the_model_wire_spellings() {
        let script = web_serial_probe_script();

        assert!(script.contains("window.isSecureContext"));
        assert!(script.contains("navigator.serial.requestPort"));
        assert!(script.contains(&format!("\"{WEB_SERIAL_PROBE_SUPPORTED}\"")));
        assert!(script.contains(&format!("\"{WEB_SERIAL_PROBE_ANDROID_BLUETOOTH_ONLY}\"")));
    }

    #[test]
    fn artifact_urls_stay_on_the_served_candidate_origin() {
        assert_eq!(
            same_origin_release_base("https://reticulum.rs/releases/0.2.6/flash-manifest.json"),
            Ok("/releases/0.2.6")
        );
        assert!(same_origin_release_base(
            "https://example.test/releases/0.2.6/flash-manifest.json"
        )
        .is_err());
        for malformed in [
            "https://reticulum.rs/releases/0.2.6/../0.2.7/flash-manifest.json",
            "https://reticulum.rs/releases/%2e%2e/flash-manifest.json",
            "https://reticulum.rs/releases/0.2.6//flash-manifest.json",
        ] {
            assert!(same_origin_release_base(malformed).is_err(), "{malformed}");
        }
        assert_eq!(
            immutable_part_url(
                "/releases/0.2.6",
                "firmware/hopspot/heltec-v4/0.2.6/application.bin"
            ),
            Ok("/releases/0.2.6/firmware/hopspot/heltec-v4/0.2.6/application.bin".to_string())
        );
        for malformed in [
            "firmware/%2e%2e/application.bin",
            "firmware/%252e%252e/application.bin",
            "firmware/../application.bin",
            "firmware//application.bin",
        ] {
            assert!(immutable_part_url("/releases/0.2.6", malformed).is_err());
        }
    }
}
