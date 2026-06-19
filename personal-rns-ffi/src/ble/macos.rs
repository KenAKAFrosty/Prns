#![allow(clippy::undocumented_unsafe_blocks)]

use std::sync::mpsc as sync_mpsc;
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_bluetooth::{
    CBAdvertisementDataServiceUUIDsKey, CBCentralManager, CBCentralManagerDelegate, CBManagerState,
    CBPeripheral, CBPeripheralManager, CBPeripheralManagerDelegate, CBUUID,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use tokio::sync::mpsc as tokio_mpsc;

use personal_rns::interfaces::bluetooth_auto::core::{BleAddress, BLE_SERVICE_UUID_BYTES};

fn service_uuid() -> Retained<CBUUID> {
    let data = NSData::with_bytes(&BLE_SERVICE_UUID_BYTES);
    unsafe { CBUUID::UUIDWithData(&data) }
}

fn advertisement_data(services: &NSArray<CBUUID>) -> Retained<NSDictionary<NSString, AnyObject>> {
    let key: &NSString = unsafe { CBAdvertisementDataServiceUUIDsKey };
    let value: &AnyObject = services;
    NSDictionary::from_slices(&[key], &[value])
}

#[derive(Debug)]
enum Event {
    Powered { on: bool },
    Sighting(BleAddress),
}

struct CentralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = CentralDelegateIvars]
    struct CentralDelegate;

    unsafe impl NSObjectProtocol for CentralDelegate {}

    unsafe impl CBCentralManagerDelegate for CentralDelegate {
        #[unsafe(method(centralManagerDidUpdateState:))]
        fn did_update_state(&self, central: &CBCentralManager) {
            let on = unsafe { central.state() } == CBManagerState::PoweredOn;
            let _ = self.ivars().events.send(Event::Powered { on });
            if on {
                let uuid = service_uuid();
                let services = NSArray::from_slice(&[&*uuid]);
                unsafe { central.scanForPeripheralsWithServices_options(Some(&services), None) };
            }
        }

        #[unsafe(method(centralManager:didDiscoverPeripheral:advertisementData:RSSI:))]
        fn did_discover(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            _advertisement_data: &NSDictionary<NSString, AnyObject>,
            _rssi: &NSNumber,
        ) {
            let token = peripheral_token(peripheral);
            let _ = self
                .ivars()
                .events
                .send(Event::Sighting(BleAddress::new(token)));
        }
    }
);

impl CentralDelegate {
    fn new(events: tokio_mpsc::UnboundedSender<Event>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CentralDelegateIvars { events });
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    struct PeripheralDelegate;

    unsafe impl NSObjectProtocol for PeripheralDelegate {}

    unsafe impl CBPeripheralManagerDelegate for PeripheralDelegate {
        #[unsafe(method(peripheralManagerDidUpdateState:))]
        fn did_update_state(&self, peripheral: &CBPeripheralManager) {
            if unsafe { peripheral.state() } == CBManagerState::PoweredOn {
                let uuid = service_uuid();
                let services = NSArray::from_slice(&[&*uuid]);
                let data = advertisement_data(&services);
                unsafe { peripheral.startAdvertising(Some(&data)) };
            }
        }
    }
);

impl PeripheralDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

fn peripheral_token(peripheral: &CBPeripheral) -> [u8; 6] {
    let identifier = unsafe { peripheral.identifier() };
    let mut raw = [0u8; 16];
    unsafe {
        let _: () = msg_send![&*identifier, getUUIDBytes: raw.as_mut_ptr()];
    }
    let mut token = [0u8; 6];
    token.copy_from_slice(&raw[..6]);
    token
}

pub struct MacosBleBackend {
    _keepalive: sync_mpsc::Sender<()>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
}

#[derive(Debug)]
pub enum MacosBleError {
    PowerOnTimeout,
    Closed,
}

const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);

impl MacosBleBackend {
    pub async fn new() -> Result<Self, MacosBleError> {
        let (events_tx, mut events_rx) = tokio_mpsc::unbounded_channel::<Event>();
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();

        std::thread::spawn(move || {
            let queue = DispatchQueue::new("com.personal.prns.ble", None);

            let central_delegate = CentralDelegate::new(events_tx);
            let central_proto = ProtocolObject::from_ref(&*central_delegate);
            let _central: Retained<CBCentralManager> = unsafe {
                CBCentralManager::initWithDelegate_queue(
                    CBCentralManager::alloc(),
                    Some(central_proto),
                    Some(&queue),
                )
            };

            let peripheral_delegate = PeripheralDelegate::new();
            let peripheral_proto = ProtocolObject::from_ref(&*peripheral_delegate);
            let _peripheral: Retained<CBPeripheralManager> = unsafe {
                CBPeripheralManager::initWithDelegate_queue(
                    CBPeripheralManager::alloc(),
                    Some(peripheral_proto),
                    Some(&queue),
                )
            };

            let _ = shutdown_rx.recv();
        });

        loop {
            match tokio::time::timeout(POWER_ON_TIMEOUT, events_rx.recv()).await {
                Ok(Some(Event::Powered { on: true })) => {
                    return Ok(Self {
                        _keepalive: keepalive,
                        events: events_rx,
                    });
                }
                Ok(Some(_)) => continue,
                Ok(None) => return Err(MacosBleError::Closed),
                Err(_) => return Err(MacosBleError::PowerOnTimeout),
            }
        }
    }

    pub async fn next_sighting(&mut self) -> Option<BleAddress> {
        loop {
            match self.events.recv().await? {
                Event::Sighting(address) => return Some(address),
                Event::Powered { .. } => continue,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "needs a real Bluetooth radio + Bluetooth permission; run with `--ignored` on a Mac"]
    async fn the_node_powers_on_advertises_and_scans() {
        let _backend = MacosBleBackend::new()
            .await
            .expect("bluetooth should power on, advertise, and scan");
    }
}
