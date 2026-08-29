use core::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
use super::gatt_link::{gatt_inbound_channel, ControlPlane, GattInboundSender, GattLink};
use super::{
    advertisement_data, cbuuid_eq, columba_identity_uuid, columba_rx_uuid, columba_tx_uuid,
    control_uuid, core_bluetooth_peer_id, data_uuid, service_uuid, CoreBluetoothPeerId, Event,
    SendPeripheralDelegate, SendPeripheralManager,
};

#[derive(Clone, Copy)]
pub(super) enum ListenerCharacteristic {
    Control,
    Data,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdvertisingOp {
    Start,
    Stop,
    None,
}

pub(super) const fn advertising_op(enabled: bool, is_advertising: bool) -> AdvertisingOp {
    match (enabled, is_advertising) {
        (true, false) => AdvertisingOp::Start,
        (false, true) => AdvertisingOp::Stop,
        _ => AdvertisingOp::None,
    }
}

enum InboundProfile {
    Native,
    Columba(BleIdentity),
}

impl InboundProfile {
    fn protocol(&self) -> PeerProtocol {
        match self {
            Self::Native => PeerProtocol::Native,
            Self::Columba(_) => PeerProtocol::Columba,
        }
    }

    fn peer_identity(&self) -> Option<BleIdentity> {
        match self {
            Self::Native => None,
            Self::Columba(identity) => Some(*identity),
        }
    }
}

struct PeripheralPeerSession {
    central: Retained<CBCentral>,
    protocol: PeerProtocol,
    control_tx: tokio_mpsc::Sender<Control>,
    data_tx: GattInboundSender,
}

pub(super) fn has_session_for_peer<V>(
    sessions: &HashMap<CoreBluetoothPeerId, V>,
    peer_id: CoreBluetoothPeerId,
) -> bool {
    sessions.contains_key(&peer_id)
}

impl PeripheralPeerSession {
    fn data_receiver_closed(&self) -> bool {
        // The control receiver is handshake-only and closes when a link settles. The data
        // receiver is retained by the attached member for the full lifetime of this role.
        self.data_tx.is_closed()
    }
}

pub(super) struct PeripheralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
    characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    data_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    columba_rx_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    columba_tx_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    columba_identity_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    queue: DispatchRetained<DispatchQueue>,
    manager: RefCell<Option<SendPeripheralManager>>,
    service_registration_requested: RefCell<bool>,
    l2cap_publication_requested: RefCell<bool>,
    sessions: RefCell<HashMap<CoreBluetoothPeerId, PeripheralPeerSession>>,
    pending_l2cap: RefCell<HashMap<CoreBluetoothPeerId, PendingL2cap>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = PeripheralDelegateIvars]
    pub(super) struct PeripheralDelegate;

    unsafe impl NSObjectProtocol for PeripheralDelegate {}

    unsafe impl CBPeripheralManagerDelegate for PeripheralDelegate {
        #[unsafe(method(peripheralManagerDidUpdateState:))]
        fn did_update_state(&self, peripheral: &CBPeripheralManager) {
            // SAFETY: CoreBluetooth supplied this live manager to its delegate on the configured
            // serial dispatch queue.
            if unsafe { peripheral.state() } == CBManagerState::PoweredOn {
                *self.ivars().manager.borrow_mut() =
                    Some(SendPeripheralManager(peripheral.retain()));
                if !*self.ivars().service_registration_requested.borrow() {
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
                    // SAFETY: every argument is a retained, correctly typed Objective-C object and
                    // the generated initializer returns ownership of the new mutable service.
                    let service = unsafe {
                        CBMutableService::initWithType_primary(
                            CBMutableService::alloc(),
                            &service_uuid(),
                            true,
                        )
                    };
                    // SAFETY: all entries are retained CoreBluetooth characteristics and the array
                    // remains live throughout the synchronous property assignment.
                    unsafe { service.setCharacteristics(Some(&characteristics)) };
                    // SAFETY: the newly initialized service remains retained while the live manager
                    // registers it on the serial CoreBluetooth queue.
                    unsafe { peripheral.addService(&service) };
                    *self.ivars().service_registration_requested.borrow_mut() = true;
                }
                if !*self.ivars().l2cap_publication_requested.borrow() {
                    // SAFETY: the live peripheral manager is messaged only from its delegate's
                    // serial dispatch queue; the boolean has the generated selector's declared
                    // type.
                    unsafe { peripheral.publishL2CAPChannelWithEncryption(false) };
                    *self.ivars().l2cap_publication_requested.borrow_mut() = true;
                }
            }
        }

        #[unsafe(method(peripheralManager:willRestoreState:))]
        fn will_restore_state(
            &self,
            peripheral: &CBPeripheralManager,
            dict: &NSDictionary<NSString, AnyObject>,
        ) {
            *self.ivars().manager.borrow_mut() = Some(SendPeripheralManager(peripheral.retain()));
            // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
            let key: &NSString = unsafe { CBPeripheralManagerRestoredStateServicesKey };
            let Some(restored) = dict.objectForKey(key) else {
                return;
            };
            // SAFETY: CoreBluetooth documents this restoration value as an NSArray of services;
            // `restored` retains the array for the duration of this borrow and iteration.
            let services: &NSArray<CBService> =
                unsafe { &*(Retained::as_ptr(&restored) as *const NSArray<CBService>) };
            let control_id = control_uuid();
            let data_id = data_uuid();
            let columba_rx_id = columba_rx_uuid();
            let columba_tx_id = columba_tx_uuid();
            let columba_identity_id = columba_identity_uuid();
            for service in services.iter() {
                // SAFETY: the service is retained by the restoration array during this iteration.
                let service_id = unsafe { service.UUID() };
                if !cbuuid_eq(&service_id, &service_uuid()) {
                    continue;
                }
                // SAFETY: CoreBluetooth owns and retains the restored service's characteristic
                // collection for the lifetime of the service.
                let Some(characteristics) = (unsafe { service.characteristics() }) else {
                    continue;
                };
                for characteristic in characteristics.iter() {
                    // SAFETY: the characteristic is retained by the collection while it is used.
                    let uuid = unsafe { characteristic.UUID() };
                    // SAFETY: restoration returns the mutable characteristics originally published
                    // by this CBPeripheralManager; the retained object remains live for this cast.
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
                *self.ivars().service_registration_requested.borrow_mut() = true;
                crate::diagnostic_log::debug!(
                    "bluetooth: restored the published Prns GATT service from a background relaunch"
                );
                let _ = self.ivars().events.send(Event::GattServicePublished);
            }
        }

        #[unsafe(method(peripheralManager:didAddService:error:))]
        fn did_add_service(
            &self,
            _peripheral: &CBPeripheralManager,
            _service: &CBService,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                crate::diagnostic_log::error!("bluetooth: GATT service add FAILED: {error:?}");
                let _ = self.ivars().events.send(Event::GattServicePublishFailed);
                return;
            }
            crate::diagnostic_log::debug!(
                "bluetooth: GATT service added (control characteristic live)"
            );
            let _ = self.ivars().events.send(Event::GattServicePublished);
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
                let _ = self.ivars().events.send(Event::L2capPublishFailed);
            } else {
                crate::diagnostic_log::debug!("bluetooth: published L2CAP channel, PSM {psm:#06x}");
                let _ = self.ivars().events.send(Event::L2capPublished { psm });
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
            let Some((peer_id, data)) = wire_l2cap(channel, &self.ivars().queue) else {
                crate::diagnostic_log::warn!(
                    "bluetooth: L2CAP channel exposes no streams — dropping"
                );
                return;
            };
            crate::diagnostic_log::debug!("bluetooth: L2CAP channel opened, data plane up");
            self.ivars()
                .pending_l2cap
                .borrow_mut()
                .entry(peer_id)
                .or_default()
                .deliver(data);
        }

        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn did_receive_write_requests(
            &self,
            peripheral: &CBPeripheralManager,
            requests: &NSArray<CBATTRequest>,
        ) {
            for request in requests.iter() {
                // SAFETY: CoreBluetooth supplied this retained request for the duration of the
                // delegate callback; its optional value has the binding-declared NSData type.
                let Some(value) = (unsafe { request.value() }) else {
                    // SAFETY: the request belongs to this live manager callback and may be answered
                    // exactly once before the callback returns.
                    unsafe {
                        peripheral.respondToRequest_withResult(&request, CBATTError::Success)
                    };
                    continue;
                };
                // SAFETY: the request is live during this callback and retains its characteristic.
                let characteristic = unsafe { request.characteristic() };
                // SAFETY: the returned characteristic is live and its UUID is immutable.
                let written_uuid = unsafe { characteristic.UUID() };
                let bytes = value.to_vec();
                // SAFETY: the live request retains the CBCentral that issued it.
                let central = unsafe { request.central() };
                let peer_id = core_bluetooth_peer_id(&central);
                if cbuuid_eq(&written_uuid, &data_uuid()) {
                    let enqueue_error = self
                        .ivars()
                        .sessions
                        .borrow()
                        .get(&peer_id)
                        .filter(|session| session.protocol == PeerProtocol::Native)
                        .and_then(|session| {
                            session.data_tx.try_send(Box::from(bytes.as_slice())).err()
                        });
                    let result = if let Some(error) = enqueue_error {
                        crate::diagnostic_log::warn!(
                            "bluetooth: GATT write inbox failed for {:02x?}: {error:?}",
                            peer_id.address().octets()
                        );
                        CBATTError::InsufficientResources
                    } else {
                        CBATTError::Success
                    };
                    // SAFETY: the request belongs to this live manager callback and is answered
                    // exactly once on this branch.
                    unsafe { peripheral.respondToRequest_withResult(&request, result) };
                    continue;
                }
                if cbuuid_eq(&written_uuid, &columba_rx_uuid()) {
                    let mut enqueue_error = None;
                    let known = self.ivars().sessions.borrow().contains_key(&peer_id);
                    if !known && bytes.len() == 16 {
                        let mut peer_identity = [0u8; 16];
                        peer_identity.copy_from_slice(&bytes);
                        self.open_inbound(
                            &central,
                            peer_id,
                            InboundProfile::Columba(BleIdentity::new(peer_identity)),
                        );
                    } else if let Some(session) = self
                        .ivars()
                        .sessions
                        .borrow()
                        .get(&peer_id)
                        .filter(|session| session.protocol == PeerProtocol::Columba)
                    {
                        enqueue_error = session.data_tx.try_send(Box::from(bytes.as_slice())).err();
                    }
                    let result = if let Some(error) = enqueue_error {
                        crate::diagnostic_log::warn!(
                            "bluetooth: Columba GATT write inbox failed for {:02x?}: {error:?}",
                            peer_id.address().octets()
                        );
                        CBATTError::InsufficientResources
                    } else {
                        CBATTError::Success
                    };
                    // SAFETY: the request belongs to this live manager callback and is answered
                    // exactly once on this branch.
                    unsafe { peripheral.respondToRequest_withResult(&request, result) };
                    continue;
                }
                if let Some(control) = Control::decode(&bytes) {
                    if !self.ivars().sessions.borrow().contains_key(&peer_id) {
                        self.open_inbound(&central, peer_id, InboundProfile::Native);
                    }
                    if let Some(session) = self
                        .ivars()
                        .sessions
                        .borrow()
                        .get(&peer_id)
                        .filter(|session| session.protocol == PeerProtocol::Native)
                    {
                        let _ = session.control_tx.try_send(control);
                    }
                }
                // SAFETY: this is the sole response for this request on the fall-through branch,
                // sent while both the request and manager remain live in their delegate callback.
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
            let peer_id = core_bluetooth_peer_id(central);
            // SAFETY: the supplied characteristic is live and its UUID is immutable.
            let uuid = unsafe { characteristic.UUID() };
            let protocol = if cbuuid_eq(&uuid, &columba_tx_uuid()) {
                PeerProtocol::Columba
            } else {
                PeerProtocol::Native
            };
            crate::diagnostic_log::debug!(
                "bluetooth: central {:02x?} subscribed to {protocol:?} notifications",
                peer_id.address().octets(),
            );
        }

        #[unsafe(method(peripheralManager:central:didUnsubscribeFromCharacteristic:))]
        fn did_unsubscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            central: &CBCentral,
            characteristic: &CBCharacteristic,
        ) {
            let peer_id = core_bluetooth_peer_id(central);
            // SAFETY: the supplied characteristic is live and its UUID is immutable.
            let uuid = unsafe { characteristic.UUID() };
            let unsubscribed_protocol = if cbuuid_eq(&uuid, &control_uuid()) {
                Some(PeerProtocol::Native)
            } else if cbuuid_eq(&uuid, &columba_tx_uuid()) {
                Some(PeerProtocol::Columba)
            } else {
                None
            };
            let remove = self
                .ivars()
                .sessions
                .borrow()
                .get(&peer_id)
                .is_some_and(|session| Some(session.protocol) == unsubscribed_protocol);
            if remove {
                self.ivars().sessions.borrow_mut().remove(&peer_id);
                self.ivars().pending_l2cap.borrow_mut().remove(&peer_id);
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
    /// Queue-confined: call only from the CoreBluetooth serial dispatch queue.
    pub(super) fn has_inbound_session(&self, peer_id: CoreBluetoothPeerId) -> bool {
        has_session_for_peer(&self.ivars().sessions.borrow(), peer_id)
    }

    pub(super) fn new(
        events: tokio_mpsc::UnboundedSender<Event>,
        queue: DispatchRetained<DispatchQueue>,
        identity: BleIdentity,
    ) -> Retained<Self> {
        let data_plane_properties = CBCharacteristicProperties::Write
            | CBCharacteristicProperties::WriteWithoutResponse
            | CBCharacteristicProperties::Notify;
        // SAFETY: all initializer arguments are retained and correctly typed; the generated binding
        // returns ownership of a newly allocated mutable characteristic.
        let characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &control_uuid(),
                data_plane_properties,
                None,
                CBAttributePermissions::Writeable,
            )
        };
        // SAFETY: all initializer arguments are retained and correctly typed; the generated binding
        // returns ownership of a newly allocated mutable characteristic.
        let data_characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &data_uuid(),
                data_plane_properties,
                None,
                CBAttributePermissions::Writeable,
            )
        };
        // SAFETY: all initializer arguments are retained and correctly typed; the generated binding
        // returns ownership of a newly allocated mutable characteristic.
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
        // SAFETY: all initializer arguments are retained and correctly typed; the generated binding
        // returns ownership of a newly allocated mutable characteristic.
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
        // SAFETY: the immutable identity NSData and all initializer arguments stay live for the
        // call; the generated binding returns ownership of the new mutable characteristic.
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
            service_registration_requested: RefCell::new(false),
            l2cap_publication_requested: RefCell::new(false),
            sessions: RefCell::new(HashMap::new()),
            pending_l2cap: RefCell::new(HashMap::new()),
        });
        // SAFETY: `this` is a freshly allocated PeripheralDelegate with fully initialized ivars;
        // forwarding to NSObject's designated initializer preserves its allocation identity.
        unsafe { msg_send![super(this), init] }
    }

    fn open_inbound(
        &self,
        central: &CBCentral,
        peer_id: CoreBluetoothPeerId,
        profile: InboundProfile,
    ) {
        let (control_tx, control_rx) = tokio_mpsc::channel::<Control>(8);
        let (data_tx, data_rx) = gatt_inbound_channel();
        let protocol = profile.protocol();
        let peer_identity = profile.peer_identity();
        // SAFETY: this is an immutable property query on the live requesting central.
        let gatt_mtu = unsafe { central.maximumUpdateValueLength() }
            .clamp(FRAGMENT_HEADER_LEN + 1, BLE_HW_MTU);
        let address = peer_id.address();
        crate::diagnostic_log::debug!(
            "bluetooth: inbound central {:02x?} — {protocol:?} control link opened",
            address.octets()
        );
        self.ivars().sessions.borrow_mut().insert(
            peer_id,
            PeripheralPeerSession {
                central: central.retain(),
                protocol,
                control_tx,
                data_tx,
            },
        );
        let link = GattLink {
            peer_protocol: protocol,
            peer_identity,
            control: ControlPlane::Listener {
                peer_id,
                delegate: SendPeripheralDelegate(self.retain()),
                gatt_mtu,
            },
            control_rx,
            address,
            data_inbound_rx: Some(data_rx),
            l2cap_pending: None,
        };
        let _ = self.ivars().events.send(Event::Inbound(link));
    }

    pub(super) fn notify(
        &self,
        peer_id: CoreBluetoothPeerId,
        target: ListenerCharacteristic,
        bytes: &[u8],
    ) -> bool {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        let bytes = Box::<[u8]>::from(bytes);
        let sent = Arc::new(AtomicBool::new(false));
        let result = sent.clone();
        queue.exec_sync(move || {
            let this = this;
            let Some(manager) = this
                .0
                .ivars()
                .manager
                .borrow()
                .as_ref()
                .map(|manager| manager.0.clone())
            else {
                return;
            };
            let Some((central, protocol)) = this
                .0
                .ivars()
                .sessions
                .borrow()
                .get(&peer_id)
                .map(|session| (session.central.clone(), session.protocol))
            else {
                return;
            };
            let characteristic = match (protocol, target) {
                (PeerProtocol::Native, ListenerCharacteristic::Control) => {
                    this.0.ivars().characteristic.borrow()
                }
                (PeerProtocol::Native, ListenerCharacteristic::Data) => {
                    this.0.ivars().data_characteristic.borrow()
                }
                (PeerProtocol::Columba, _) => this.0.ivars().columba_tx_characteristic.borrow(),
            };
            let data = NSData::with_bytes(&bytes);
            let centrals = NSArray::from_slice(&[&*central]);
            // SAFETY: the retained mutable characteristic was published by this retained manager;
            // CoreBluetooth receives retained data and an exact live subscribed central.
            let accepted = unsafe {
                manager.updateValue_forCharacteristic_onSubscribedCentrals(
                    &data,
                    &characteristic,
                    Some(&centrals),
                )
            };
            result.store(accepted, Ordering::Release);
        });
        sent.load(Ordering::Acquire)
    }

    pub(super) fn arm_pending_channel(
        &self,
        peer_id: CoreBluetoothPeerId,
        tx: oneshot::Sender<DataPlane>,
    ) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            this.0
                .ivars()
                .pending_l2cap
                .borrow_mut()
                .entry(peer_id)
                .or_default()
                .arm(tx);
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
            // SAFETY: this authoritative CoreBluetooth state query runs on the retained manager's
            // serial dispatch queue.
            let is_advertising = unsafe { manager.isAdvertising() };
            match advertising_op(mode.is_on(), is_advertising) {
                AdvertisingOp::Start => {
                    let uuid = service_uuid();
                    let services = NSArray::from_slice(&[&*uuid]);
                    let data = advertisement_data(&services);
                    // SAFETY: the retained manager is messaged on its serial dispatch queue and the
                    // advertisement dictionary remains live for the synchronous call.
                    unsafe { manager.startAdvertising(Some(&data)) };
                }
                AdvertisingOp::Stop => {
                    // SAFETY: the retained manager is messaged only on its serial dispatch queue.
                    unsafe { manager.stopAdvertising() };
                    crate::diagnostic_log::debug!(
                        "bluetooth: advertising stopped — at connection capacity"
                    );
                }
                AdvertisingOp::None => {}
            }
        });
    }

    pub(super) fn clear_closed_peer(&self, address: BleAddress) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            let mut removed = false;
            this.0
                .ivars()
                .sessions
                .borrow_mut()
                .retain(|peer_id, session| {
                    let remove = peer_id.address() == address && session.data_receiver_closed();
                    removed |= remove;
                    !remove
                });
            if removed {
                this.0
                    .ivars()
                    .pending_l2cap
                    .borrow_mut()
                    .retain(|peer_id, _| peer_id.address() != address);
            }
        });
    }
}
