#![allow(clippy::undocumented_unsafe_blocks)]

mod backend;
mod central;
mod data_plane;
mod gatt_link;
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
    CBCentralManagerScanOptionAllowDuplicatesKey, CBCharacteristic, CBMutableCharacteristic,
    CBPeripheral, CBPeripheralManager, CBUUID,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString, NSUUID};

use prns_core::interfaces::bluetooth_auto::{
    BleAddress, BleUuid, BLE_SERVICE_UUID, COLUMBA_IDENTITY_UUID, COLUMBA_RX_UUID, COLUMBA_TX_UUID,
    NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};

use central::CentralDelegate;
use gatt_link::GattLink;
use peripheral::PeripheralDelegate;

pub use backend::MacosBleBackend;
pub use gatt_link::{GattSink, GattSource};

type PeripheralTable = Arc<Mutex<HashMap<[u8; 6], (SendPeripheral, Option<i8>)>>>;
type RestoredPeripherals = Arc<Mutex<VecDeque<[u8; 6]>>>;

fn cbuuid(uuid: BleUuid) -> Retained<CBUUID> {
    match uuid {
        BleUuid::Bit128(bytes) => unsafe { CBUUID::UUIDWithData(&NSData::with_bytes(&bytes)) },
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
    unsafe { a.data() }.to_vec() == unsafe { b.data() }.to_vec()
}

fn advertisement_data(services: &NSArray<CBUUID>) -> Retained<NSDictionary<NSString, AnyObject>> {
    let uuids_key: &NSString = unsafe { CBAdvertisementDataServiceUUIDsKey };
    let uuids_value: &AnyObject = services;
    let name_key: &NSString = unsafe { CBAdvertisementDataLocalNameKey };
    let name = NSString::from_str("Prns");
    let name_ref: &NSString = &name;
    let name_value: &AnyObject = name_ref;
    NSDictionary::from_slices(&[uuids_key, name_key], &[uuids_value, name_value])
}

fn scan_options() -> Retained<NSDictionary<NSString, AnyObject>> {
    let duplicates_key: &NSString = unsafe { CBCentralManagerScanOptionAllowDuplicatesKey };
    let duplicates = NSNumber::new_bool(true);
    let duplicates_value: &AnyObject = &duplicates;
    NSDictionary::from_slices(&[duplicates_key], &[duplicates_value])
}

#[cfg(target_os = "ios")]
fn central_manager_options() -> Retained<NSDictionary<NSString, AnyObject>> {
    use objc2_core_bluetooth::CBCentralManagerOptionRestoreIdentifierKey;
    let key: &NSString = unsafe { CBCentralManagerOptionRestoreIdentifierKey };
    let value = NSString::from_str(CENTRAL_RESTORE_IDENTIFIER);
    let value_ref: &NSString = &value;
    let value_obj: &AnyObject = value_ref;
    NSDictionary::from_slices(&[key], &[value_obj])
}

#[cfg(target_os = "ios")]
fn peripheral_manager_options() -> Retained<NSDictionary<NSString, AnyObject>> {
    use objc2_core_bluetooth::CBPeripheralManagerOptionRestoreIdentifierKey;
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
    unsafe { central.scanForPeripheralsWithServices_options(Some(&services), Some(&options)) };
}

fn uuid_token(uuid: &NSUUID) -> [u8; 6] {
    let mut raw = [0u8; 16];
    unsafe {
        let _: () = msg_send![uuid, getUUIDBytes: raw.as_mut_ptr()];
    }
    let mut token = [0u8; 6];
    token.copy_from_slice(&raw[..6]);
    token
}

struct SendPeripheralManager(Retained<CBPeripheralManager>);
unsafe impl Send for SendPeripheralManager {}

struct SendCharacteristic(Retained<CBMutableCharacteristic>);
unsafe impl Send for SendCharacteristic {}

struct SendPeripheral(Retained<CBPeripheral>);
unsafe impl Send for SendPeripheral {}

struct SendCharacteristicRef(Retained<CBCharacteristic>);
unsafe impl Send for SendCharacteristicRef {}

struct SendCentralManager(Retained<CBCentralManager>);
unsafe impl Send for SendCentralManager {}

struct SendCentralDelegate(Retained<CentralDelegate>);
unsafe impl Send for SendCentralDelegate {}

struct SendPeripheralDelegate(Retained<PeripheralDelegate>);
unsafe impl Send for SendPeripheralDelegate {}

enum Event {
    Powered,
    Published {
        psm: u16,
    },
    PublishFailed,
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
}
