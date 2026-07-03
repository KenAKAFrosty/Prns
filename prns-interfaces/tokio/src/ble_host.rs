//! Zero-config Reticulum-over-BLE for a std host: pick this platform's native backend,
//! introduce it as the node's identity, and supervise the fleet once the radio settles.

use prns_runtime::interfaces::bluetooth_auto::core::BleIdentity;
use prns_runtime::runtime::{Attachable, TokioPrnsHandle};

/// Reticulum-over-BLE with everything defaulted: CoreBluetooth on macOS, WinRT (GATT-only)
/// on Windows, BlueZ/BlueR on Linux. Needs the node to hold an identity (a `Single`
/// pre-configured destination). Backend bring-up runs on its own task, so a slow or off
/// radio never blocks the node coming up; a host without a usable radio logs and runs on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoBle;

impl Attachable for AutoBle {
    type Attached = ();
    fn attach_to(self, handle: &TokioPrnsHandle) {
        let Some(identity) = handle.node_identity() else {
            log::warn!("bluetooth disabled: this node holds no identity to introduce itself as");
            return;
        };
        spawn_platform_bluetooth(handle.clone(), BleIdentity::new(*identity.as_bytes()));
    }
}

#[cfg(target_os = "macos")]
fn spawn_platform_bluetooth(handle: TokioPrnsHandle, ble_identity: BleIdentity) {
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
                let bluetooth = BluetoothAuto::<_, { MacosBleBackend::MAX_PEERS }>::new(
                    backend,
                    ble_identity,
                    Endpoint::CoreBluetooth(AppleHost::MacOs),
                    LinkCapabilities {
                        l2cap: Some(psm),
                        link_mtu: BLE_HW_MTU as u16,
                    },
                );
                handle.supervise(bluetooth);
                log::info!(
                    "bluetooth: supervising CoreBluetooth, L2CAP psm {:#06x}",
                    psm.get()
                );
            }
            Err(error) => {
                log::warn!(
                    "bluetooth disabled ({error:?}); grant Bluetooth in System Settings > Privacy & Security > Bluetooth"
                );
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn spawn_platform_bluetooth(handle: TokioPrnsHandle, ble_identity: BleIdentity) {
    use crate::ble::tokio::BluetoothAuto;
    use prns_ffi::ble::windows::WindowsBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::core::{
        Endpoint, LinkCapabilities, WinRtHost, BLE_HW_MTU,
    };
    use prns_runtime::interfaces::bluetooth_auto::seam::BleBackend;

    tokio::spawn(async move {
        match WindowsBleBackend::new().await {
            Ok(backend) => {
                let bluetooth = BluetoothAuto::<_, { WindowsBleBackend::MAX_PEERS }>::new(
                    backend,
                    ble_identity,
                    Endpoint::WinRt(WinRtHost::Windows),
                    LinkCapabilities {
                        l2cap: None,
                        link_mtu: BLE_HW_MTU as u16,
                    },
                );
                handle.supervise(bluetooth);
                log::info!("bluetooth: supervising WinRT (GATT-only)");
            }
            Err(error) => {
                log::warn!(
                    "bluetooth disabled ({error:?}); check that Bluetooth is on and supported on this machine"
                );
            }
        }
    });
}

#[cfg(target_os = "linux")]
fn spawn_platform_bluetooth(handle: TokioPrnsHandle, ble_identity: BleIdentity) {
    use crate::ble::bluer::BluerBackend;
    use crate::ble::tokio::BluetoothAuto;
    use prns_runtime::interfaces::bluetooth_auto::core::{
        BlueZHost, Endpoint, LinkCapabilities, Psm, BLE_HW_MTU,
    };
    use prns_runtime::interfaces::bluetooth_auto::seam::BleBackend;

    const CONTROL_PSM: u16 = 0x0083;

    let Some(psm) = Psm::new(CONTROL_PSM) else {
        log::warn!("bluetooth disabled: invalid Linux control PSM {CONTROL_PSM:#x}");
        return;
    };
    tokio::spawn(async move {
        match BluerBackend::open(psm).await {
            Ok(backend) => {
                let bluetooth = BluetoothAuto::<_, { BluerBackend::MAX_PEERS }>::new(
                    backend,
                    ble_identity,
                    Endpoint::BlueZ(BlueZHost::Linux),
                    LinkCapabilities {
                        l2cap: Some(psm),
                        link_mtu: BLE_HW_MTU as u16,
                    },
                );
                handle.supervise(bluetooth);
                log::info!("bluetooth: supervising BlueZ/BlueR, control psm {CONTROL_PSM:#x}");
            }
            Err(error) => {
                log::warn!(
                    "bluetooth disabled ({error:?}); check bluetoothd, adapter power, and BlueZ LE advertising/GATT support"
                );
            }
        }
    });
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn spawn_platform_bluetooth(_handle: TokioPrnsHandle, _ble_identity: BleIdentity) {
    log::warn!("bluetooth disabled: no native AutoBle backend for this platform");
}
