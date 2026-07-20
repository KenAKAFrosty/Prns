use std::fmt;
use std::future::Future;
use std::io;
use std::time::Duration;

use btleplug::api::{
    Central, CentralEvent, CharPropFlags, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures_util::StreamExt;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use prns_config::RNodeBleAddress;
use prns_config::RNodeBleTarget;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::{self, MissedTickBehavior};
use uuid::Uuid;

const NUS_SERVICE_UUID: Uuid = Uuid::from_u128(0x6e40_0001_b5a3_f393_e0a9_e50e_24dc_ca9e);
const NUS_WRITE_UUID: Uuid = Uuid::from_u128(0x6e40_0002_b5a3_f393_e0a9_e50e_24dc_ca9e);
const NUS_NOTIFY_UUID: Uuid = Uuid::from_u128(0x6e40_0003_b5a3_f393_e0a9_e50e_24dc_ca9e);
const SCAN_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECTION_POLL_INTERVAL: Duration = Duration::from_millis(250);
const ATT_WRITE_OVERHEAD: u16 = 3;
const MINIMUM_WRITE_WITHOUT_RESPONSE: usize = 20;
const DRIVER_BUFFER_CAPACITY: usize = 32_768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RNodeBleOperation {
    InitializeManager,
    EnumerateAdapters,
    ReadAdapterEvents,
    StartScan,
    StopScan,
    ReadPeripheral,
    ReadPeripheralProperties,
    Connect,
    DiscoverServices,
    Subscribe,
    OpenNotifications,
    Disconnect,
    ReadConnectionState,
    Write,
}

impl fmt::Display for RNodeBleOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operation = match self {
            Self::InitializeManager => "initialize the Bluetooth manager",
            Self::EnumerateAdapters => "enumerate Bluetooth adapters",
            Self::ReadAdapterEvents => "subscribe to Bluetooth adapter events",
            Self::StartScan => "start the RNode BLE scan",
            Self::StopScan => "stop the RNode BLE scan",
            Self::ReadPeripheral => "read a discovered BLE peripheral",
            Self::ReadPeripheralProperties => "read BLE advertisement properties",
            Self::Connect => "connect to the RNode",
            Self::DiscoverServices => "discover the RNode GATT service",
            Self::Subscribe => "subscribe to RNode notifications",
            Self::OpenNotifications => "open the RNode notification stream",
            Self::Disconnect => "disconnect from the RNode",
            Self::ReadConnectionState => "read the RNode connection state",
            Self::Write => "write to the RNode",
        };
        formatter.write_str(operation)
    }
}

#[derive(Debug)]
enum RNodeBleError {
    Backend {
        operation: RNodeBleOperation,
        source: btleplug::Error,
    },
    OperationTimedOut {
        operation: RNodeBleOperation,
        deadline: Duration,
    },
    BluetoothPermissionDenied(RNodeBleOperation),
    NoAdapter,
    DeviceUnavailable(RNodeBleOperation),
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    AddressTargetUnsupportedOnApple(RNodeBleAddress),
    TargetNotFound(RNodeBleTarget),
    TargetNotBonded(RNodeBleTarget),
    MissingWriteCharacteristic,
    MissingNotifyCharacteristic,
    Disconnected,
    Driver(io::Error),
    #[cfg(target_os = "linux")]
    BondStatus(bluer::Error),
}

impl fmt::Display for RNodeBleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend { operation, source } => {
                write!(formatter, "could not {operation}: {source}")
            }
            Self::OperationTimedOut {
                operation,
                deadline,
            } => write!(
                formatter,
                "could not {operation} within {} seconds; verify Bluetooth is powered on and grant the running application Bluetooth access in the operating system privacy settings",
                deadline.as_secs()
            ),
            Self::BluetoothPermissionDenied(operation) => write!(
                formatter,
                "permission was denied while trying to {operation}; grant the running application Bluetooth access in the operating system privacy settings, then restart it"
            ),
            Self::NoAdapter => formatter.write_str(
                "no Bluetooth adapter is available; enable Bluetooth or attach an adapter",
            ),
            Self::DeviceUnavailable(operation) => write!(
                formatter,
                "the RNode disappeared while trying to {operation}; verify it is powered on, paired, and within Bluetooth range"
            ),
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Self::AddressTargetUnsupportedOnApple(address) => write!(
                formatter,
                "cannot select RNode {address} by MAC address on Apple platforms; use `port = ble://<exact device name>` or `port = ble://`"
            ),
            Self::TargetNotFound(target) => write!(
                formatter,
                "could not find {target} advertising the Nordic UART service within 2 seconds"
            ),
            Self::TargetNotBonded(target) => write!(
                formatter,
                "found {target}, but it is not bonded; pair it in the operating system Bluetooth settings before starting prnsd"
            ),
            Self::MissingWriteCharacteristic => formatter.write_str(
                "the selected device advertises Nordic UART but has no writable RX characteristic",
            ),
            Self::MissingNotifyCharacteristic => formatter.write_str(
                "the selected device advertises Nordic UART but has no notifying TX characteristic",
            ),
            Self::Disconnected => formatter.write_str("the RNode BLE connection closed"),
            Self::Driver(source) => write!(formatter, "the local RNode byte stream failed: {source}"),
            #[cfg(target_os = "linux")]
            Self::BondStatus(source) => {
                write!(formatter, "could not verify the RNode bond in BlueZ: {source}")
            }
        }
    }
}

impl std::error::Error for RNodeBleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend { source, .. } => Some(source),
            Self::Driver(source) => Some(source),
            #[cfg(target_os = "linux")]
            Self::BondStatus(source) => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BondStatus {
    Eligible,
    Missing,
}

#[derive(Debug)]
struct MatchingPeripheral {
    peripheral: Peripheral,
    bond_status: BondStatus,
}

pub(super) async fn open(target: &RNodeBleTarget) -> io::Result<DuplexStream> {
    open_inner(target).await.map_err(io::Error::other)
}

async fn open_inner(target: &RNodeBleTarget) -> Result<DuplexStream, RNodeBleError> {
    reject_unsupported_target(target)?;
    let manager = with_backend_timeout(
        RNodeBleOperation::InitializeManager,
        CONNECT_TIMEOUT,
        Manager::new(),
    )
    .await?;
    let adapter = with_backend_timeout(
        RNodeBleOperation::EnumerateAdapters,
        CONNECT_TIMEOUT,
        manager.adapters(),
    )
    .await?
    .into_iter()
    .next()
    .ok_or(RNodeBleError::NoAdapter)?;
    let matching = scan(&adapter, target).await?;
    if matching.bond_status == BondStatus::Missing {
        return Err(RNodeBleError::TargetNotBonded(target.clone()));
    }
    connect(matching.peripheral).await
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn reject_unsupported_target(target: &RNodeBleTarget) -> Result<(), RNodeBleError> {
    match target {
        RNodeBleTarget::Address(address) => Err(RNodeBleError::AddressTargetUnsupportedOnApple(
            address.clone(),
        )),
        RNodeBleTarget::FirstBondedRnode | RNodeBleTarget::Name(_) => Ok(()),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn reject_unsupported_target(_target: &RNodeBleTarget) -> Result<(), RNodeBleError> {
    Ok(())
}

async fn scan(
    adapter: &Adapter,
    target: &RNodeBleTarget,
) -> Result<MatchingPeripheral, RNodeBleError> {
    let mut events = with_backend_timeout(
        RNodeBleOperation::ReadAdapterEvents,
        CONNECT_TIMEOUT,
        adapter.events(),
    )
    .await?;
    with_backend_timeout(
        RNodeBleOperation::StartScan,
        CONNECT_TIMEOUT,
        adapter.start_scan(ScanFilter {
            services: vec![NUS_SERVICE_UUID],
        }),
    )
    .await?;

    let result = time::timeout(SCAN_TIMEOUT, async {
        if let Some(matching) = matching_peripheral(adapter, target).await? {
            return Ok(matching);
        }
        while let Some(event) = events.next().await {
            if !is_candidate_event(&event) {
                continue;
            }
            if let Some(matching) = matching_peripheral(adapter, target).await? {
                return Ok(matching);
            }
        }
        Err(RNodeBleError::TargetNotFound(target.clone()))
    })
    .await;
    if let Err(error) = with_backend_timeout(
        RNodeBleOperation::StopScan,
        SCAN_TIMEOUT,
        adapter.stop_scan(),
    )
    .await
    {
        crate::diagnostic_log::warn!("RNode BLE scan could not stop cleanly: {error}");
    }
    match result {
        Ok(result) => result,
        Err(_) => Err(RNodeBleError::TargetNotFound(target.clone())),
    }
}

fn is_candidate_event(event: &CentralEvent) -> bool {
    matches!(
        event,
        CentralEvent::DeviceDiscovered(_)
            | CentralEvent::DeviceUpdated(_)
            | CentralEvent::ServicesAdvertisement { .. }
    )
}

async fn matching_peripheral(
    adapter: &Adapter,
    target: &RNodeBleTarget,
) -> Result<Option<MatchingPeripheral>, RNodeBleError> {
    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|source| backend(RNodeBleOperation::ReadPeripheral, source))?;
    let mut unbonded_match = None;
    for peripheral in peripherals {
        let Some(properties) = peripheral
            .properties()
            .await
            .map_err(|source| backend(RNodeBleOperation::ReadPeripheralProperties, source))?
        else {
            continue;
        };
        if !properties.services.contains(&NUS_SERVICE_UUID) {
            continue;
        }
        let name = properties
            .local_name
            .as_deref()
            .or(properties.advertisement_name.as_deref());
        if !target_matches(target, peripheral.address().into_inner(), name) {
            continue;
        }
        let bond_status = bond_status(peripheral.address().into_inner()).await?;
        let matching = MatchingPeripheral {
            peripheral,
            bond_status,
        };
        if bond_status == BondStatus::Eligible {
            return Ok(Some(matching));
        }
        if !matches!(target, RNodeBleTarget::FirstBondedRnode) {
            unbonded_match = Some(matching);
        }
    }
    Ok(unbonded_match)
}

fn target_matches(target: &RNodeBleTarget, address: [u8; 6], name: Option<&str>) -> bool {
    match target {
        RNodeBleTarget::FirstBondedRnode => name.is_some_and(|name| name.starts_with("RNode ")),
        RNodeBleTarget::Address(target) => target.octets() == address,
        RNodeBleTarget::Name(target) => name.is_some_and(|name| name == target.as_str()),
    }
}

#[cfg(target_os = "linux")]
async fn bond_status(address: [u8; 6]) -> Result<BondStatus, RNodeBleError> {
    let session = bluer::Session::new()
        .await
        .map_err(RNodeBleError::BondStatus)?;
    for adapter_name in session
        .adapter_names()
        .await
        .map_err(RNodeBleError::BondStatus)?
    {
        let adapter = session
            .adapter(&adapter_name)
            .map_err(RNodeBleError::BondStatus)?;
        let device = adapter
            .device(bluer::Address(address))
            .map_err(RNodeBleError::BondStatus)?;
        if let Ok(paired) = device.is_paired().await {
            return Ok(if paired {
                BondStatus::Eligible
            } else {
                BondStatus::Missing
            });
        }
    }
    Ok(BondStatus::Missing)
}

#[cfg(not(target_os = "linux"))]
async fn bond_status(_address: [u8; 6]) -> Result<BondStatus, RNodeBleError> {
    Ok(BondStatus::Eligible)
}

async fn connect(peripheral: Peripheral) -> Result<DuplexStream, RNodeBleError> {
    with_backend_timeout(
        RNodeBleOperation::Connect,
        CONNECT_TIMEOUT,
        peripheral.connect_with_timeout(CONNECT_TIMEOUT),
    )
    .await?;
    let prepared = prepare_connection(peripheral.clone()).await;
    if prepared.is_err() {
        disconnect(&peripheral).await;
    }
    prepared
}

async fn prepare_connection(peripheral: Peripheral) -> Result<DuplexStream, RNodeBleError> {
    with_backend_timeout(
        RNodeBleOperation::DiscoverServices,
        CONNECT_TIMEOUT,
        peripheral.discover_services_with_timeout(CONNECT_TIMEOUT),
    )
    .await?;
    let write = peripheral
        .characteristics()
        .into_iter()
        .find(|characteristic| {
            characteristic.service_uuid == NUS_SERVICE_UUID
                && characteristic.uuid == NUS_WRITE_UUID
                && characteristic
                    .properties
                    .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
        })
        .ok_or(RNodeBleError::MissingWriteCharacteristic)?;
    let notify = peripheral
        .characteristics()
        .into_iter()
        .find(|characteristic| {
            characteristic.service_uuid == NUS_SERVICE_UUID
                && characteristic.uuid == NUS_NOTIFY_UUID
                && characteristic.properties.contains(CharPropFlags::NOTIFY)
        })
        .ok_or(RNodeBleError::MissingNotifyCharacteristic)?;
    with_backend_timeout(
        RNodeBleOperation::Subscribe,
        CONNECT_TIMEOUT,
        peripheral.subscribe(&notify),
    )
    .await?;
    let notifications = with_backend_timeout(
        RNodeBleOperation::OpenNotifications,
        CONNECT_TIMEOUT,
        peripheral.notifications(),
    )
    .await?;
    let write_capacity = write_capacity(peripheral.mtu());
    let (driver, bridge) = tokio::io::duplex(DRIVER_BUFFER_CAPACITY);
    tokio::spawn(async move {
        let result = drive(
            peripheral.clone(),
            write,
            notifications,
            bridge,
            write_capacity,
        )
        .await;
        disconnect(&peripheral).await;
        if let Err(error) = result {
            crate::diagnostic_log::warn!("RNode BLE byte stream closed: {error}");
        }
    });
    Ok(driver)
}

async fn disconnect(peripheral: &Peripheral) {
    let result = with_backend_timeout(
        RNodeBleOperation::Disconnect,
        CONNECT_TIMEOUT,
        peripheral.disconnect(),
    )
    .await;
    match result {
        Ok(()) | Err(RNodeBleError::DeviceUnavailable(RNodeBleOperation::Disconnect)) => {}
        Err(error) => {
            crate::diagnostic_log::warn!("RNode BLE disconnect did not complete cleanly: {error}");
        }
    }
}

async fn drive(
    peripheral: Peripheral,
    write: btleplug::api::Characteristic,
    mut notifications: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = btleplug::api::ValueNotification> + Send>,
    >,
    mut bridge: DuplexStream,
    write_capacity: usize,
) -> Result<(), RNodeBleError> {
    let mut outbound = vec![0; write_capacity];
    let mut connection_poll = time::interval(CONNECTION_POLL_INTERVAL);
    connection_poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            read = bridge.read(&mut outbound) => {
                let read = read.map_err(RNodeBleError::Driver)?;
                if read == 0 {
                    return Ok(());
                }
                with_backend_timeout(
                    RNodeBleOperation::Write,
                    CONNECT_TIMEOUT,
                    peripheral.write(&write, &outbound[..read], WriteType::WithoutResponse),
                )
                .await?;
            }
            notification = notifications.next() => {
                let Some(notification) = notification else {
                    return Err(RNodeBleError::Disconnected);
                };
                if notification.service_uuid == NUS_SERVICE_UUID
                    && notification.uuid == NUS_NOTIFY_UUID
                {
                    bridge
                        .write_all(&notification.value)
                        .await
                        .map_err(RNodeBleError::Driver)?;
                }
            }
            _ = connection_poll.tick() => {
                let connected = with_backend_timeout(
                    RNodeBleOperation::ReadConnectionState,
                    CONNECT_TIMEOUT,
                    peripheral.is_connected(),
                )
                .await?;
                if !connected {
                    return Err(RNodeBleError::Disconnected);
                }
            }
        }
    }
}

fn write_capacity(mtu: u16) -> usize {
    usize::from(mtu.saturating_sub(ATT_WRITE_OVERHEAD)).max(MINIMUM_WRITE_WITHOUT_RESPONSE)
}

fn backend(operation: RNodeBleOperation, source: btleplug::Error) -> RNodeBleError {
    match source {
        btleplug::Error::PermissionDenied => RNodeBleError::BluetoothPermissionDenied(operation),
        btleplug::Error::NoAdapterAvailable => RNodeBleError::NoAdapter,
        btleplug::Error::DeviceNotFound | btleplug::Error::NotConnected => {
            RNodeBleError::DeviceUnavailable(operation)
        }
        btleplug::Error::TimedOut(deadline) => RNodeBleError::OperationTimedOut {
            operation,
            deadline,
        },
        source => RNodeBleError::Backend { operation, source },
    }
}

async fn with_backend_timeout<T, F>(
    operation: RNodeBleOperation,
    deadline: Duration,
    future: F,
) -> Result<T, RNodeBleError>
where
    F: Future<Output = Result<T, btleplug::Error>>,
{
    match time::timeout(deadline, future).await {
        Ok(result) => result.map_err(|source| backend(operation, source)),
        Err(_) => Err(RNodeBleError::OperationTimedOut {
            operation,
            deadline,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_config::{parse_and_plan, PlannedMedium, RNodeTransportPlan};

    fn planned_target(port: &str) -> RNodeBleTarget {
        let config = format!(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = {port}\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
             codingrate = 5\n"
        );
        let plan = parse_and_plan(&config)
            .expect("RNode BLE config plans")
            .value;
        let PlannedMedium::Rnode { transport, .. } = &plan.interfaces[0].medium else {
            panic!("RNode transport expected")
        };
        let RNodeTransportPlan::Ble(target) = transport else {
            panic!("RNode BLE transport expected")
        };
        target.clone()
    }

    #[test]
    fn target_matching_keeps_stock_auto_name_and_address_semantics() {
        let address = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        assert!(target_matches(
            &planned_target("ble://"),
            address,
            Some("RNode 1234")
        ));
        assert!(!target_matches(
            &planned_target("ble://"),
            address,
            Some("Other RNode")
        ));
        assert!(target_matches(
            &planned_target("ble://RNode 1234"),
            address,
            Some("RNode 1234")
        ));
        assert!(target_matches(
            &planned_target("ble://AA:BB:CC:DD:EE:FF"),
            address,
            None
        ));
    }

    #[test]
    fn negotiated_mtu_selects_the_write_without_response_payload() {
        assert_eq!(write_capacity(23), 20);
        assert_eq!(write_capacity(185), 182);
        assert_eq!(write_capacity(0), 20);
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_platform_operations_become_actionable_failures() {
        let deadline = Duration::from_secs(5);
        let error = with_backend_timeout(
            RNodeBleOperation::EnumerateAdapters,
            deadline,
            std::future::pending::<Result<(), btleplug::Error>>(),
        )
        .await
        .expect_err("stalled adapter enumeration must time out");
        assert!(matches!(
            error,
            RNodeBleError::OperationTimedOut {
                operation: RNodeBleOperation::EnumerateAdapters,
                deadline: actual,
            } if actual == deadline
        ));
        let rendered = error.to_string();
        assert!(rendered.contains("grant the running application Bluetooth access"));
        assert!(rendered.contains("within 5 seconds"));
    }

    #[test]
    fn denied_bluetooth_access_names_the_operator_repair() {
        let error = backend(
            RNodeBleOperation::EnumerateAdapters,
            btleplug::Error::PermissionDenied,
        );
        let rendered = error.to_string();
        assert!(rendered.contains("operating system privacy settings"));
        assert!(rendered.contains("restart it"));
    }

    #[test]
    fn platform_failures_keep_adapter_device_and_timeout_repairs() {
        assert!(matches!(
            backend(
                RNodeBleOperation::EnumerateAdapters,
                btleplug::Error::NoAdapterAvailable
            ),
            RNodeBleError::NoAdapter
        ));
        let disappeared =
            backend(RNodeBleOperation::Write, btleplug::Error::NotConnected).to_string();
        assert!(disappeared.contains("powered on, paired, and within Bluetooth range"));
        let timed_out = backend(
            RNodeBleOperation::Connect,
            btleplug::Error::TimedOut(Duration::from_secs(5)),
        )
        .to_string();
        assert!(timed_out.contains("within 5 seconds"));
    }
}
