use prns_runtime::interfaces::bluetooth_auto::core::{self, BleIdentity};
use prns_runtime::interfaces::ifac::IfacContext;
use prns_runtime::interfaces::{
    ConfiguredInterfacePolicy, EffectiveInterfacePolicy, InterfaceId, InterfaceKind,
    InterfaceStatus, ReportsStatus,
};
use prns_runtime::runtime::{
    ephemeral_ble_identity, Attachable, Fleet, InterfaceSupervisor, PrnsNodeHandle,
};

use crate::ble::tokio::BluetoothAutoStatus;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoBle;

pub struct ConfiguredAutoBle {
    policy: EffectiveInterfacePolicy,
}

impl AutoBle {
    #[must_use]
    pub fn with_policy(policy: EffectiveInterfacePolicy) -> ConfiguredAutoBle {
        ConfiguredAutoBle { policy }
    }
}

pub struct AttachedBle {
    status: BluetoothAutoStatus,
}

impl AttachedBle {
    #[must_use]
    pub fn status(&self) -> BluetoothAutoStatus {
        self.status.clone()
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.status.id()
    }
}

impl Attachable for AutoBle {
    type Attached = AttachedBle;
    fn attach_to(self, handle: &PrnsNodeHandle) -> AttachedBle {
        attach_platform_bluetooth(
            handle,
            prns_runtime::interfaces::bluetooth_auto::core::defaults_for_bitrate(
                prns_runtime::interfaces::bluetooth_auto::core::BLE_BITRATE_GUESS_BPS,
            )
            .configured(ConfiguredInterfacePolicy::default()),
            None,
        )
    }

    fn attach_to_with_ifac(
        self,
        handle: &PrnsNodeHandle,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedBle {
        attach_platform_bluetooth(
            handle,
            prns_runtime::interfaces::bluetooth_auto::core::defaults_for_bitrate(
                prns_runtime::interfaces::bluetooth_auto::core::BLE_BITRATE_GUESS_BPS,
            )
            .configured(ConfiguredInterfacePolicy::default()),
            Some((ifac, network_name)),
        )
    }
}

impl Attachable for ConfiguredAutoBle {
    type Attached = AttachedBle;

    fn attach_to(self, handle: &PrnsNodeHandle) -> AttachedBle {
        attach_platform_bluetooth(handle, self.policy, None)
    }

    fn attach_to_with_ifac(
        self,
        handle: &PrnsNodeHandle,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedBle {
        attach_platform_bluetooth(handle, self.policy, Some((ifac, network_name)))
    }
}

fn attach_platform_bluetooth(
    handle: &PrnsNodeHandle,
    policy: EffectiveInterfacePolicy,
    ifac: Option<(IfacContext, Option<String>)>,
) -> AttachedBle {
    let status = BluetoothAutoStatus::new();
    let bluetooth = PlatformBluetooth {
        ble_identity: ephemeral_ble_identity(),
        policy,
        status: status.clone(),
    };
    match ifac {
        Some((ifac, network_name)) => {
            handle.supervise_with_ifac_name(bluetooth, ifac, network_name)
        }
        None => handle.supervise(bluetooth),
    };
    AttachedBle { status }
}

struct PlatformBluetooth {
    ble_identity: BleIdentity,
    policy: EffectiveInterfacePolicy,
    status: BluetoothAutoStatus,
}

impl ReportsStatus for PlatformBluetooth {
    fn status_view(&self) -> Option<prns_runtime::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_runtime::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl InterfaceSupervisor for PlatformBluetooth {
    const KIND: InterfaceKind = InterfaceKind::BluetoothAuto;

    fn channel_tag(&self) -> &[u8] {
        core::GROUP_ID
    }

    async fn run(self, fleet: Fleet) {
        run_platform_bluetooth(fleet, self.ble_identity, self.status, self.policy).await;
    }
}

#[cfg(target_os = "macos")]
async fn run_platform_bluetooth(
    fleet: Fleet,
    ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
) {
    use crate::ble::tokio::BluetoothAuto;
    use prns_ffi::ble::macos::MacosBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::core::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };
    use prns_runtime::interfaces::bluetooth_auto::seam::BleBackend;

    match MacosBleBackend::new(ble_identity).await {
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
            )
            .with_policy(policy);
            crate::diagnostic_log::info!(
                "bluetooth: supervising CoreBluetooth, L2CAP psm {:#06x}",
                psm.get()
            );
            bluetooth.run(fleet).await;
        }
        Err(error) => {
            status.mark_failed(Some("Bluetooth not granted or radio unavailable"));
            crate::diagnostic_log::warn!(
                "bluetooth disabled ({error:?}); grant Bluetooth in System Settings > Privacy & Security > Bluetooth"
            );
            std::future::pending().await
        }
    }
}

#[cfg(target_os = "ios")]
async fn run_platform_bluetooth(
    fleet: Fleet,
    ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
) {
    use crate::ble::tokio::BluetoothAuto;
    use prns_ffi::ble::macos::MacosBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::core::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };
    use prns_runtime::interfaces::bluetooth_auto::limits;
    use prns_runtime::interfaces::bluetooth_auto::seam::BleBackend;

    match MacosBleBackend::new(ble_identity).await {
        Ok(backend) => {
            let psm = backend.psm();
            let bluetooth = BluetoothAuto::<_, { limits::IOS_MAX_PEERS }>::with_status(
                backend,
                ble_identity,
                Endpoint::CoreBluetooth(AppleHost::Ios),
                LinkCapabilities {
                    l2cap: None,
                    link_mtu: BLE_HW_MTU as u16,
                },
                status,
            )
            .with_policy(policy);
            crate::diagnostic_log::info!(
                "bluetooth: supervising CoreBluetooth (iOS), GATT-only floor; local L2CAP psm {:#06x} withheld",
                psm.get()
            );
            bluetooth.run(fleet).await;
        }
        Err(error) => {
            status.mark_failed(Some("Bluetooth not granted or radio unavailable"));
            crate::diagnostic_log::warn!(
                "bluetooth disabled ({error:?}); grant Bluetooth in Settings > Privacy & Security > Bluetooth"
            );
            std::future::pending().await
        }
    }
}

#[cfg(target_os = "windows")]
async fn run_platform_bluetooth(
    fleet: Fleet,
    ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
) {
    use crate::ble::tokio::BluetoothAuto;
    use prns_ffi::ble::windows::WindowsBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::core::{
        Endpoint, LinkCapabilities, WinRtHost, BLE_HW_MTU,
    };
    use prns_runtime::interfaces::bluetooth_auto::seam::BleBackend;

    match WindowsBleBackend::new(ble_identity).await {
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
            )
            .with_policy(policy);
            crate::diagnostic_log::info!("bluetooth: supervising WinRT (GATT-only)");
            bluetooth.run(fleet).await;
        }
        Err(error) => {
            status.mark_failed(Some("Bluetooth off or unsupported"));
            crate::diagnostic_log::warn!(
                "bluetooth disabled ({error:?}); check that Bluetooth is on and supported on this machine"
            );
            std::future::pending().await
        }
    }
}

#[cfg(target_os = "linux")]
async fn run_platform_bluetooth(
    fleet: Fleet,
    ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
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
        crate::diagnostic_log::warn!(
            "bluetooth disabled: invalid Linux control PSM {CONTROL_PSM:#x}"
        );
        return std::future::pending().await;
    };
    match BluerBackend::open(psm, ble_identity).await {
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
            )
            .with_policy(policy);
            crate::diagnostic_log::info!(
                "bluetooth: supervising BlueZ/BlueR, control psm {CONTROL_PSM:#x}"
            );
            bluetooth.run(fleet).await;
        }
        Err(error) => {
            status.mark_failed(Some("bluetoothd or adapter unavailable"));
            crate::diagnostic_log::warn!(
                "bluetooth disabled ({error:?}); check bluetoothd, adapter power, and BlueZ LE advertising/GATT support"
            );
            std::future::pending().await
        }
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux"
)))]
async fn run_platform_bluetooth(
    _fleet: Fleet,
    _ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
    _policy: EffectiveInterfacePolicy,
) {
    status.mark_failed(Some("no native BLE backend for this platform"));
    crate::diagnostic_log::warn!("bluetooth disabled: no native AutoBle backend for this platform");
    std::future::pending().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_runtime::interfaces::ConnectionState;
    use prns_runtime::runtime::{Manual, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe};
    use prns_runtime::storage::GrowableHeap;

    #[test]
    fn auto_ble_registers_before_platform_backend_initialization() {
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
            app_state: (),
            storage: GrowableHeap,
            routes: prns_runtime::routes![],
            interfaces: Manual,
            on_event: |_event, _state: &()| {},
        });
        let attached = node.handle().attach(AutoBle);

        assert!(node
            .handle()
            .set_interface_name(attached.id(), "Configured BLE"));
        let inventory = node.handle().interface_inventory();
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].name.as_deref(), Some("Configured BLE"));
        assert_eq!(
            inventory[0].snapshot.connection,
            ConnectionState::Initializing
        );
    }
}
