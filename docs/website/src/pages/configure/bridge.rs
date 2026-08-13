//! Wire bridge for the `/configure` headless-config page. The Rust side renders
//! the UI and issues [`ConfigureRequest`]s; the staged JS module
//! (`/assets/configure/configure.js`, built from `prns-wasm/examples/configure`)
//! owns the WebUSB session + the wasm codec and returns [`ConfigureEvent`]s.
//!
//! Each action is a one-shot `document::eval`: the request is inlined as a JSON
//! literal, the script imports the cached module, `await`s `dispatch`, and
//! returns the event. The wasm/module load is cached on `window.__prnsConfigure`
//! after the first action, so subsequent actions are cheap.
//!
//! Parity reference: `pages/flash/bridge.rs` (the flasher uses a persistent
//! `dioxus.recv/send` loop because it streams progress; configure is strictly
//! request/response per action, so a one-shot eval per action is simpler and
//! keeps the Rust side stateless across actions).
//!
//! The JS-side shapes live in `prns-wasm/examples/configure/configure.ts` and
//! the wasm snapshot projection in `prns-wasm/src/js_translation.rs`. These
//! serde types must match those exactly (camelCase, `type`-tagged sections).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// A request from the Rust UI to the JS config lane. Mirrors `ConfigureRequest`
/// in `configure.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConfigureRequest {
    Ready,
    Connect,
    Snapshot,
    #[serde(rename_all = "camelCase")]
    ApplySetLoRaProfile {
        frequency_hz: u32,
        spreading_factor: u8,
        bandwidth: u8,
        coding_rate: u8,
        tx_power_dbm: i32,
        preamble: u16,
        region_code: u8,
    },
    #[serde(rename_all = "camelCase")]
    ApplyToggleInterface {
        interface_code: u8,
    },
    ApplyResetLoRaProfile,
    ApplySleep,
    ApplyWake,
    ApplyAnnounce,
    Close,
}

/// The action result the device returns for a non-snapshot `ConfigResponse`.
/// Mirrors `UsbAutoConfigResult` in `configure.ts` / `prns-js`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigResult {
    Ok,
    ApplyFailed,
    ProfileNotSaved,
    Rejected,
    BadPayload,
}

/// An event from the JS config lane back to the Rust UI. Mirrors
/// `ConfigureEvent` in `configure.ts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConfigureEvent {
    #[serde(rename_all = "camelCase")]
    Ready {
        supported: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Connected,
    #[serde(rename_all = "camelCase")]
    ConnectFailed {
        code: String,
        detail: String,
    },
    Snapshot {
        snapshot: ConfigureSnapshot,
    },
    #[serde(rename_all = "camelCase")]
    SnapshotFailed {
        code: String,
        detail: String,
    },
    Applied {
        result: ConfigResult,
    },
    #[serde(rename_all = "camelCase")]
    ApplyFailed {
        code: String,
        detail: String,
    },
    Closed,
    #[serde(rename_all = "camelCase")]
    SessionFailed {
        code: String,
        detail: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigureSnapshot {
    pub sections: Vec<ConfigureSection>,
}

/// One snapshot section. Tagged by `type` (the wasm projection in
/// `js_translation::snapshot_section_to_js` sets `type`, not `kind`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConfigureSection {
    DeviceInfo {
        version: String,
    },
    Persistence {
        state: PersistenceState,
    },
    LoraStatus {
        status: InterfaceStatus,
    },
    UsbStatus {
        status: InterfaceStatus,
    },
    #[serde(rename_all = "camelCase")]
    BleStatus {
        status: InterfaceStatus,
        failure_reason: String,
    },
    #[serde(rename_all = "camelCase")]
    BleRecovery {
        ingress_pressure: u32,
        setup_failures: u32,
        transport_closures: u32,
        egress_pressure_events: u32,
        member_count: u8,
    },
    #[serde(rename_all = "camelCase")]
    LoraSpectrum {
        channel_busy_per_mille: u16,
        #[serde(default)]
        noise_floor_dbm: Option<i16>,
        #[serde(default)]
        cca_threshold_dbm: Option<i16>,
        deferrals: u32,
        false_preambles: u32,
        contention_timeouts: u32,
        duty_holds: u32,
        duty_timeouts: u32,
        radio_recoveries: u32,
    },
    RadioProfile {
        profile: RadioProfile,
    },
    InterfaceCounts {
        counts: Vec<InterfaceCount>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PersistenceState {
    Durable,
    Deferred,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    Initializing,
    Connected,
    Degraded,
    Reconnecting,
    Failed,
    Disconnected,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InterfaceStatus {
    pub enabled: bool,
    pub connection: ConnectionState,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    #[serde(default)]
    pub airtime: Option<Airtime>,
    #[serde(default)]
    pub transfer_rates: Option<TransferRates>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Airtime {
    pub short_per_mille: u16,
    pub long_per_mille: u16,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferRates {
    pub rx_bps: u32,
    pub tx_bps: u32,
}

/// The persisted radio profile. Mirrors `UsbAutoRadioProfile` / the wasm
/// `profile_to_js` projection (frequency, SF, BW, CR, tx power, preamble,
/// region code + the human region label).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RadioProfile {
    pub frequency_hz: u32,
    pub spreading_factor: u8,
    pub bandwidth: u8,
    pub coding_rate: u8,
    pub tx_power_dbm: i32,
    pub preamble: u16,
    pub region_code: u8,
    pub region: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InterfaceKind {
    Lora,
    Usb,
    Ble,
}

impl InterfaceKind {
    pub fn wire_code(self) -> u8 {
        // Matches `ConfigInterface::to_wire_code` on the device.
        match self {
            InterfaceKind::Lora => 0x01,
            InterfaceKind::Usb => 0x02,
            InterfaceKind::Ble => 0x03,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InterfaceCount {
    pub kind: InterfaceKind,
    pub destinations: u32,
    pub links: u32,
    pub transported_links: u32,
}

/// The script that boots the JS config lane on first use. The module caches
/// itself on `window.__prnsConfigure`; later actions skip the `import`.
const BOOTSTRAP_PREFIX: &str =
    "window.__prnsConfigure = window.__prnsConfigure || await import('/assets/configure/configure.js');";

/// Send `request` to the JS config lane and await its event. Returns `None` if
/// the eval itself failed (script error / deserialization), which the UI
/// surfaces as a generic session failure.
pub async fn dispatch(request: &ConfigureRequest) -> Option<ConfigureEvent> {
    let payload = serde_json::to_string(request).ok()?;
    // The request is a JSON object literal, safe to inline into a JS expression
    // (no `</script>` boundary — this is `document::eval`, not inline HTML).
    let script =
        format!("{BOOTSTRAP_PREFIX}\nreturn await window.__prnsConfigure.dispatch({payload});");
    document::eval(&script).join::<ConfigureEvent>().await.ok()
}

/// Probe WebUSB availability without starting a session. Cheaper than a full
/// `Connect` because it never requests a device.
pub async fn probe_ready() -> Option<ConfigureEvent> {
    dispatch(&ConfigureRequest::Ready).await
}
