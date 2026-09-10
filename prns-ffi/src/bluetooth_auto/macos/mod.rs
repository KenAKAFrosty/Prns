mod backend;
mod central;
mod data_plane;
mod discovery;
mod gatt_link;
mod gatt_write;
mod l2cap_lifecycle;
mod peripheral;

#[cfg(test)]
mod peripheral_tests;
#[cfg(test)]
mod radio_lifecycle_tests;
#[cfg(test)]
mod tests;

#[cfg(any(test, target_os = "ios"))]
use std::fmt;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_bluetooth::{
    CBAdvertisementDataLocalNameKey, CBAdvertisementDataServiceUUIDsKey, CBCentralManager,
    CBCentralManagerScanOptionAllowDuplicatesKey, CBCharacteristic, CBPeer, CBPeripheral,
    CBPeripheralManager, CBUUID,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString};
use tokio::sync::{mpsc as tokio_mpsc, watch};

use prns_core::interfaces::bluetooth_auto::{
    BleAddress, BleUuid, BLE_SERVICE_UUID, COLUMBA_IDENTITY_UUID, COLUMBA_RX_UUID, COLUMBA_TX_UUID,
    NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};

use central::CentralDelegate;
use peripheral::PeripheralDelegate;

pub use backend::{MacosBleBackend, PreparedMacosBleBackend};
pub use gatt_link::{GattSink, GattSource};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CoreBluetoothPeerId([u8; 16]);

impl CoreBluetoothPeerId {
    fn address(self) -> BleAddress {
        let mut bytes = [0u8; 6];
        bytes.copy_from_slice(&self.0[..6]);
        BleAddress::new(bytes)
    }
}

#[cfg(any(test, target_os = "ios"))]
const CENTRAL_RESTORE_IDENTIFIER: &str = "com.personal.prns.ble.central";
#[cfg(any(test, target_os = "ios"))]
const PERIPHERAL_RESTORE_IDENTIFIER: &str = "com.personal.prns.ble.peripheral";

/// Stable, application-owned identifiers for the two CoreBluetooth managers used by Bluetooth
/// Auto.
///
/// Passing these identifiers opts the caller into CoreBluetooth state preservation and
/// restoration. The containing application is responsible for declaring the matching central and
/// peripheral background modes and reinstantiating the managers with the same identifiers when
/// iOS relaunches it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(test, target_os = "ios"))]
pub struct CoreBluetoothRestorationIdentifiers {
    central: String,
    peripheral: String,
}

#[cfg(any(test, target_os = "ios"))]
impl CoreBluetoothRestorationIdentifiers {
    pub fn new(
        central: impl Into<String>,
        peripheral: impl Into<String>,
    ) -> Result<Self, CoreBluetoothRestorationIdentifiersError> {
        let central = central.into();
        if central.is_empty() {
            return Err(CoreBluetoothRestorationIdentifiersError::EmptyCentral);
        }
        let peripheral = peripheral.into();
        if peripheral.is_empty() {
            return Err(CoreBluetoothRestorationIdentifiersError::EmptyPeripheral);
        }
        if central == peripheral {
            return Err(CoreBluetoothRestorationIdentifiersError::Duplicate);
        }
        Ok(Self {
            central,
            peripheral,
        })
    }

    #[must_use]
    pub fn central(&self) -> &str {
        &self.central
    }

    #[must_use]
    pub fn peripheral(&self) -> &str {
        &self.peripheral
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(test, target_os = "ios"))]
pub enum CoreBluetoothRestorationIdentifiersError {
    EmptyCentral,
    EmptyPeripheral,
    Duplicate,
}

#[cfg(any(test, target_os = "ios"))]
impl fmt::Display for CoreBluetoothRestorationIdentifiersError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCentral => formatter
                .write_str("the CoreBluetooth central restoration identifier must not be empty"),
            Self::EmptyPeripheral => formatter
                .write_str("the CoreBluetooth peripheral restoration identifier must not be empty"),
            Self::Duplicate => formatter.write_str(
                "the CoreBluetooth central and peripheral restoration identifiers must differ",
            ),
        }
    }
}

#[cfg(any(test, target_os = "ios"))]
impl std::error::Error for CoreBluetoothRestorationIdentifiersError {}

#[cfg(any(test, target_os = "ios"))]
fn legacy_restoration_identifiers() -> CoreBluetoothRestorationIdentifiers {
    CoreBluetoothRestorationIdentifiers {
        central: CENTRAL_RESTORE_IDENTIFIER.to_owned(),
        peripheral: PERIPHERAL_RESTORE_IDENTIFIER.to_owned(),
    }
}

fn cbuuid(uuid: BleUuid) -> Retained<CBUUID> {
    match uuid {
        // SAFETY: NSData owns the exact UUID bytes for the duration of the initializer, and the
        // generated CoreBluetooth binding returns a retained CBUUID.
        BleUuid::Bit128(bytes) => unsafe { CBUUID::UUIDWithData(&NSData::with_bytes(&bytes)) },
        // SAFETY: NSData owns the exact big-endian UUID bytes for the duration of the initializer,
        // and the generated CoreBluetooth binding returns a retained CBUUID.
        BleUuid::Bit16(short) => unsafe {
            CBUUID::UUIDWithData(&NSData::with_bytes(&short.to_be_bytes()))
        },
    }
}

fn service_uuid() -> Retained<CBUUID> {
    cbuuid(BLE_SERVICE_UUID)
}

fn control_uuid() -> Retained<CBUUID> {
    cbuuid(NATIVE_CONTROL_UUID)
}

fn data_uuid() -> Retained<CBUUID> {
    cbuuid(NATIVE_DATA_UUID)
}

fn columba_rx_uuid() -> Retained<CBUUID> {
    cbuuid(COLUMBA_RX_UUID)
}

fn columba_tx_uuid() -> Retained<CBUUID> {
    cbuuid(COLUMBA_TX_UUID)
}

fn columba_identity_uuid() -> Retained<CBUUID> {
    cbuuid(COLUMBA_IDENTITY_UUID)
}

fn cbuuid_eq(a: &CBUUID, b: &CBUUID) -> bool {
    // SAFETY: both arguments are live retained CBUUID objects and `data` returns retained immutable
    // NSData instances whose bytes are copied immediately.
    let a = unsafe { a.data() }.to_vec();
    // SAFETY: as above, `b` is a live CBUUID and the returned immutable data is copied immediately.
    let b = unsafe { b.data() }.to_vec();
    a == b
}

fn advertisement_data(services: &NSArray<CBUUID>) -> Retained<NSDictionary<NSString, AnyObject>> {
    // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
    let uuids_key: &NSString = unsafe { CBAdvertisementDataServiceUUIDsKey };
    let uuids_value: &AnyObject = services;
    // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
    let name_key: &NSString = unsafe { CBAdvertisementDataLocalNameKey };
    let name = NSString::from_str("Prns");
    let name_ref: &NSString = &name;
    let name_value: &AnyObject = name_ref;
    NSDictionary::from_slices(&[uuids_key, name_key], &[uuids_value, name_value])
}

fn scan_options() -> Retained<NSDictionary<NSString, AnyObject>> {
    // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
    let duplicates_key: &NSString = unsafe { CBCentralManagerScanOptionAllowDuplicatesKey };
    let duplicates = NSNumber::new_bool(true);
    let duplicates_value: &AnyObject = &duplicates;
    NSDictionary::from_slices(&[duplicates_key], &[duplicates_value])
}

#[cfg(target_os = "ios")]
fn central_manager_options(identifier: &str) -> Retained<NSDictionary<NSString, AnyObject>> {
    use objc2_core_bluetooth::CBCentralManagerOptionRestoreIdentifierKey;
    // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
    let key: &NSString = unsafe { CBCentralManagerOptionRestoreIdentifierKey };
    let value = NSString::from_str(identifier);
    let value_ref: &NSString = &value;
    let value_obj: &AnyObject = value_ref;
    NSDictionary::from_slices(&[key], &[value_obj])
}

#[cfg(target_os = "ios")]
fn peripheral_manager_options(identifier: &str) -> Retained<NSDictionary<NSString, AnyObject>> {
    use objc2_core_bluetooth::CBPeripheralManagerOptionRestoreIdentifierKey;
    // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
    let key: &NSString = unsafe { CBPeripheralManagerOptionRestoreIdentifierKey };
    let value = NSString::from_str(identifier);
    let value_ref: &NSString = &value;
    let value_obj: &AnyObject = value_ref;
    NSDictionary::from_slices(&[key], &[value_obj])
}

fn start_scan(central: &CBCentralManager) {
    let uuid = service_uuid();
    let services = NSArray::from_slice(&[&*uuid]);
    let options = scan_options();
    // SAFETY: `central` is a live manager used on its serial queue, and both retained collection
    // arguments remain alive for the synchronous Objective-C message.
    unsafe { central.scanForPeripheralsWithServices_options(Some(&services), Some(&options)) };
}

fn core_bluetooth_peer_id(peer: &CBPeer) -> CoreBluetoothPeerId {
    let mut raw = [0u8; 16];
    // SAFETY: CoreBluetooth supplied a live peer, and NSUUID guarantees `getUUIDBytes:` writes
    // exactly 16 bytes into the live writable buffer.
    unsafe {
        let uuid = peer.identifier();
        let _: () = msg_send![&*uuid, getUUIDBytes: raw.as_mut_ptr()];
    }
    CoreBluetoothPeerId(raw)
}

struct SendPeripheralManager(Retained<CBPeripheralManager>);
// SAFETY: this wrapper is only transferred into jobs on the manager's serial dispatch queue; the
// retained Objective-C object is never concurrently messaged by Prns.
unsafe impl Send for SendPeripheralManager {}

#[derive(Clone)]
struct SendPeripheral(Retained<CBPeripheral>);
// SAFETY: this wrapper is only transferred into jobs on the central manager's serial dispatch
// queue; Prns does not concurrently message the retained peripheral.
unsafe impl Send for SendPeripheral {}

struct SendCharacteristicRef(Retained<CBCharacteristic>);
// SAFETY: this wrapper moves only with its owning peripheral to the serial CoreBluetooth dispatch
// queue, so Prns never concurrently messages the retained characteristic.
unsafe impl Send for SendCharacteristicRef {}

struct SendCentralManager(Retained<CBCentralManager>);
// SAFETY: the retained manager is moved only into work submitted to its own serial dispatch queue,
// and all Prns messages to it are queue-confined.
unsafe impl Send for SendCentralManager {}

struct SendCentralDelegate(Retained<CentralDelegate>);
// SAFETY: the retained delegate's RefCell-backed state is accessed only by the serial CoreBluetooth
// dispatch queue before and after transfer.
unsafe impl Send for SendCentralDelegate {}

struct SendPeripheralDelegate(Retained<PeripheralDelegate>);
// SAFETY: the retained delegate's RefCell-backed state is accessed only by the serial CoreBluetooth
// dispatch queue before and after transfer.
unsafe impl Send for SendPeripheralDelegate {}

impl SendPeripheralDelegate {
    /// Queue-confined: call only from the CoreBluetooth serial dispatch queue.
    fn has_inbound_session(&self, peer_id: CoreBluetoothPeerId) -> bool {
        self.0.has_inbound_session(peer_id)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PublicationState {
    #[default]
    Waiting,
    Published,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum L2capPublicationState {
    #[default]
    Waiting,
    Published(u16),
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ManagerSignals {
    central_powered_generation: u64,
    gatt: PublicationState,
    l2cap: L2capPublicationState,
}

#[derive(Clone)]
struct ManagerSignalSender(watch::Sender<ManagerSignals>);

impl ManagerSignalSender {
    fn central_powered(&self) {
        self.0.send_modify(|signals| {
            signals.central_powered_generation = signals.central_powered_generation.wrapping_add(1);
        });
    }

    fn gatt_service_published(&self) {
        self.0.send_modify(|signals| {
            if signals.gatt != PublicationState::Failed {
                signals.gatt = PublicationState::Published;
            }
        });
    }

    fn gatt_service_publish_failed(&self) {
        self.0
            .send_modify(|signals| signals.gatt = PublicationState::Failed);
    }

    fn l2cap_published(&self, psm: u16) {
        self.0.send_modify(|signals| {
            if signals.l2cap != L2capPublicationState::Failed {
                signals.l2cap = L2capPublicationState::Published(psm);
            }
        });
    }

    fn l2cap_publish_failed(&self) {
        self.0
            .send_modify(|signals| signals.l2cap = L2capPublicationState::Failed);
    }
}

fn manager_signal_channel() -> (ManagerSignalSender, watch::Receiver<ManagerSignals>) {
    let (sender, receiver) = watch::channel(ManagerSignals::default());
    (ManagerSignalSender(sender), receiver)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sighting {
    address: BleAddress,
    rssi: Option<i8>,
}

#[derive(Debug, PartialEq, Eq)]
enum BoundedIngress<T> {
    Accepted,
    Full(T),
    Closed(T),
}

fn try_bounded_ingress<T>(sender: &tokio_mpsc::Sender<T>, value: T) -> BoundedIngress<T> {
    match sender.try_send(value) {
        Ok(()) => BoundedIngress::Accepted,
        Err(tokio_mpsc::error::TrySendError::Full(value)) => BoundedIngress::Full(value),
        Err(tokio_mpsc::error::TrySendError::Closed(value)) => BoundedIngress::Closed(value),
    }
}

#[derive(Debug)]
pub enum MacosBleError {
    PowerOnTimeout,
    RadioTransitionTimeout,
    Closed,
    ControlTooLarge,
    NotifyFailed,
    PublishFailed,
    FrameTooLarge,
    QueueFull,
    DialFailed,
    MissingColumbaIdentity,
    UnsupportedWriteMode,
    InvalidWriteMtu,
    GattWriteFailed,
    GattWriteTimeout,
}
