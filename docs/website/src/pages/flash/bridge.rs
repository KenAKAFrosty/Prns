use dioxus::prelude::*;
use prns_flash_manifest::{TargetManifest, Transport};
use serde::{Deserialize, Serialize};

use super::contract;
use super::model::{part_kind, FlasherState};

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
pub(super) struct BridgeProvisioning {
    pub(super) action: String,
    pub(super) offset: u32,
    pub(super) size: u32,
    pub(super) ssid: String,
    pub(super) password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BridgeEvent {
    schema: u8,
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

impl BridgeRequest {
    pub(super) fn from_target(
        target: &TargetManifest,
        manifest_url: &str,
        provisioning: Option<BridgeProvisioning>,
    ) -> Result<Self, String> {
        let (base_url, _) = manifest_url
            .rsplit_once('/')
            .ok_or_else(|| "The immutable manifest URL has no release directory.".to_string())?;
        Ok(Self {
            schema: contract::schema(),
            board_slug: target.board_slug.to_string(),
            display_name: target.display_name.to_string(),
            transport: target.transport,
            expected_chip: target.expected_chip.as_ref().map(ToString::to_string),
            flash_size: target.flash_size,
            flash_mode: target.flash_mode.as_ref().map(ToString::to_string),
            flash_frequency: target.flash_frequency.as_ref().map(ToString::to_string),
            before_reset: target.before_reset.as_ref().map(ToString::to_string),
            after_reset: target.after_reset.as_ref().map(ToString::to_string),
            provisioning,
            parts: target
                .parts
                .iter()
                .map(|part| BridgePart {
                    kind: part_kind(part.kind),
                    path: part.path.to_string(),
                    url: format!("{base_url}/{}", part.path),
                    offset: part.offset,
                    size: part.size,
                    sha256: part.sha256.to_string(),
                })
                .collect(),
        })
    }
}

impl BridgeEvent {
    fn validate(self) -> Result<Self, String> {
        contract::validate_event(self.schema, &self.phase, self.code.as_deref())
            .map_err(|error| error.to_string())?;
        Ok(self)
    }
}

pub(super) async fn browser_supported() -> Option<bool> {
    document::eval(BROWSER_SUPPORT_SCRIPT)
        .join::<bool>()
        .await
        .ok()
}

pub(super) fn clear_prepared() {
    document::eval("window.__prnsFlash?.clearPrepared();");
}

pub(super) fn cancel() {
    document::eval("window.__prnsFlash?.cancel();");
}

pub(super) async fn prepare(request: BridgeRequest, mut state: FlasherState) -> Result<(), String> {
    let mut bridge = document::eval(PREPARE_SCRIPT);
    bridge
        .send(request)
        .map_err(|_| "Could not start the local flasher engine.".to_string())?;
    loop {
        let event = bridge
            .recv::<BridgeEvent>()
            .await
            .map_err(|_| "The local flasher engine stopped unexpectedly.".to_string())?
            .validate()?;
        let terminal = apply_event(&event, &mut state);
        if terminal {
            if event.phase == "ready" {
                return Ok(());
            }
            return Err(event
                .message
                .unwrap_or_else(|| "Release preparation failed safely.".to_string()));
        }
    }
}

pub(super) async fn run_flash(mut state: FlasherState) {
    state.phase.set("requesting_port".to_string());
    state
        .status
        .set("Waiting for the browser's device picker…".to_string());
    let mut bridge = document::eval(FLASH_SCRIPT);
    loop {
        let event = match bridge.recv::<BridgeEvent>().await {
            Ok(event) => match event.validate() {
                Ok(event) => event,
                Err(message) => {
                    fail_closed(&mut state, message);
                    return;
                }
            },
            Err(_) => {
                fail_closed(
                    &mut state,
                    "The local device engine stopped unexpectedly. No success was reported."
                        .to_string(),
                );
                return;
            }
        };
        if apply_event(&event, &mut state) {
            state.prepared.set(false);
            state.ssid.set(String::new());
            state.password.set(String::new());
            focus_status();
            return;
        }
    }
}

fn fail_closed(state: &mut FlasherState, message: String) {
    state.phase.set("failed".to_string());
    state.status.set(message);
    state.prepared.set(false);
    state.ssid.set(String::new());
    state.password.set(String::new());
    focus_status();
}

fn apply_event(event: &BridgeEvent, state: &mut FlasherState) -> bool {
    state.phase.set(event.phase.clone());
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
            .unwrap_or_else(|| event_message(event)),
    );
    contract::phase(&event.phase)
        .expect("validated bridge event phase")
        .terminal()
}

fn event_message(event: &BridgeEvent) -> String {
    match event.phase.as_str() {
        "idle" => "Confirm the exact board to begin.".to_string(),
        "validating_manifest" => "Validating the signed sparse flash plan…".to_string(),
        "downloading" => format!(
            "Downloading verified {} bytes…",
            event.total.unwrap_or_default()
        ),
        "verifying_artifacts" => "Checking exact size and SHA-256 locally…".to_string(),
        "ready" => format!(
            "Release ready: {} local bytes verified. Device access has not started.",
            event.bytes.unwrap_or_default()
        ),
        "requesting_port" => {
            "Choose the board's USB serial port in the browser dialog.".to_string()
        }
        "connecting" => "Connecting to the Espressif ROM bootloader…".to_string(),
        "verifying_target" => format!(
            "Detected {} and matched the selected chip family.",
            event.detected_chip.as_deref().unwrap_or("the expected chip")
        ),
        "writing" => format!(
            "Writing{}{} without a full erase…",
            event
                .part
                .as_deref()
                .map(|part| format!(" {part}"))
                .unwrap_or_default(),
            match (event.part_index, event.part_count) {
                (Some(index), Some(count)) => format!(" (part {} of {count})", index + 1),
                _ => String::new(),
            }
        ),
        "verifying_flash" => "Performing device-side MD5 verification…".to_string(),
        "resetting" => "Verification passed. Resetting into Personal Hopspot…".to_string(),
        "success" => {
            "Verified operation complete. The device is starting Personal Hopspot.".to_string()
        }
        "cancelled" => "Operation cancelled; no success was reported.".to_string(),
        "failed" => format!(
            "Flashing stopped safely ({}). Follow the recovery steps and restart the complete operation.",
            event.code.as_deref().unwrap_or("unknown error")
        ),
        _ => unreachable!("bridge event phases are contract-validated"),
    }
}

pub(super) fn focus_status() {
    document::eval(FOCUS_STATUS_SCRIPT);
}

pub(super) fn is_busy(phase: &str) -> bool {
    contract::phase(phase).is_ok_and(|definition| definition.busy())
}

pub(super) fn status_class(phase: &str) -> &'static str {
    contract::phase(phase)
        .map(contract::PhaseDefinition::status_class)
        .unwrap_or("flash-status-chip")
}

pub(super) fn phase_label(phase: &str) -> &str {
    contract::phase(phase)
        .map(contract::PhaseDefinition::label)
        .unwrap_or("Unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn rust_event_shape_and_messages_cover_the_shared_contract() {
        let mut contract_fields = contract::event_fields().collect::<Vec<_>>();
        contract_fields.sort_unstable();
        let mut rust_fields = EVENT_FIELDS.to_vec();
        rust_fields.sort_unstable();
        assert_eq!(contract_fields, rust_fields);

        for phase in contract::phase_names() {
            let event = BridgeEvent {
                schema: contract::schema(),
                phase: phase.to_string(),
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
            assert!(!event_message(&event).is_empty());
        }
    }

    #[test]
    fn rust_rejects_unknown_bridge_spellings() {
        let event = BridgeEvent {
            schema: contract::schema(),
            phase: "invented".to_string(),
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
        assert!(event.validate().is_err());
    }
}
