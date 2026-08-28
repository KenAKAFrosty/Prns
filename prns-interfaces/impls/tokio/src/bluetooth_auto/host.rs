use prns_runtime::interfaces::bluetooth_auto::{self as contract, BleIdentity};
use prns_runtime::interfaces::IfacContext;
use prns_runtime::interfaces::{
    ConfiguredInterfacePolicy, EffectiveInterfacePolicy, InterfaceId, InterfaceKind,
    InterfaceStatus, ReportsStatus,
};
use prns_runtime::runtime::{Attachable, Fleet, InterfaceSupervisor, PrnsNodeHandle};

use super::BluetoothAutoStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoBle {
    identity: BleIdentity,
    group_tag: [u8; 4],
}

/// Canonical name for the Bluetooth LE auto-interface constructor.
pub type AutoBluetoothLe = AutoBle;

pub struct ConfiguredAutoBle {
    identity: BleIdentity,
    policy: EffectiveInterfacePolicy,
    group_tag: [u8; 4],
}

/// Canonical name for a configured Bluetooth LE auto-interface.
pub type ConfiguredAutoBluetoothLe = ConfiguredAutoBle;

impl AutoBle {
    #[must_use]
    pub const fn new(identity: BleIdentity) -> Self {
        Self {
            identity,
            group_tag: contract::DEFAULT_GROUP_TAG,
        }
    }

    #[must_use]
    pub const fn with_group_tag(mut self, group_tag: [u8; 4]) -> Self {
        self.group_tag = group_tag;
        self
    }

    #[must_use]
    pub fn with_policy(
        identity: BleIdentity,
        policy: EffectiveInterfacePolicy,
    ) -> ConfiguredAutoBle {
        Self::with_policy_and_group(identity, policy, contract::default_group_tag())
    }

    #[must_use]
    pub fn with_policy_and_group(
        identity: BleIdentity,
        policy: EffectiveInterfacePolicy,
        group_tag: [u8; 4],
    ) -> ConfiguredAutoBle {
        ConfiguredAutoBle {
            identity,
            policy,
            group_tag,
        }
    }

    /// Creates the restoration-aware CoreBluetooth managers immediately while leaving radio
    /// authorization and service readiness to the attached asynchronous supervisor.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub async fn prepare(
        identity: BleIdentity,
    ) -> Result<PreparedAutoBle, prns_ffi::bluetooth_auto::macos::MacosBleError> {
        Self::prepare_with_group(identity, contract::default_group_tag()).await
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub async fn prepare_with_group(
        identity: BleIdentity,
        group_tag: [u8; 4],
    ) -> Result<PreparedAutoBle, prns_ffi::bluetooth_auto::macos::MacosBleError> {
        let backend =
            prns_ffi::bluetooth_auto::macos::MacosBleBackend::prepare(identity, group_tag).await?;
        Ok(PreparedAutoBle {
            identity,
            group_tag,
            policy: prns_runtime::interfaces::bluetooth_auto::defaults_for_bitrate(
                prns_runtime::interfaces::bluetooth_auto::BLE_BITRATE_GUESS_BPS,
            )
            .configured(ConfiguredInterfacePolicy::default()),
            status: BluetoothAutoStatus::new(),
            backend: Some(backend),
        })
    }

    /// Produces a failed-but-supervised Bluetooth LE attachment when native manager preparation itself
    /// cannot be started. The core node and every other transport remain available.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub fn unavailable(identity: BleIdentity) -> PreparedAutoBle {
        PreparedAutoBle {
            identity,
            group_tag: contract::DEFAULT_GROUP_TAG,
            policy: prns_runtime::interfaces::bluetooth_auto::defaults_for_bitrate(
                prns_runtime::interfaces::bluetooth_auto::BLE_BITRATE_GUESS_BPS,
            )
            .configured(ConfiguredInterfacePolicy::default()),
            status: BluetoothAutoStatus::new(),
            backend: None,
        }
    }
}

pub struct AttachedBle {
    status: BluetoothAutoStatus,
}

/// Canonical name for an attached Bluetooth LE auto-interface.
pub type AttachedBluetoothLe = AttachedBle;

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub struct PreparedAutoBle {
    identity: BleIdentity,
    group_tag: [u8; 4],
    policy: EffectiveInterfacePolicy,
    status: BluetoothAutoStatus,
    backend: Option<prns_ffi::bluetooth_auto::macos::PreparedMacosBleBackend>,
}

/// Canonical name for a prepared Apple-platform Bluetooth LE auto-interface.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub type PreparedAutoBluetoothLe = PreparedAutoBle;

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
            self.identity,
            self.group_tag,
            prns_runtime::interfaces::bluetooth_auto::defaults_for_bitrate(
                prns_runtime::interfaces::bluetooth_auto::BLE_BITRATE_GUESS_BPS,
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
            self.identity,
            self.group_tag,
            prns_runtime::interfaces::bluetooth_auto::defaults_for_bitrate(
                prns_runtime::interfaces::bluetooth_auto::BLE_BITRATE_GUESS_BPS,
            )
            .configured(ConfiguredInterfacePolicy::default()),
            Some((ifac, network_name)),
        )
    }
}

impl Attachable for ConfiguredAutoBle {
    type Attached = AttachedBle;

    fn attach_to(self, handle: &PrnsNodeHandle) -> AttachedBle {
        attach_platform_bluetooth(handle, self.identity, self.group_tag, self.policy, None)
    }

    fn attach_to_with_ifac(
        self,
        handle: &PrnsNodeHandle,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedBle {
        attach_platform_bluetooth(
            handle,
            self.identity,
            self.group_tag,
            self.policy,
            Some((ifac, network_name)),
        )
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl Attachable for PreparedAutoBle {
    type Attached = AttachedBle;

    fn attach_to(self, handle: &PrnsNodeHandle) -> AttachedBle {
        let status = self.status.clone();
        handle.supervise(PreparedPlatformBluetooth {
            identity: self.identity,
            group_tag: self.group_tag,
            policy: self.policy,
            status: self.status,
            backend: self.backend,
        });
        AttachedBle { status }
    }

    fn attach_to_with_ifac(
        self,
        handle: &PrnsNodeHandle,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedBle {
        let status = self.status.clone();
        handle.supervise_with_ifac_name(
            PreparedPlatformBluetooth {
                identity: self.identity,
                group_tag: self.group_tag,
                policy: self.policy,
                status: self.status,
                backend: self.backend,
            },
            ifac,
            network_name,
        );
        AttachedBle { status }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
struct PreparedPlatformBluetooth {
    identity: BleIdentity,
    group_tag: [u8; 4],
    policy: EffectiveInterfacePolicy,
    status: BluetoothAutoStatus,
    backend: Option<prns_ffi::bluetooth_auto::macos::PreparedMacosBleBackend>,
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
const APPLE_BLE_READINESS_RETRY_DELAY: core::time::Duration = core::time::Duration::from_secs(2);

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl ReportsStatus for PreparedPlatformBluetooth {
    fn status_view(&self) -> Option<prns_runtime::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_runtime::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl InterfaceSupervisor for PreparedPlatformBluetooth {
    const KIND: InterfaceKind = InterfaceKind::BluetoothAuto;

    fn channel_tag(&self) -> &[u8] {
        contract::CHANNEL_TAG
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        run_prepared_platform_bluetooth(
            fleet,
            self.identity,
            self.group_tag,
            self.status,
            self.policy,
            self.backend,
        )
        .await;
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
async fn run_prepared_platform_bluetooth(
    fleet: Fleet,
    ble_identity: BleIdentity,
    group_tag: [u8; 4],
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
    backend: Option<prns_ffi::bluetooth_auto::macos::PreparedMacosBleBackend>,
) {
    use super::BluetoothAuto;
    use prns_ffi::bluetooth_auto::macos::MacosBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };

    let mut prepared = backend;
    loop {
        let candidate = match prepared.take() {
            Some(backend) => backend,
            None => match MacosBleBackend::prepare(ble_identity, group_tag).await {
                Ok(backend) => backend,
                Err(error) => {
                    status.mark_failed(Some("Bluetooth native manager unavailable"));
                    crate::diagnostic_log::warn!(
                        "bluetooth manager preparation failed ({error:?}); retrying in {}s",
                        APPLE_BLE_READINESS_RETRY_DELAY.as_secs()
                    );
                    tokio::time::sleep(APPLE_BLE_READINESS_RETRY_DELAY).await;
                    continue;
                }
            },
        };

        match candidate.ready().await {
            Ok(backend) => {
                status.clear_failure();
                let psm = backend.psm();
                #[cfg(target_os = "macos")]
                let endpoint = Endpoint::CoreBluetooth(AppleHost::MacOs);
                #[cfg(target_os = "ios")]
                let endpoint = Endpoint::CoreBluetooth(AppleHost::Ios);
                #[cfg(target_os = "macos")]
                let l2cap = Some(psm);
                #[cfg(target_os = "ios")]
                let l2cap = None;
                let bluetooth = BluetoothAuto::<_, { MacosBleBackend::MAX_PEERS }>::with_status(
                    backend,
                    ble_identity,
                    endpoint,
                    LinkCapabilities {
                        l2cap,
                        link_mtu: BLE_HW_MTU as u16,
                    },
                    group_tag,
                    status,
                )
                .with_policy(policy);
                crate::diagnostic_log::info!(
                    "bluetooth: supervising prepared CoreBluetooth backend, local psm {:#06x}",
                    psm.get()
                );
                bluetooth.run(fleet).await;
                return;
            }
            Err(error) => {
                status.mark_failed(Some("Bluetooth not granted or radio unavailable"));
                crate::diagnostic_log::warn!(
                    "bluetooth readiness unavailable ({error:?}); retrying manager preparation in {}s",
                    APPLE_BLE_READINESS_RETRY_DELAY.as_secs()
                );
                tokio::time::sleep(APPLE_BLE_READINESS_RETRY_DELAY).await;
            }
        }
    }
}

fn attach_platform_bluetooth(
    handle: &PrnsNodeHandle,
    ble_identity: BleIdentity,
    group_tag: [u8; 4],
    policy: EffectiveInterfacePolicy,
    ifac: Option<(IfacContext, Option<String>)>,
) -> AttachedBle {
    let status = BluetoothAutoStatus::new();
    let bluetooth = PlatformBluetooth {
        ble_identity,
        group_tag,
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
    group_tag: [u8; 4],
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
        contract::CHANNEL_TAG
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        run_platform_bluetooth(
            fleet,
            self.ble_identity,
            self.group_tag,
            self.status,
            self.policy,
        )
        .await;
    }
}

#[cfg(target_os = "macos")]
async fn run_platform_bluetooth(
    fleet: Fleet,
    ble_identity: BleIdentity,
    group_tag: [u8; 4],
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
) {
    use super::BluetoothAuto;
    use prns_ffi::bluetooth_auto::macos::MacosBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };

    match MacosBleBackend::new(ble_identity, group_tag).await {
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
                group_tag,
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
    group_tag: [u8; 4],
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
) {
    use super::BluetoothAuto;
    use prns_ffi::bluetooth_auto::macos::MacosBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };

    match MacosBleBackend::new(ble_identity, group_tag).await {
        Ok(backend) => {
            let psm = backend.psm();
            let bluetooth = BluetoothAuto::<_, { MacosBleBackend::MAX_PEERS }>::with_status(
                backend,
                ble_identity,
                Endpoint::CoreBluetooth(AppleHost::Ios),
                LinkCapabilities {
                    l2cap: None,
                    link_mtu: BLE_HW_MTU as u16,
                },
                group_tag,
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
    group_tag: [u8; 4],
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
) {
    use super::BluetoothAuto;
    use prns_ffi::bluetooth_auto::windows::WindowsBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::{
        Endpoint, LinkCapabilities, WinRtHost, BLE_HW_MTU,
    };

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
                group_tag,
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
    group_tag: [u8; 4],
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
) {
    use super::{BluerBackend, BluetoothAuto};
    use prns_runtime::interfaces::bluetooth_auto::{
        BlueZHost, Endpoint, LinkCapabilities, Psm, BLE_HW_MTU,
    };

    const CONTROL_PSM: u16 = 0x0083;

    let Some(psm) = Psm::new(CONTROL_PSM) else {
        status.mark_failed(Some("invalid Linux control PSM"));
        crate::diagnostic_log::warn!(
            "bluetooth disabled: invalid Linux control PSM {CONTROL_PSM:#x}"
        );
        return std::future::pending().await;
    };
    match BluerBackend::open(psm, ble_identity, group_tag).await {
        Ok(backend) => {
            let bluetooth = BluetoothAuto::<_, { BluerBackend::MAX_PEERS }>::with_status(
                backend,
                ble_identity,
                Endpoint::BlueZ(BlueZHost::Linux),
                LinkCapabilities {
                    l2cap: Some(psm),
                    link_mtu: BLE_HW_MTU as u16,
                },
                group_tag,
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
    _group_tag: [u8; 4],
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
    use prns_runtime::runtime::{
        ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
    };
    use prns_runtime::storage::GrowableHeap;

    #[test]
    fn auto_ble_registers_before_platform_backend_initialization() {
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: prns_runtime::request_endpoints![],
            remote_control: prns_runtime::remote_control::RemoteControlService::Unavailable,
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: |_event, _state: &()| {},
        });
        let attached = node
            .handle()
            .attach(AutoBle::new(BleIdentity::new([0x31; 16])));

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
