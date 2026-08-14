//! Headless config task for the T1000-E (no screen, no button). Consumes
//! `ConfigRequest`s forwarded by the USB Auto device lane, applies them through
//! the same primitives the T-Echo render loop uses, and replies with a
//! `ConfigReply` that the lane turns into a wire `ConfigResponse` or `Snapshot`.
//!
//! Parity reference: the ephemeral action logic (toggle/sleep/wake/announce)
//! mirrors `firmware.rs`'s render-loop `UiAction` dispatch (T-Echo). The
//! persisted LoRa path shares `hopspot::apply_and_persist_radio_profile`, so the
//! saved/apply-failed/not-saved outcomes match exactly. See
//! `T1000E_HEADLESS_CONFIG.md`.

use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use heapless::String as HeaplessString;
use heapless::Vec as HeaplessVec;

use nrf_softdevice::Flash;
use personal_hopspot_core as hopspot;
use personal_rns::bluetooth_auto::BluetoothAutoStatus;
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::interfaces::lora::{RadioProfile, DEFAULT_915_PROFILE};
use personal_rns::interfaces::usb_auto::{
    BluetoothRecoveryBody, ConfigAction, ConfigInterface, ConfigReply, ConfigRequest, ConfigResult,
    InterfaceCount, InterfaceStatusBody, LoRaSpectrumBody, SnapshotBody, SnapshotPersistence,
    SnapshotSection, MAX_FAILURE_REASON_BYTES, MAX_SNAPSHOT_BODY_BYTES, MAX_SNAPSHOT_SECTIONS,
    SNAPSHOT_SCHEMA_VERSION,
};
use personal_rns::interfaces::{InterfaceId, InterfaceStatus};
use personal_rns::lora::{LoRaApplyOutcome, LoRaSpectrumStatus};
use personal_rns::manifold::embassy::EmbassyInterfaceStatus;
use personal_rns::runtime::SharedNorFlash;
use personal_rns::usb_auto::CONFIG_CHANNEL_CAPACITY;
use personal_rns::wire::DestinationHash;

use super::bluetooth_auto::{BLE_SHARED, BLE_SUPERVISOR_ID};
use super::node::{
    UiHandle, BLE_MANIFOLD_LANE, COMMANDS, COMPLETION, INTERFACE_STORE, LORA_CONTROL,
};
use crate::boards::selected as board;

/// Persistent profile store shared between the render loop (T-Echo) and this
/// config task (T1000-E). `NoopRawMutex` is safe because a single embassy
/// task accesses the store at a time per board; it avoids masking interrupts
/// across the multi-millisecond flash save.
pub(crate) type ProfileStore = Mutex<
    NoopRawMutex,
    hopspot::RadioProfileStore<SharedNorFlash<'static, CriticalSectionRawMutex, Flash>>,
>;

static REQUESTS: Channel<CriticalSectionRawMutex, ConfigRequest, CONFIG_CHANNEL_CAPACITY> =
    Channel::new();
static REPLIES: Channel<CriticalSectionRawMutex, ConfigReply, CONFIG_CHANNEL_CAPACITY> =
    Channel::new();

/// The lane-facing endpoints: the device lane sends requests and receives
/// replies. Pass this to `UsbAutoDevice::with_config`. Only the T1000-E wires a
/// config lane, so this is compiled only with `board-t1000e`.
#[cfg(feature = "board-t1000e")]
#[must_use]
pub fn lane_endpoints() -> personal_rns::usb_auto::ConfigEndpoints<'static> {
    use personal_rns::usb_auto::ConfigEndpoints;
    ConfigEndpoints {
        requests: REQUESTS.sender(),
        replies: REPLIES.receiver(),
    }
}

/// Run the config task. Pulls requests from [`REQUESTS`], applies them, and
/// pushes replies to [`REPLIES`]. Never returns.
pub async fn run(
    store: &'static ProfileStore,
    lora_status: &'static EmbassyInterfaceStatus,
    usb_status: &'static EmbassyInterfaceStatus,
    lora_spectrum: &'static LoRaSpectrumStatus,
    node_page_destination: DestinationHash,
) {
    let ui_handle = UiHandle::new(COMMANDS.sender(), &COMPLETION);
    let ctx = ConfigCtx {
        store,
        lora_status,
        usb_status,
        lora_spectrum,
        ui_handle: &ui_handle,
        node_page_destination,
    };
    loop {
        let request = REQUESTS.receive().await;
        let request_id = request.request_id;
        let reply = match ConfigAction::decode(&request.action) {
            None => ConfigReply::response(request_id, ConfigResult::BadPayload),
            Some(action) => apply_action(action, request_id, &ctx).await,
        };
        REPLIES.send(reply).await;
    }
}

/// Shared runtime handles a request dispatch needs. Bundling them keeps
/// [`apply_action`] and [`build_snapshot`] signatures readable and avoids
/// threading half a dozen `&'static` refs through every call.
struct ConfigCtx<'a> {
    store: &'a ProfileStore,
    lora_status: &'a EmbassyInterfaceStatus,
    usb_status: &'a EmbassyInterfaceStatus,
    lora_spectrum: &'a LoRaSpectrumStatus,
    ui_handle: &'a UiHandle,
    node_page_destination: DestinationHash,
}

async fn apply_action(action: ConfigAction, request_id: u8, ctx: &ConfigCtx<'_>) -> ConfigReply {
    let ConfigCtx {
        store,
        lora_status,
        usb_status,
        ui_handle,
        node_page_destination,
        ..
    } = *ctx;
    match action {
        ConfigAction::SetLoRaProfile(profile) => {
            ConfigReply::response(request_id, persist_profile(profile, true, store).await)
        }
        ConfigAction::ResetLoRaProfile => ConfigReply::response(
            request_id,
            persist_profile(DEFAULT_915_PROFILE, false, store).await,
        ),
        ConfigAction::ToggleInterface(interface) => {
            match interface {
                ConfigInterface::Lora => lora_status.toggle_enabled(),
                ConfigInterface::Usb => usb_status.toggle_enabled(),
                ConfigInterface::Ble => BluetoothAutoStatus::new(&BLE_SHARED).toggle_enabled(),
            }
            ConfigReply::response(request_id, ConfigResult::Ok)
        }
        ConfigAction::Sleep => {
            lora_status.disable();
            usb_status.disable();
            BluetoothAutoStatus::new(&BLE_SHARED).disable();
            ConfigReply::response(request_id, ConfigResult::Ok)
        }
        ConfigAction::Wake => {
            lora_status.enable();
            usb_status.enable();
            BluetoothAutoStatus::new(&BLE_SHARED).enable();
            ConfigReply::response(request_id, ConfigResult::Ok)
        }
        ConfigAction::Announce => {
            let _ = ui_handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                destination: node_page_destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }));
            ConfigReply::response(request_id, ConfigResult::Ok)
        }
        ConfigAction::RequestSnapshot => {
            let body = build_snapshot(ctx).await;
            let mut buf = [0u8; MAX_SNAPSHOT_BODY_BYTES];
            match body.encode(&mut buf) {
                Ok(len) => {
                    let mut wire = HeaplessVec::new();
                    let _ = wire.extend_from_slice(&buf[..len]);
                    ConfigReply::Snapshot {
                        schema_version: SNAPSHOT_SCHEMA_VERSION,
                        body: wire,
                    }
                }
                Err(_) => ConfigReply::response(request_id, ConfigResult::ApplyFailed),
            }
        }
    }
}

/// Apply `profile` to the radio and persist it (save when `is_set`, reset to
/// the default otherwise), mapping the shared result onto a wire `ConfigResult`.
async fn persist_profile(
    profile: RadioProfile,
    is_set: bool,
    store: &ProfileStore,
) -> ConfigResult {
    let result = hopspot::apply_and_persist_radio_profile(
        async { LORA_CONTROL.apply(profile).await == LoRaApplyOutcome::Applied },
        || async {
            if is_set {
                store.lock().await.save(profile).await.is_ok()
            } else {
                store.lock().await.reset().await.is_ok()
            }
        },
    )
    .await;
    match result {
        hopspot::RadioProfileChangeResult::Saved => ConfigResult::Ok,
        hopspot::RadioProfileChangeResult::ApplyFailed => ConfigResult::ApplyFailed,
        hopspot::RadioProfileChangeResult::ProfileNotSaved => ConfigResult::ProfileNotSaved,
    }
}

/// Assemble a [`SnapshotBody`] from live runtime state. The webUI renders this
/// in place of the e-ink screen on headless boards. v1 carries the radio
/// profile, per-interface status, BLE recovery, the LoRa spectrum menu, the
/// profile-store persistence state, and per-interface destination/link counts.
/// Battery, last-activity, and boot notices are deferred to a later schema.
async fn build_snapshot(ctx: &ConfigCtx<'_>) -> SnapshotBody {
    let ConfigCtx {
        store,
        lora_status,
        usb_status,
        lora_spectrum,
        ..
    } = *ctx;
    let mut sections: HeaplessVec<SnapshotSection, MAX_SNAPSHOT_SECTIONS> = HeaplessVec::new();

    let mut version = HeaplessString::new();
    let _ = version.push_str(env!("CARGO_PKG_VERSION"));
    let _ = sections.push(SnapshotSection::DeviceInfo { version });

    let _ = sections.push(SnapshotSection::Persistence {
        state: map_persistence(board::persistence_state()),
    });

    let _ = sections.push(SnapshotSection::LoraStatus(status_body(
        lora_status,
        lora_status.is_enabled(),
    )));
    let _ = sections.push(SnapshotSection::UsbStatus(status_body(
        usb_status,
        usb_status.is_enabled(),
    )));

    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let _ = sections.push(SnapshotSection::BleStatus {
        status: status_body(&ble, ble.is_enabled()),
        failure_reason: failure_reason_string(ble.failure_reason()),
    });
    let _ = sections.push(SnapshotSection::BleRecovery(ble_recovery_body(&ble)));

    let spectrum = lora_spectrum.snapshot();
    let _ = sections.push(SnapshotSection::LoraSpectrum(LoRaSpectrumBody {
        channel_busy_per_mille: spectrum.channel_busy_per_mille,
        noise_floor_dbm: spectrum.noise_floor_dbm,
        cca_threshold_dbm: spectrum.cca_threshold_dbm,
        deferrals: spectrum.deferrals,
        false_preambles: spectrum.false_preambles,
        contention_timeouts: spectrum.contention_timeouts,
        duty_holds: spectrum.duty_holds,
        duty_timeouts: spectrum.duty_timeouts,
        radio_recoveries: spectrum.radio_recoveries,
    }));

    // The persisted profile is the source of truth the webUI editor loads; the
    // live applied profile is not readable from the config task and matches the
    // persisted one after a successful apply-and-persist.
    let profile = match store.lock().await.load(DEFAULT_915_PROFILE).await {
        Ok(loaded) => loaded.profile,
        Err(_) => DEFAULT_915_PROFILE,
    };
    let _ = sections.push(SnapshotSection::RadioProfile(profile));

    let mut counts = HeaplessVec::new();
    let _ = counts.push(interface_count(ConfigInterface::Lora, lora_status.id()));
    let _ = counts.push(interface_count(ConfigInterface::Usb, usb_status.id()));
    let _ = counts.push(interface_count(ConfigInterface::Ble, BLE_SUPERVISOR_ID));
    let _ = sections.push(SnapshotSection::InterfaceCounts(counts));

    SnapshotBody { sections }
}

/// Project the runtime persistence state machine onto its snapshot wire rep.
fn map_persistence(state: hopspot::PersistenceState) -> SnapshotPersistence {
    match state {
        hopspot::PersistenceState::Durable => SnapshotPersistence::Durable,
        hopspot::PersistenceState::Deferred => SnapshotPersistence::Deferred,
        hopspot::PersistenceState::Failed => SnapshotPersistence::Failed,
    }
}

/// Build an [`InterfaceStatusBody`] from any [`InterfaceStatus`] plus the
/// enabled flag (not part of the trait, so callers pass it in).
fn status_body<S: InterfaceStatus>(status: &S, enabled: bool) -> InterfaceStatusBody {
    InterfaceStatusBody {
        enabled,
        connection: status.connection(),
        rx_bytes: status.rx_bytes(),
        tx_bytes: status.tx_bytes(),
        airtime: status.airtime(),
        transfer_rates: status.transfer_rates(),
    }
}

/// Copy the BLE failure reason into a bounded snapshot string. `None` and
/// over-long reasons collapse to an empty string; the webUI shows the
/// connection state regardless.
fn failure_reason_string(reason: Option<&'static str>) -> HeaplessString<MAX_FAILURE_REASON_BYTES> {
    let mut s = HeaplessString::new();
    if let Some(reason) = reason {
        let _ = s.push_str(reason);
    }
    s
}

/// BLE recovery counters plus the supervisor-lane egress pressure and the
/// live member count.
fn ble_recovery_body<const MEMBERS: usize>(
    ble: &BluetoothAutoStatus<MEMBERS>,
) -> BluetoothRecoveryBody {
    let counters = ble.recovery_counters();
    BluetoothRecoveryBody {
        ingress_pressure: counters.ingress_pressure,
        setup_failures: counters.setup_failures,
        transport_closures: counters.transport_closures,
        egress_pressure_events: BLE_MANIFOLD_LANE.egress_pressure_events(),
        member_count: ble.members().count().min(u8::MAX as usize) as u8,
    }
}

/// One interface's destination/link counts from the shared interface store.
fn interface_count(kind: ConfigInterface, interface: InterfaceId) -> InterfaceCount {
    let counts = INTERFACE_STORE.counts(interface);
    InterfaceCount {
        kind,
        destinations: counts.destinations,
        links: counts.links,
        transported_links: counts.transported_links,
    }
}
