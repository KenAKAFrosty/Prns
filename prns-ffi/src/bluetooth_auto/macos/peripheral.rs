use core::cell::RefCell;

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_bluetooth::{
    CBATTError, CBATTRequest, CBAttributePermissions, CBCentral, CBCharacteristic,
    CBCharacteristicProperties, CBL2CAPChannel, CBManagerState, CBMutableCharacteristic,
    CBMutableService, CBPeripheralManager, CBPeripheralManagerDelegate,
    CBPeripheralManagerRestoredStateServicesKey, CBService,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSObject, NSObjectProtocol, NSString,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use prns_core::interfaces::bluetooth_auto::AdvertisingMode;
use prns_core::interfaces::bluetooth_auto::{
    BleAddress, BleIdentity, Control, PeerProtocol, BLE_HW_MTU, FRAGMENT_HEADER_LEN,
};

use super::data_plane::{wire_l2cap, DataPlane, PendingL2cap};
use super::gatt_link::{ControlPlane, GattLink};
use super::{
    advertisement_data, cbuuid_eq, columba_identity_uuid, columba_rx_uuid, columba_tx_uuid,
    control_uuid, data_uuid, service_uuid, uuid_token, Event, SendCharacteristic,
    SendPeripheralDelegate, SendPeripheralManager,
};

pub(super) struct PeripheralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
    characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    data_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    columba_rx_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    columba_tx_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    columba_identity_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    queue: DispatchRetained<DispatchQueue>,
    manager: RefCell<Option<SendPeripheralManager>>,
    service_published: RefCell<bool>,
    active: RefCell<Option<tokio_mpsc::Sender<Control>>>,
    active_protocol: RefCell<Option<PeerProtocol>>,
    active_address: RefCell<Option<[u8; 6]>>,
    data_inbound: RefCell<Option<tokio_mpsc::Sender<Box<[u8]>>>>,
    pending: RefCell<PendingL2cap>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = PeripheralDelegateIvars]
    pub(super) struct PeripheralDelegate;

    unsafe impl NSObjectProtocol for PeripheralDelegate {}

    unsafe impl CBPeripheralManagerDelegate for PeripheralDelegate {
        #[unsafe(method(peripheralManagerDidUpdateState:))]
        fn did_update_state(&self, peripheral: &CBPeripheralManager) {
            if unsafe { peripheral.state() } == CBManagerState::PoweredOn {
                *self.ivars().manager.borrow_mut() =
                    Some(SendPeripheralManager(peripheral.retain()));
                if !*self.ivars().service_published.borrow() {
                    let control_ref = self.ivars().characteristic.borrow();
                    let data_ref = self.ivars().data_characteristic.borrow();
                    let columba_rx_ref = self.ivars().columba_rx_characteristic.borrow();
                    let columba_tx_ref = self.ivars().columba_tx_characteristic.borrow();
                    let columba_identity_ref =
                        self.ivars().columba_identity_characteristic.borrow();
                    let control: &CBCharacteristic = &control_ref;
                    let data: &CBCharacteristic = &data_ref;
                    let columba_rx: &CBCharacteristic = &columba_rx_ref;
                    let columba_tx: &CBCharacteristic = &columba_tx_ref;
                    let columba_identity: &CBCharacteristic = &columba_identity_ref;
                    let characteristics = NSArray::from_slice(&[
                        control,
                        data,
                        columba_rx,
                        columba_tx,
                        columba_identity,
                    ]);
                    let service = unsafe {
                        CBMutableService::initWithType_primary(
                            CBMutableService::alloc(),
                            &service_uuid(),
                            true,
                        )
                    };
                    unsafe { service.setCharacteristics(Some(&characteristics)) };
                    unsafe { peripheral.addService(&service) };
                    *self.ivars().service_published.borrow_mut() = true;
                }
                unsafe { peripheral.publishL2CAPChannelWithEncryption(false) };
            }
        }

        #[unsafe(method(peripheralManager:willRestoreState:))]
        fn will_restore_state(
            &self,
            peripheral: &CBPeripheralManager,
            dict: &NSDictionary<NSString, AnyObject>,
        ) {
            *self.ivars().manager.borrow_mut() = Some(SendPeripheralManager(peripheral.retain()));
            let key: &NSString = unsafe { CBPeripheralManagerRestoredStateServicesKey };
            let Some(restored) = dict.objectForKey(key) else {
                return;
            };
            let services: &NSArray<CBService> =
                unsafe { &*(Retained::as_ptr(&restored) as *const NSArray<CBService>) };
            let control_id = control_uuid();
            let data_id = data_uuid();
            let columba_rx_id = columba_rx_uuid();
            let columba_tx_id = columba_tx_uuid();
            let columba_identity_id = columba_identity_uuid();
            for service in services.iter() {
                let service_id = unsafe { service.UUID() };
                if !cbuuid_eq(&service_id, &service_uuid()) {
                    continue;
                }
                let Some(characteristics) = (unsafe { service.characteristics() }) else {
                    continue;
                };
                for characteristic in characteristics.iter() {
                    let uuid = unsafe { characteristic.UUID() };
                    let mutable: &CBMutableCharacteristic = unsafe {
                        &*(Retained::as_ptr(&characteristic) as *const CBMutableCharacteristic)
                    };
                    if cbuuid_eq(&uuid, &control_id) {
                        *self.ivars().characteristic.borrow_mut() = mutable.retain();
                    } else if cbuuid_eq(&uuid, &data_id) {
                        *self.ivars().data_characteristic.borrow_mut() = mutable.retain();
                    } else if cbuuid_eq(&uuid, &columba_rx_id) {
                        *self.ivars().columba_rx_characteristic.borrow_mut() = mutable.retain();
                    } else if cbuuid_eq(&uuid, &columba_tx_id) {
                        *self.ivars().columba_tx_characteristic.borrow_mut() = mutable.retain();
                    } else if cbuuid_eq(&uuid, &columba_identity_id) {
                        *self.ivars().columba_identity_characteristic.borrow_mut() =
                            mutable.retain();
                    }
                }
                *self.ivars().service_published.borrow_mut() = true;
                crate::diagnostic_log::debug!(
                    "bluetooth: restored the published Prns GATT service from a background relaunch"
                );
            }
        }

        #[unsafe(method(peripheralManager:didAddService:error:))]
        fn did_add_service(
            &self,
            peripheral: &CBPeripheralManager,
            _service: &CBService,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                crate::diagnostic_log::error!("bluetooth: GATT service add FAILED: {error:?}");
                return;
            }
            crate::diagnostic_log::debug!(
                "bluetooth: GATT service added (control characteristic live), starting advertising"
            );
            let uuid = service_uuid();
            let services = NSArray::from_slice(&[&*uuid]);
            let data = advertisement_data(&services);
            unsafe { peripheral.startAdvertising(Some(&data)) };
        }

        #[unsafe(method(peripheralManagerDidStartAdvertising:error:))]
        fn did_start_advertising(
            &self,
            _peripheral: &CBPeripheralManager,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                crate::diagnostic_log::error!("bluetooth: advertising FAILED to start: {error:?}");
            } else {
                crate::diagnostic_log::debug!(
                    "bluetooth: advertising started — discoverable as Prns, service UUID in the BlueZ-visible packet"
                );
            }
        }

        #[unsafe(method(peripheralManager:didPublishL2CAPChannel:error:))]
        fn did_publish_l2cap(
            &self,
            _peripheral: &CBPeripheralManager,
            psm: u16,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                crate::diagnostic_log::error!("bluetooth: L2CAP publish FAILED: {error:?}");
                let _ = self.ivars().events.send(Event::PublishFailed);
            } else {
                crate::diagnostic_log::debug!("bluetooth: published L2CAP channel, PSM {psm:#06x}");
                let _ = self.ivars().events.send(Event::Published { psm });
            }
        }

        #[unsafe(method(peripheralManager:didOpenL2CAPChannel:error:))]
        fn did_open_l2cap(
            &self,
            _peripheral: &CBPeripheralManager,
            channel: Option<&CBL2CAPChannel>,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                crate::diagnostic_log::warn!("bluetooth: L2CAP channel open FAILED: {error:?}");
            }
            let Some(channel) = channel else {
                crate::diagnostic_log::warn!(
                    "bluetooth: L2CAP open callback with no channel — data plane not established"
                );
                return;
            };
            let Some(data) = wire_l2cap(channel, &self.ivars().queue) else {
                crate::diagnostic_log::warn!(
                    "bluetooth: L2CAP channel exposes no streams — dropping"
                );
                return;
            };
            crate::diagnostic_log::debug!("bluetooth: L2CAP channel opened, data plane up");
            self.ivars().pending.borrow_mut().deliver(data);
        }

        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn did_receive_write_requests(
            &self,
            peripheral: &CBPeripheralManager,
            requests: &NSArray<CBATTRequest>,
        ) {
            for request in requests.iter() {
                let Some(value) = (unsafe { request.value() }) else {
                    unsafe {
                        peripheral.respondToRequest_withResult(&request, CBATTError::Success)
                    };
                    continue;
                };
                let characteristic = unsafe { request.characteristic() };
                let written_uuid = unsafe { characteristic.UUID() };
                let bytes = value.to_vec();
                if cbuuid_eq(&written_uuid, &data_uuid()) {
                    if self.ivars().active_protocol.borrow().as_ref() == Some(&PeerProtocol::Native)
                    {
                        if let Some(tx) = self.ivars().data_inbound.borrow().as_ref() {
                            let _ = tx.try_send(Box::from(bytes.as_slice()));
                        }
                    }
                    unsafe {
                        peripheral.respondToRequest_withResult(&request, CBATTError::Success)
                    };
                    continue;
                }
                if cbuuid_eq(&written_uuid, &columba_rx_uuid()) {
                    let mut active = self.ivars().active.borrow_mut();
                    if active.is_none() && bytes.len() == 16 {
                        let mut peer_identity = [0u8; 16];
                        peer_identity.copy_from_slice(&bytes);
                        let (tx, rx) = tokio_mpsc::channel::<Control>(8);
                        let (data_tx, data_rx) = tokio_mpsc::channel::<Box<[u8]>>(16);
                        let central = unsafe { request.central() };
                        let gatt_mtu = unsafe { central.maximumUpdateValueLength() }
                            .clamp(FRAGMENT_HEADER_LEN + 1, BLE_HW_MTU);
                        let identifier = unsafe { central.identifier() };
                        let address = BleAddress::new(uuid_token(&identifier));
                        *self.ivars().active_address.borrow_mut() = Some(*address.octets());
                        crate::diagnostic_log::debug!(
                            "bluetooth: inbound central {:02x?} — control link opened, handshaking",
                            address.octets()
                        );
                        let link = GattLink {
                            peer_protocol: PeerProtocol::Columba,
                            peer_identity: Some(BleIdentity::new(peer_identity)),
                            control: ControlPlane::Listener {
                                manager: SendPeripheralManager(peripheral.retain()),
                                characteristic: SendCharacteristic(
                                    self.ivars().columba_tx_characteristic.borrow().clone(),
                                ),
                                data_characteristic: SendCharacteristic(
                                    self.ivars().columba_tx_characteristic.borrow().clone(),
                                ),
                                delegate: SendPeripheralDelegate(self.retain()),
                                gatt_mtu,
                            },
                            control_rx: rx,
                            address,
                            data_inbound_rx: Some(data_rx),
                            l2cap_pending: None,
                        };
                        let _ = self.ivars().events.send(Event::Inbound(link));
                        *active = Some(tx);
                        *self.ivars().active_protocol.borrow_mut() = Some(PeerProtocol::Columba);
                        *self.ivars().data_inbound.borrow_mut() = Some(data_tx);
                    } else if self.ivars().active_protocol.borrow().as_ref()
                        == Some(&PeerProtocol::Columba)
                    {
                        if let Some(tx) = self.ivars().data_inbound.borrow().as_ref() {
                            let _ = tx.try_send(Box::from(bytes.as_slice()));
                        }
                    }
                    unsafe {
                        peripheral.respondToRequest_withResult(&request, CBATTError::Success)
                    };
                    continue;
                }
                if let Some(control) = Control::decode(&bytes) {
                    let mut active = self.ivars().active.borrow_mut();
                    if active.is_none() {
                        let (tx, rx) = tokio_mpsc::channel::<Control>(8);
                        let (data_tx, data_rx) = tokio_mpsc::channel::<Box<[u8]>>(16);
                        let central = unsafe { request.central() };
                        let gatt_mtu = unsafe { central.maximumUpdateValueLength() }
                            .clamp(FRAGMENT_HEADER_LEN + 1, BLE_HW_MTU);
                        let identifier = unsafe { central.identifier() };
                        let address = BleAddress::new(uuid_token(&identifier));
                        *self.ivars().active_address.borrow_mut() = Some(*address.octets());
                        crate::diagnostic_log::debug!(
                            "bluetooth: inbound central {:02x?} — native control link opened",
                            address.octets()
                        );
                        let link = GattLink {
                            peer_protocol: PeerProtocol::Native,
                            peer_identity: None,
                            control: ControlPlane::Listener {
                                manager: SendPeripheralManager(peripheral.retain()),
                                characteristic: SendCharacteristic(
                                    self.ivars().characteristic.borrow().clone(),
                                ),
                                data_characteristic: SendCharacteristic(
                                    self.ivars().data_characteristic.borrow().clone(),
                                ),
                                delegate: SendPeripheralDelegate(self.retain()),
                                gatt_mtu,
                            },
                            control_rx: rx,
                            address,
                            data_inbound_rx: Some(data_rx),
                            l2cap_pending: None,
                        };
                        let _ = self.ivars().events.send(Event::Inbound(link));
                        *active = Some(tx);
                        *self.ivars().active_protocol.borrow_mut() = Some(PeerProtocol::Native);
                        *self.ivars().data_inbound.borrow_mut() = Some(data_tx);
                    }
                    if self.ivars().active_protocol.borrow().as_ref() == Some(&PeerProtocol::Native)
                    {
                        if let Some(tx) = active.as_ref() {
                            let _ = tx.try_send(control);
                        }
                    }
                }
                unsafe { peripheral.respondToRequest_withResult(&request, CBATTError::Success) };
            }
        }

        #[unsafe(method(peripheralManager:central:didSubscribeToCharacteristic:))]
        fn did_subscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            central: &CBCentral,
            characteristic: &CBCharacteristic,
        ) {
            let identifier = unsafe { central.identifier() };
            let uuid = unsafe { characteristic.UUID() };
            let protocol = if cbuuid_eq(&uuid, &columba_tx_uuid()) {
                PeerProtocol::Columba
            } else {
                PeerProtocol::Native
            };
            crate::diagnostic_log::debug!(
                "bluetooth: central {:02x?} subscribed to {protocol:?} notifications",
                uuid_token(&identifier),
            );
        }

        #[unsafe(method(peripheralManager:central:didUnsubscribeFromCharacteristic:))]
        fn did_unsubscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            central: &CBCentral,
            characteristic: &CBCharacteristic,
        ) {
            let identifier = unsafe { central.identifier() };
            let token = uuid_token(&identifier);
            let uuid = unsafe { characteristic.UUID() };
            let unsubscribed_protocol = if cbuuid_eq(&uuid, &control_uuid()) {
                Some(PeerProtocol::Native)
            } else if cbuuid_eq(&uuid, &columba_tx_uuid()) {
                Some(PeerProtocol::Columba)
            } else {
                None
            };
            if self
                .ivars()
                .active_address
                .borrow()
                .is_none_or(|active| active == token)
                && unsubscribed_protocol == *self.ivars().active_protocol.borrow()
            {
                self.ivars().active.borrow_mut().take();
                self.ivars().active_protocol.borrow_mut().take();
                self.ivars().active_address.borrow_mut().take();
                self.ivars().data_inbound.borrow_mut().take();
                self.ivars().pending.borrow_mut().clear();
            }
        }

        #[unsafe(method(peripheralManagerIsReadyToUpdateSubscribers:))]
        fn is_ready_to_update(&self, _peripheral: &CBPeripheralManager) {
            crate::diagnostic_log::debug!(
                "bluetooth: notify queue drained — ready to update subscribers"
            );
        }
    }
);

impl PeripheralDelegate {
    pub(super) fn new(
        events: tokio_mpsc::UnboundedSender<Event>,
        queue: DispatchRetained<DispatchQueue>,
        identity: BleIdentity,
    ) -> Retained<Self> {
        let data_plane_properties = CBCharacteristicProperties::Write
            | CBCharacteristicProperties::WriteWithoutResponse
            | CBCharacteristicProperties::Notify;
        let characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &control_uuid(),
                data_plane_properties,
                None,
                CBAttributePermissions::Writeable,
            )
        };
        let data_characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &data_uuid(),
                data_plane_properties,
                None,
                CBAttributePermissions::Writeable,
            )
        };
        let columba_rx_characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &columba_rx_uuid(),
                CBCharacteristicProperties::Write
                    | CBCharacteristicProperties::WriteWithoutResponse,
                None,
                CBAttributePermissions::Writeable,
            )
        };
        let columba_tx_characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &columba_tx_uuid(),
                CBCharacteristicProperties::Read | CBCharacteristicProperties::Notify,
                None,
                CBAttributePermissions::Readable,
            )
        };
        let identity_value = NSData::with_bytes(identity.as_bytes());
        let columba_identity_characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &columba_identity_uuid(),
                CBCharacteristicProperties::Read,
                Some(&identity_value),
                CBAttributePermissions::Readable,
            )
        };
        let this = Self::alloc().set_ivars(PeripheralDelegateIvars {
            events,
            characteristic: RefCell::new(characteristic),
            data_characteristic: RefCell::new(data_characteristic),
            columba_rx_characteristic: RefCell::new(columba_rx_characteristic),
            columba_tx_characteristic: RefCell::new(columba_tx_characteristic),
            columba_identity_characteristic: RefCell::new(columba_identity_characteristic),
            queue,
            manager: RefCell::new(None),
            service_published: RefCell::new(false),
            active: RefCell::new(None),
            active_protocol: RefCell::new(None),
            active_address: RefCell::new(None),
            data_inbound: RefCell::new(None),
            pending: RefCell::new(PendingL2cap::default()),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub(super) fn arm_pending_channel(&self, tx: oneshot::Sender<DataPlane>) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            this.0.ivars().pending.borrow_mut().arm(tx);
        });
    }

    pub(super) fn set_advertising(&self, mode: AdvertisingMode) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            let Some(manager) = this
                .0
                .ivars()
                .manager
                .borrow()
                .as_ref()
                .map(|m| m.0.clone())
            else {
                return;
            };
            if mode.is_on() {
                let uuid = service_uuid();
                let services = NSArray::from_slice(&[&*uuid]);
                let data = advertisement_data(&services);
                unsafe { manager.startAdvertising(Some(&data)) };
            } else {
                unsafe { manager.stopAdvertising() };
                crate::diagnostic_log::debug!(
                    "bluetooth: advertising stopped — at connection capacity"
                );
            }
        });
    }

    pub(super) fn clear_active(&self, address: [u8; 6]) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            if this
                .0
                .ivars()
                .active_address
                .borrow()
                .is_some_and(|active| active == address)
            {
                this.0.ivars().active.borrow_mut().take();
                this.0.ivars().active_protocol.borrow_mut().take();
                this.0.ivars().active_address.borrow_mut().take();
                this.0.ivars().data_inbound.borrow_mut().take();
                this.0.ivars().pending.borrow_mut().clear();
            }
        });
    }
}
