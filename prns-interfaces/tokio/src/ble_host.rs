//! Zero-config Reticulum-over-BLE for a std host: pick this platform's native backend,
//! introduce it under a fresh ephemeral wire identity, and supervise the fleet once the
//! radio settles. Attaching yields the fleet's live status immediately — real before the
//! radio settles, `Failed` if bring-up is refused, toggleable the whole way through.

use prns_runtime::interfaces::bluetooth_auto::core::BleIdentity;
use prns_runtime::runtime::{ephemeral_ble_identity, Attachable, TokioPrnsHandle};

use crate::ble::tokio::BluetoothAutoStatus;

/// Reticulum-over-BLE with everything defaulted: CoreBluetooth on macOS and iOS, WinRT
/// (GATT-only) on Windows, BlueZ/BlueR on Linux. Backend bring-up runs on its own task, so
/// a slow or off radio never blocks the node coming up; a host without a usable radio marks
/// the fleet `Failed`, logs, and the node runs on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoBle;

/// The BLE fleet's handle, live from the moment of attach: `Initializing` while the radio
/// settles, `Failed` if bring-up is refused, and [`set_enabled`](BluetoothAutoStatus::set_enabled)
/// effective throughout — the same status object the supervisor reports on once it runs.
pub struct AttachedBle {
    status: BluetoothAutoStatus,
}

impl AttachedBle {
    #[must_use]
    pub fn status(&self) -> BluetoothAutoStatus {
        self.status.clone()
    }
}

impl Attachable for AutoBle {
    type Attached = AttachedBle;
    fn attach_to(self, handle: &TokioPrnsHandle) -> AttachedBle {
        let status = BluetoothAutoStatus::new();
        spawn_platform_bluetooth(handle.clone(), ephemeral_ble_identity(), status.clone());
        AttachedBle { status }
    }
}

#[cfg(target_os = "macos")]
fn spawn_platform_bluetooth(
    handle: TokioPrnsHandle,
    ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
) {
    use crate::ble::tokio::BluetoothAuto;
    use prns_ffi::ble::macos::MacosBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::core::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };
    use prns_runtime::interfaces::bluetooth_auto::seam::BleBackend;

    tokio::spawn(async move {
        match MacosBleBackend::new().await {
            Ok(backend) => {
                let psm = backend.psm();
                let bluetooth = BluetoothAuto::<_, { MacosBleBackend::MAX_PEERS }>::with_status(
                    backend,
                    ble_identity,
                    Endpoint::CoreBluetooth(AppleHost::MacOs),
                    LinkCapabilities {
                        l2cap: Some(psm),
                        link_mtu: BLE_HW_MTU as u16,
                    },
                    status,
                );
                handle.supervise(bluetooth);
                log::info!(
                    "bluetooth: supervising CoreBluetooth, L2CAP psm {:#06x}",
                    psm.get()
                );
            }
            Err(error) => {
                status.mark_failed(Some("Bluetooth not granted or radio unavailable"));
                log::warn!(
                    "bluetooth disabled ({error:?}); grant Bluetooth in System Settings > Privacy & Security > Bluetooth"
                );
            }
        }
    });
}

#[cfg(target_os = "ios")]
fn spawn_platform_bluetooth(
    handle: TokioPrnsHandle,
    ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
) {
    use crate::ble::tokio::BluetoothAuto;
    use prns_ffi::ble::macos::MacosBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::core::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };
    use prns_runtime::interfaces::bluetooth_auto::limits;
    use prns_runtime::interfaces::bluetooth_auto::seam::BleBackend;

    tokio::spawn(async move {
        match MacosBleBackend::new().await {
            Ok(backend) => {
                let psm = backend.psm();
                let bluetooth = BluetoothAuto::<_, { limits::IOS_MAX_PEERS }>::with_status(
                    backend,
                    ble_identity,
                    Endpoint::CoreBluetooth(AppleHost::Ios),
                    // iOS keeps the plain GATT data floor: CoreBluetooth's L2CAP path can
                    // trigger an OS pairing prompt when a laptop opens the channel.
                    LinkCapabilities {
                        l2cap: None,
                        link_mtu: BLE_HW_MTU as u16,
                    },
                    status,
                );
                handle.supervise(bluetooth);
                log::info!(
                    "bluetooth: supervising CoreBluetooth (iOS), GATT-only floor; local L2CAP psm {:#06x} withheld",
                    psm.get()
                );
            }
            Err(error) => {
                status.mark_failed(Some("Bluetooth not granted or radio unavailable"));
                log::warn!(
                    "bluetooth disabled ({error:?}); grant Bluetooth in Settings > Privacy & Security > Bluetooth"
                );
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn spawn_platform_bluetooth(
    handle: TokioPrnsHandle,
    ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
) {
    use crate::ble::tokio::BluetoothAuto;
    use prns_ffi::ble::windows::WindowsBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::core::{
        Endpoint, LinkCapabilities, WinRtHost, BLE_HW_MTU,
    };
    use prns_runtime::interfaces::bluetooth_auto::seam::BleBackend;

    tokio::spawn(async move {
        match WindowsBleBackend::new().await {
            Ok(backend) => {
                let bluetooth = BluetoothAuto::<_, { WindowsBleBackend::MAX_PEERS }>::with_status(
                    backend,
                    ble_identity,
                    Endpoint::WinRt(WinRtHost::Windows),
                    LinkCapabilities {
                        l2cap: None,
                        link_mtu: BLE_HW_MTU as u16,
                    },
                    status,
                );
                handle.supervise(bluetooth);
                log::info!("bluetooth: supervising WinRT (GATT-only)");
            }
            Err(error) => {
                status.mark_failed(Some("Bluetooth off or unsupported"));
                log::warn!(
                    "bluetooth disabled ({error:?}); check that Bluetooth is on and supported on this machine"
                );
            }
        }
    });
}

#[cfg(target_os = "linux")]
fn spawn_platform_bluetooth(
    handle: TokioPrnsHandle,
    ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
) {
    use crate::ble::bluer::BluerBackend;
    use crate::ble::tokio::BluetoothAuto;
    use prns_runtime::interfaces::bluetooth_auto::core::{
        BlueZHost, Endpoint, LinkCapabilities, Psm, BLE_HW_MTU,
    };
    use prns_runtime::interfaces::bluetooth_auto::seam::BleBackend;

    const CONTROL_PSM: u16 = 0x0083;

    let Some(psm) = Psm::new(CONTROL_PSM) else {
        status.mark_failed(Some("invalid Linux control PSM"));
        log::warn!("bluetooth disabled: invalid Linux control PSM {CONTROL_PSM:#x}");
        return;
    };
    tokio::spawn(async move {
        match BluerBackend::open(psm).await {
            Ok(backend) => {
                let bluetooth = BluetoothAuto::<_, { BluerBackend::MAX_PEERS }>::with_status(
                    backend,
                    ble_identity,
                    Endpoint::BlueZ(BlueZHost::Linux),
                    LinkCapabilities {
                        l2cap: Some(psm),
                        link_mtu: BLE_HW_MTU as u16,
                    },
                    status,
                );
                handle.supervise(bluetooth);
                log::info!("bluetooth: supervising BlueZ/BlueR, control psm {CONTROL_PSM:#x}");
            }
            Err(error) => {
                status.mark_failed(Some("bluetoothd or adapter unavailable"));
                log::warn!(
                    "bluetooth disabled ({error:?}); check bluetoothd, adapter power, and BlueZ LE advertising/GATT support"
                );
            }
        }
    });
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux"
)))]
fn spawn_platform_bluetooth(
    _handle: TokioPrnsHandle,
    _ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
) {
    status.mark_failed(Some("no native BLE backend for this platform"));
    log::warn!("bluetooth disabled: no native AutoBle backend for this platform");
}
