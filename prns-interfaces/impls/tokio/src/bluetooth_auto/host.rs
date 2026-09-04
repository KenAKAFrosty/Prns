use prns_runtime::interfaces::bluetooth_auto::{self as contract, BleIdentity};
use prns_runtime::interfaces::IfacContext;
use prns_runtime::interfaces::{
    ConfiguredInterfacePolicy, EffectiveInterfacePolicy, InterfaceId, InterfaceKind,
    InterfaceStatus, ReportsStatus,
};
use prns_runtime::runtime::{Attachable, Fleet, InterfaceSupervisor, PrnsNodeHandle};

use super::BluetoothAutoStatus;

#[cfg(target_os = "ios")]
pub use prns_ffi::bluetooth_auto::macos::{
    CoreBluetoothCentralRestorationIdentifier, CoreBluetoothRestorationIdentifiers,
    CoreBluetoothRestorationIdentifiersError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoBle {
    identity: BleIdentity,
}

/// Canonical name for the Bluetooth LE auto-interface constructor.
pub type AutoBluetoothLe = AutoBle;

pub struct ConfiguredAutoBle {
    identity: BleIdentity,
    policy: EffectiveInterfacePolicy,
}

/// Canonical name for a configured Bluetooth LE auto-interface.
pub type ConfiguredAutoBluetoothLe = ConfiguredAutoBle;

impl AutoBle {
    #[must_use]
    pub const fn new(identity: BleIdentity) -> Self {
        Self { identity }
    }

    #[must_use]
    pub fn with_policy(
        identity: BleIdentity,
        policy: EffectiveInterfacePolicy,
    ) -> ConfiguredAutoBle {
        ConfiguredAutoBle { identity, policy }
    }

    /// Creates the restoration-aware CoreBluetooth managers immediately while leaving radio
    /// authorization and service readiness to the attached asynchronous supervisor.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub async fn prepare(
        identity: BleIdentity,
    ) -> Result<PreparedAutoBle, prns_ffi::bluetooth_auto::macos::MacosBleError> {
        let manager_preparation = AppleManagerPreparation::LegacyRestorationAware;
        let backend = manager_preparation.prepare(identity).await?;
        Ok(PreparedAutoBle::new(
            identity,
            manager_preparation,
            Some(backend),
        ))
    }

    /// Creates restoration-aware CoreBluetooth managers using identifiers supplied by the
    /// application that owns their lifecycle.
    ///
    /// The selected identifiers are retained for every later readiness retry.
    #[cfg(target_os = "ios")]
    pub async fn prepare_with_restoration(
        identity: BleIdentity,
        identifiers: CoreBluetoothRestorationIdentifiers,
    ) -> Result<PreparedAutoBle, prns_ffi::bluetooth_auto::macos::MacosBleError> {
        let manager_preparation = AppleManagerPreparation::RestorationAware(identifiers);
        let backend = manager_preparation.prepare(identity).await?;
        Ok(PreparedAutoBle::new(
            identity,
            manager_preparation,
            Some(backend),
        ))
    }

    /// Creates only the restoration-aware CoreBluetooth central manager.
    ///
    /// The selected identifier is retained for every later readiness retry. The containing
    /// application must declare the `bluetooth-central` background mode and recreate the manager
    /// with this exact identifier on an iOS restoration launch. No peripheral manager is created.
    #[cfg(target_os = "ios")]
    pub async fn prepare_central_only_with_restoration(
        identity: BleIdentity,
        identifier: CoreBluetoothCentralRestorationIdentifier,
    ) -> Result<PreparedAutoBle, prns_ffi::bluetooth_auto::macos::MacosBleError> {
        let manager_preparation = AppleManagerPreparation::CentralOnlyRestorationAware(identifier);
        let backend = manager_preparation.prepare(identity).await?;
        Ok(PreparedAutoBle::new(
            identity,
            manager_preparation,
            Some(backend),
        ))
    }

    /// Creates CoreBluetooth managers without state restoration while leaving radio authorization
    /// and service readiness to the attached asynchronous supervisor.
    ///
    /// This path does not opt into CoreBluetooth state restoration. The selected preparation mode
    /// is retained for every later readiness retry.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub async fn prepare_without_restoration(
        identity: BleIdentity,
    ) -> Result<PreparedAutoBle, prns_ffi::bluetooth_auto::macos::MacosBleError> {
        let manager_preparation = AppleManagerPreparation::WithoutRestoration;
        let backend = manager_preparation.prepare(identity).await?;
        Ok(PreparedAutoBle::new(
            identity,
            manager_preparation,
            Some(backend),
        ))
    }

    /// Creates only a CoreBluetooth central manager without state restoration.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub async fn prepare_central_only_without_restoration(
        identity: BleIdentity,
    ) -> Result<PreparedAutoBle, prns_ffi::bluetooth_auto::macos::MacosBleError> {
        let manager_preparation = AppleManagerPreparation::CentralOnlyWithoutRestoration;
        let backend = manager_preparation.prepare(identity).await?;
        Ok(PreparedAutoBle::new(
            identity,
            manager_preparation,
            Some(backend),
        ))
    }

    /// Produces a failed-but-supervised Bluetooth LE attachment when native manager preparation itself
    /// cannot be started. Retries retain the restoration-aware behavior of [`Self::prepare`]. The
    /// core node and every other transport remain available.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub fn unavailable(identity: BleIdentity) -> PreparedAutoBle {
        PreparedAutoBle::new(
            identity,
            AppleManagerPreparation::LegacyRestorationAware,
            None,
        )
    }

    /// Produces a failed-but-supervised restoration-aware Bluetooth LE attachment when native
    /// manager preparation itself cannot be started. Retries reuse the caller's identifiers.
    #[cfg(target_os = "ios")]
    pub fn unavailable_with_restoration(
        identity: BleIdentity,
        identifiers: CoreBluetoothRestorationIdentifiers,
    ) -> PreparedAutoBle {
        PreparedAutoBle::new(
            identity,
            AppleManagerPreparation::RestorationAware(identifiers),
            None,
        )
    }

    /// Produces a failed-but-supervised central-only attachment whose retries reuse the caller's
    /// central restoration identifier.
    #[cfg(target_os = "ios")]
    pub fn unavailable_central_only_with_restoration(
        identity: BleIdentity,
        identifier: CoreBluetoothCentralRestorationIdentifier,
    ) -> PreparedAutoBle {
        PreparedAutoBle::new(
            identity,
            AppleManagerPreparation::CentralOnlyRestorationAware(identifier),
            None,
        )
    }

    /// Produces a failed-but-supervised Bluetooth LE attachment without state restoration when
    /// native manager preparation itself cannot be started. The core node and every other
    /// transport remain available, and retries never opt into state restoration.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub fn unavailable_without_restoration(identity: BleIdentity) -> PreparedAutoBle {
        PreparedAutoBle::new(identity, AppleManagerPreparation::WithoutRestoration, None)
    }

    /// Produces a failed-but-supervised central-only attachment whose retries remain outside the
    /// CoreBluetooth restoration contract.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub fn unavailable_central_only_without_restoration(identity: BleIdentity) -> PreparedAutoBle {
        PreparedAutoBle::new(
            identity,
            AppleManagerPreparation::CentralOnlyWithoutRestoration,
            None,
        )
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[derive(Clone, Debug, PartialEq, Eq)]
enum AppleManagerPreparation {
    LegacyRestorationAware,
    #[cfg(target_os = "ios")]
    RestorationAware(CoreBluetoothRestorationIdentifiers),
    #[cfg(target_os = "ios")]
    CentralOnlyRestorationAware(CoreBluetoothCentralRestorationIdentifier),
    WithoutRestoration,
    CentralOnlyWithoutRestoration,
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
enum PreparedAppleBackend {
    DualRole(prns_ffi::bluetooth_auto::macos::PreparedMacosBleBackend),
    CentralOnly(prns_ffi::bluetooth_auto::macos::PreparedCentralOnlyMacosBleBackend),
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
enum ReadyAppleBackend {
    DualRole(prns_ffi::bluetooth_auto::macos::MacosBleBackend),
    CentralOnly(prns_ffi::bluetooth_auto::macos::CentralOnlyMacosBleBackend),
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl AppleManagerPreparation {
    async fn prepare(
        &self,
        identity: BleIdentity,
    ) -> Result<PreparedAppleBackend, prns_ffi::bluetooth_auto::macos::MacosBleError> {
        use prns_ffi::bluetooth_auto::macos::{CentralOnlyMacosBleBackend, MacosBleBackend};

        match self {
            Self::LegacyRestorationAware => MacosBleBackend::prepare(identity)
                .await
                .map(PreparedAppleBackend::DualRole),
            #[cfg(target_os = "ios")]
            Self::RestorationAware(identifiers) => {
                MacosBleBackend::prepare_with_restoration(identity, identifiers.clone())
                    .await
                    .map(PreparedAppleBackend::DualRole)
            }
            #[cfg(target_os = "ios")]
            Self::CentralOnlyRestorationAware(identifier) => {
                CentralOnlyMacosBleBackend::prepare_with_restoration(identity, identifier.clone())
                    .await
                    .map(PreparedAppleBackend::CentralOnly)
            }
            Self::WithoutRestoration => MacosBleBackend::prepare_without_restoration(identity)
                .await
                .map(PreparedAppleBackend::DualRole),
            Self::CentralOnlyWithoutRestoration => {
                CentralOnlyMacosBleBackend::prepare_without_restoration(identity)
                    .await
                    .map(PreparedAppleBackend::CentralOnly)
            }
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
mod apple_manager_preparation_tests {
    use super::*;

    #[test]
    fn unavailable_central_only_preparation_retains_its_role_for_retry() {
        let identity = BleIdentity::new([0; 16]);
        let prepared = AutoBle::unavailable_central_only_without_restoration(identity);
        assert_eq!(
            prepared.manager_preparation,
            AppleManagerPreparation::CentralOnlyWithoutRestoration
        );
    }

    #[cfg(target_os = "ios")]
    #[test]
    fn unavailable_central_only_preparation_retains_its_restoration_identifier() {
        let identity = BleIdentity::new([0; 16]);
        let identifier = CoreBluetoothCentralRestorationIdentifier::new("central-only")
            .expect("the fixture identifier is valid");
        let prepared =
            AutoBle::unavailable_central_only_with_restoration(identity, identifier.clone());
        assert_eq!(
            prepared.manager_preparation,
            AppleManagerPreparation::CentralOnlyRestorationAware(identifier)
        );
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl PreparedAutoBle {
    fn new(
        identity: BleIdentity,
        manager_preparation: AppleManagerPreparation,
        backend: Option<PreparedAppleBackend>,
    ) -> Self {
        Self {
            identity,
            policy: prns_runtime::interfaces::bluetooth_auto::defaults_for_bitrate(
                prns_runtime::interfaces::bluetooth_auto::BLE_BITRATE_GUESS_BPS,
            )
            .configured(ConfiguredInterfacePolicy::default()),
            status: BluetoothAutoStatus::new(),
            manager_preparation,
            backend,
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
    policy: EffectiveInterfacePolicy,
    status: BluetoothAutoStatus,
    manager_preparation: AppleManagerPreparation,
    backend: Option<PreparedAppleBackend>,
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
        attach_platform_bluetooth(handle, self.identity, self.policy, None)
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
            policy: self.policy,
            status: self.status,
            manager_preparation: self.manager_preparation,
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
                policy: self.policy,
                status: self.status,
                manager_preparation: self.manager_preparation,
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
    policy: EffectiveInterfacePolicy,
    status: BluetoothAutoStatus,
    manager_preparation: AppleManagerPreparation,
    backend: Option<PreparedAppleBackend>,
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
        contract::GROUP_ID
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        run_prepared_platform_bluetooth(
            fleet,
            self.identity,
            self.status,
            self.policy,
            self.manager_preparation,
            self.backend,
        )
        .await;
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
async fn run_prepared_platform_bluetooth(
    fleet: Fleet,
    ble_identity: BleIdentity,
    status: BluetoothAutoStatus,
    policy: EffectiveInterfacePolicy,
    manager_preparation: AppleManagerPreparation,
    backend: Option<PreparedAppleBackend>,
) {
    use super::BluetoothAuto;
    use prns_ffi::bluetooth_auto::macos::{CentralOnlyMacosBleBackend, MacosBleBackend};
    use prns_runtime::interfaces::bluetooth_auto::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };

    let mut prepared = backend;
    loop {
        let candidate = match prepared.take() {
            Some(backend) => backend,
            None => match manager_preparation.prepare(ble_identity).await {
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

        let ready = match candidate {
            PreparedAppleBackend::DualRole(backend) => {
                backend.ready().await.map(ReadyAppleBackend::DualRole)
            }
            PreparedAppleBackend::CentralOnly(backend) => {
                backend.ready().await.map(ReadyAppleBackend::CentralOnly)
            }
        };
        match ready {
            Ok(ReadyAppleBackend::DualRole(backend)) => {
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
            Ok(ReadyAppleBackend::CentralOnly(backend)) => {
                status.clear_failure();
                #[cfg(target_os = "macos")]
                let endpoint = Endpoint::CoreBluetooth(AppleHost::MacOs);
                #[cfg(target_os = "ios")]
                let endpoint = Endpoint::CoreBluetooth(AppleHost::Ios);
                let bluetooth =
                    BluetoothAuto::<_, { CentralOnlyMacosBleBackend::MAX_PEERS }>::with_status(
                        backend,
                        ble_identity,
                        endpoint,
                        LinkCapabilities {
                            l2cap: None,
                            link_mtu: BLE_HW_MTU as u16,
                        },
                        status,
                    )
                    .with_policy(policy);
                crate::diagnostic_log::info!(
                    "bluetooth: supervising prepared central-only CoreBluetooth backend; no local peripheral or L2CAP capability"
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
    policy: EffectiveInterfacePolicy,
    ifac: Option<(IfacContext, Option<String>)>,
) -> AttachedBle {
    let status = BluetoothAutoStatus::new();
    let bluetooth = PlatformBluetooth {
        ble_identity,
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
        contract::GROUP_ID
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
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
    use super::BluetoothAuto;
    use prns_ffi::bluetooth_auto::macos::MacosBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };

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
    use super::BluetoothAuto;
    use prns_ffi::bluetooth_auto::macos::MacosBleBackend;
    use prns_runtime::interfaces::bluetooth_auto::{
        AppleHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };

    match MacosBleBackend::new(ble_identity).await {
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
    use prns_runtime::runtime::{
        ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
    };
    use prns_runtime::storage::GrowableHeap;

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn unavailable_preparation_retains_the_selected_retry_mode() {
        let identity = BleIdentity::new([0x30; 16]);
        let without_restoration = AutoBle::unavailable_without_restoration(identity);
        assert_eq!(
            without_restoration.manager_preparation,
            AppleManagerPreparation::WithoutRestoration
        );
        assert!(without_restoration.backend.is_none());

        let restoration_aware = AutoBle::unavailable(identity);
        assert_eq!(
            restoration_aware.manager_preparation,
            AppleManagerPreparation::LegacyRestorationAware
        );
        assert!(restoration_aware.backend.is_none());
    }

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
