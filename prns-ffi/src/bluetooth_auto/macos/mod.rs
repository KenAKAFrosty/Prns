mod backend;
mod central;
mod data_plane;
mod discovery;
mod gatt_link;
mod gatt_write;
mod peripheral;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_bluetooth::{
    CBAdvertisementDataLocalNameKey, CBAdvertisementDataServiceUUIDsKey, CBCentralManager,
    CBCentralManagerScanOptionAllowDuplicatesKey, CBCharacteristic, CBPeer, CBPeripheral,
    CBPeripheralManager, CBUUID,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString};

use prns_core::interfaces::bluetooth_auto::{
    BleAddress, BleUuid, BLE_SERVICE_UUID, COLUMBA_IDENTITY_UUID, COLUMBA_RX_UUID, COLUMBA_TX_UUID,
    NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};

use central::CentralDelegate;
use gatt_link::GattLink;
use peripheral::PeripheralDelegate;

pub use backend::{MacosBleBackend, PreparedMacosBleBackend};
pub use gatt_link::{GattSink, GattSource};

type PeripheralTable = Arc<Mutex<HashMap<CoreBluetoothPeerId, (SendPeripheral, Option<i8>)>>>;
type RestoredPeripherals = Arc<Mutex<VecDeque<CoreBluetoothPeerId>>>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CoreBluetoothPeerId([u8; 16]);

impl CoreBluetoothPeerId {
    fn address(self) -> BleAddress {
        let mut bytes = [0u8; 6];
        bytes.copy_from_slice(&self.0[..6]);
        BleAddress::new(bytes)
    }
}

#[cfg(target_os = "ios")]
const CENTRAL_RESTORE_IDENTIFIER: &str = "com.personal.prns.ble.central";
#[cfg(target_os = "ios")]
const PERIPHERAL_RESTORE_IDENTIFIER: &str = "com.personal.prns.ble.peripheral";

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
fn central_manager_options() -> Retained<NSDictionary<NSString, AnyObject>> {
    use objc2_core_bluetooth::CBCentralManagerOptionRestoreIdentifierKey;
    // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
    let key: &NSString = unsafe { CBCentralManagerOptionRestoreIdentifierKey };
    let value = NSString::from_str(CENTRAL_RESTORE_IDENTIFIER);
    let value_ref: &NSString = &value;
    let value_obj: &AnyObject = value_ref;
    NSDictionary::from_slices(&[key], &[value_obj])
}

#[cfg(target_os = "ios")]
fn peripheral_manager_options() -> Retained<NSDictionary<NSString, AnyObject>> {
    use objc2_core_bluetooth::CBPeripheralManagerOptionRestoreIdentifierKey;
    // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
    let key: &NSString = unsafe { CBPeripheralManagerOptionRestoreIdentifierKey };
    let value = NSString::from_str(PERIPHERAL_RESTORE_IDENTIFIER);
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

enum Event {
    CentralPowered,
    GattServicePublished,
    GattServicePublishFailed,
    L2capPublished {
        psm: u16,
    },
    L2capPublishFailed,
    Sighting {
        address: BleAddress,
        rssi: Option<i8>,
    },
    Inbound(GattLink),
}

#[derive(Debug)]
pub enum MacosBleError {
    PowerOnTimeout,
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
