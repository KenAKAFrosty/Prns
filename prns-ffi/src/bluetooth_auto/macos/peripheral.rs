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
    HANDSHAKE_SLACK,
};

use super::backend::central_peripheral_capacity;
use super::data_plane::{close_l2cap, l2cap_peer_id, wire_l2cap, DataPlane, PendingL2cap};
use super::gatt_link::{gatt_inbound_channel, ControlPlane, GattInboundSender, GattLink};
use super::{
    advertisement_data, cbuuid_eq, columba_identity_uuid, columba_rx_uuid, columba_tx_uuid,
    control_uuid, core_bluetooth_peer_id, data_uuid, service_uuid, try_bounded_ingress,
    BoundedIngress, CoreBluetoothPeerId, ManagerSignalSender, SendPeripheralDelegate,
    SendPeripheralManager,
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

pub(super) const fn peripheral_session_capacity(max_peers: usize) -> usize {
    max_peers.saturating_mul(2).saturating_add(HANDSHAKE_SLACK)
}

pub(super) const fn pending_l2cap_capacity(max_peers: usize) -> usize {
    central_peripheral_capacity(max_peers).saturating_add(peripheral_session_capacity(max_peers))
}

pub(super) const fn can_open_inbound(current_sessions: usize, capacity: usize) -> bool {
    current_sessions < capacity
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum L2capDeliveryAdmission {
    Existing,
    Listener,
    Unknown,
    Full,
}

pub(super) fn l2cap_delivery_admission<S, P>(
    sessions: &HashMap<CoreBluetoothPeerId, S>,
    pending: &HashMap<CoreBluetoothPeerId, P>,
    peer_id: CoreBluetoothPeerId,
    capacity: usize,
) -> L2capDeliveryAdmission {
    if pending.contains_key(&peer_id) {
        L2capDeliveryAdmission::Existing
    } else if !sessions.contains_key(&peer_id) {
        L2capDeliveryAdmission::Unknown
    } else if pending.len() >= capacity {
        L2capDeliveryAdmission::Full
    } else {
        L2capDeliveryAdmission::Listener
    }
}

pub(super) fn can_arm_l2cap<P>(
    pending: &HashMap<CoreBluetoothPeerId, P>,
    peer_id: CoreBluetoothPeerId,
    capacity: usize,
) -> bool {
    pending.contains_key(&peer_id) || pending.len() < capacity
}

pub(super) fn reap_closed_sessions<S, P>(
    sessions: &mut HashMap<CoreBluetoothPeerId, S>,
    pending: &mut HashMap<CoreBluetoothPeerId, P>,
    address: Option<BleAddress>,
    mut is_closed: impl FnMut(&S) -> bool,
) -> usize {
    let closed = sessions
        .iter()
        .filter_map(|(peer_id, session)| {
            let address_matches = address.is_none_or(|address| peer_id.address() == address);
            (address_matches && is_closed(session)).then_some(*peer_id)
        })
        .collect::<Vec<_>>();
    for peer_id in &closed {
        sessions.remove(peer_id);
        pending.remove(peer_id);
    }
    closed.len()
}

pub(super) fn reap_stale_pending_l2cap(
    pending: &mut HashMap<CoreBluetoothPeerId, PendingL2cap>,
) -> usize {
    let before = pending.len();
    pending.retain(|_, state| {
        state.reap_closed();
        !state.is_empty()
    });
    before.saturating_sub(pending.len())
}

impl PeripheralPeerSession {
    fn data_receiver_closed(&self) -> bool {
        // The control receiver is handshake-only and closes when a link settles. The data
        // receiver is retained by the attached member for the full lifetime of this role.
        self.data_tx.is_closed()
    }
}

pub(super) struct PeripheralDelegateIvars {
    manager_signals: ManagerSignalSender,
    inbound: tokio_mpsc::Sender<GattLink>,
    characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    data_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    columba_rx_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    columba_tx_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    columba_identity_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    queue: DispatchRetained<DispatchQueue>,
    manager: RefCell<Option<SendPeripheralManager>>,
    radio_enabled: Arc<AtomicBool>,
    service_registration_requested: RefCell<bool>,
    l2cap_publication_requested: RefCell<bool>,
    session_capacity: usize,
    pending_l2cap_capacity: usize,
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
                self.ivars().manager_signals.gatt_service_published();
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
                self.ivars().manager_signals.gatt_service_publish_failed();
                return;
            }
            crate::diagnostic_log::debug!(
                "bluetooth: GATT service added (control characteristic live)"
            );
            self.ivars().manager_signals.gatt_service_published();
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
                self.ivars().manager_signals.l2cap_publish_failed();
            } else {
                crate::diagnostic_log::debug!("bluetooth: published L2CAP channel, PSM {psm:#06x}");
                self.ivars().manager_signals.l2cap_published(psm);
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
            if !self.ivars().radio_enabled.load(Ordering::Acquire) {
                crate::diagnostic_log::debug!(
                    "bluetooth: refusing inbound L2CAP channel while the logical radio is off"
                );
                close_l2cap(channel);
                return;
            }
            let Some(peer_id) = l2cap_peer_id(channel) else {
                crate::diagnostic_log::warn!(
                    "bluetooth: refusing L2CAP channel with no exact CoreBluetooth peer identity"
                );
                close_l2cap(channel);
                return;
            };
            self.reap_closed_state_on_queue();
            let (admission, pending_count, peer_has_capacity) = {
                let sessions = self.ivars().sessions.borrow();
                let pending = self.ivars().pending_l2cap.borrow();
                (
                    l2cap_delivery_admission(
                        &sessions,
                        &pending,
                        peer_id,
                        self.ivars().pending_l2cap_capacity,
                    ),
                    pending.len(),
                    pending.get(&peer_id).is_none_or(PendingL2cap::can_deliver),
                )
            };
            match admission {
                L2capDeliveryAdmission::Unknown => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: refusing L2CAP channel from {:02x?} — no exact peer session or waiter",
                        peer_id.address().octets()
                    );
                    close_l2cap(channel);
                    return;
                }
                L2capDeliveryAdmission::Full => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: refusing L2CAP channel from {:02x?} — pending-peer capacity is full ({pending_count}/{})",
                        peer_id.address().octets(),
                        self.ivars().pending_l2cap_capacity
                    );
                    close_l2cap(channel);
                    return;
                }
                L2capDeliveryAdmission::Existing | L2capDeliveryAdmission::Listener
                    if !peer_has_capacity =>
                {
                    crate::diagnostic_log::warn!(
                        "bluetooth: refusing L2CAP channel from {:02x?} — this exact peer's pending-channel capacity is full",
                        peer_id.address().octets()
                    );
                    close_l2cap(channel);
                    return;
                }
                L2capDeliveryAdmission::Existing | L2capDeliveryAdmission::Listener => {}
            }
            // Stream access, callback registration, and `open` happen only after the exact peer
            // passed both aggregate and per-peer admission above.
            let Some(data) = wire_l2cap(channel, &self.ivars().queue) else {
                crate::diagnostic_log::warn!(
                    "bluetooth: admitted L2CAP channel exposes no streams — dropping"
                );
                close_l2cap(channel);
                return;
            };
            crate::diagnostic_log::debug!("bluetooth: L2CAP channel opened, data plane up");
            let mut pending = self.ivars().pending_l2cap.borrow_mut();
            let entry = pending.entry(peer_id).or_default();
            if !entry.deliver(data) {
                crate::diagnostic_log::warn!(
                    "bluetooth: dropping L2CAP channel from {:02x?} — exact-peer delivery capacity closed after admission",
                    peer_id.address().octets()
                );
            }
            if entry.is_empty() {
                pending.remove(&peer_id);
            }
        }

        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn did_receive_write_requests(
            &self,
            peripheral: &CBPeripheralManager,
            requests: &NSArray<CBATTRequest>,
        ) {
            if !self.ivars().radio_enabled.load(Ordering::Acquire) {
                for request in requests.iter() {
                    // SAFETY: every request belongs to this live callback and is answered exactly
                    // once before returning; disabled state cannot accept new session work.
                    unsafe {
                        peripheral.respondToRequest_withResult(
                            &request,
                            CBATTError::InsufficientResources,
                        )
                    };
                }
                return;
            }
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
                self.reap_closed_state_on_queue();
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
                    let mut accepted = true;
                    let known = self.ivars().sessions.borrow().contains_key(&peer_id);
                    if !known && bytes.len() == 16 {
                        let mut peer_identity = [0u8; 16];
                        peer_identity.copy_from_slice(&bytes);
                        accepted = self.open_inbound(
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
                        if let Err(error) = session.data_tx.try_send(Box::from(bytes.as_slice())) {
                            crate::diagnostic_log::warn!(
                                "bluetooth: Columba GATT write inbox failed for {:02x?}: {error:?}",
                                peer_id.address().octets()
                            );
                            accepted = false;
                        }
                    }
                    let result = if accepted {
                        CBATTError::Success
                    } else {
                        CBATTError::InsufficientResources
                    };
                    // SAFETY: the request belongs to this live manager callback and is answered
                    // exactly once on this branch.
                    unsafe { peripheral.respondToRequest_withResult(&request, result) };
                    continue;
                }
                if let Some(control) = Control::decode(&bytes) {
                    let mut accepted = true;
                    if !self.ivars().sessions.borrow().contains_key(&peer_id) {
                        accepted = self.open_inbound(&central, peer_id, InboundProfile::Native);
                    }
                    if let Some(session) = self
                        .ivars()
                        .sessions
                        .borrow()
                        .get(&peer_id)
                        .filter(|session| session.protocol == PeerProtocol::Native)
                    {
                        accepted &= session.control_tx.try_send(control).is_ok();
                    }
                    if !accepted {
                        // SAFETY: the request belongs to this live manager callback and is answered
                        // exactly once on this branch.
                        unsafe {
                            peripheral.respondToRequest_withResult(
                                &request,
                                CBATTError::InsufficientResources,
                            )
                        };
                        continue;
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
            if !self.ivars().radio_enabled.load(Ordering::Acquire) {
                return;
            }
            let peer_id = core_bluetooth_peer_id(central);
            self.reap_closed_state_on_queue();
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
            self.reap_closed_state_on_queue();
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
                self.clear_peer_on_queue(peer_id);
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
        if !self.ivars().radio_enabled.load(Ordering::Acquire) {
            return false;
        }
        self.reap_closed_state_on_queue();
        has_session_for_peer(&self.ivars().sessions.borrow(), peer_id)
    }

    pub(super) fn new(
        manager_signals: ManagerSignalSender,
        inbound: tokio_mpsc::Sender<GattLink>,
        queue: DispatchRetained<DispatchQueue>,
        identity: BleIdentity,
        radio_enabled: Arc<AtomicBool>,
        max_peers: usize,
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
            manager_signals,
            inbound,
            characteristic: RefCell::new(characteristic),
            data_characteristic: RefCell::new(data_characteristic),
            columba_rx_characteristic: RefCell::new(columba_rx_characteristic),
            columba_tx_characteristic: RefCell::new(columba_tx_characteristic),
            columba_identity_characteristic: RefCell::new(columba_identity_characteristic),
            queue,
            manager: RefCell::new(None),
            radio_enabled,
            service_registration_requested: RefCell::new(false),
            l2cap_publication_requested: RefCell::new(false),
            session_capacity: peripheral_session_capacity(max_peers),
            pending_l2cap_capacity: pending_l2cap_capacity(max_peers),
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
    ) -> bool {
        if !self.ivars().radio_enabled.load(Ordering::Acquire) {
            return false;
        }
        self.reap_closed_state_on_queue();
        let session_count = self.ivars().sessions.borrow().len();
        if !can_open_inbound(session_count, self.ivars().session_capacity) {
            crate::diagnostic_log::warn!(
                "bluetooth: rejecting inbound control link from {:02x?} — peripheral-session capacity is full ({session_count}/{})",
                peer_id.address().octets(),
                self.ivars().session_capacity
            );
            return false;
        }
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
        let rejected = match try_bounded_ingress(&self.ivars().inbound, link) {
            BoundedIngress::Accepted => return true,
            BoundedIngress::Full(link) => {
                crate::diagnostic_log::warn!(
                    "bluetooth: rejecting inbound link from {:02x?} — inbound queue is full",
                    link.address.octets()
                );
                link
            }
            BoundedIngress::Closed(link) => {
                crate::diagnostic_log::debug!(
                    "bluetooth: rejecting inbound link from {:02x?} — backend is closed",
                    link.address.octets()
                );
                link
            }
        };
        // The session's data sender observes receiver closure only after the rejected link is
        // dropped. Remove the queue-confined session and any pending L2CAP state immediately
        // afterward by exact CoreBluetooth peer ID so a synthetic-address collision cannot tear
        // down an unrelated peer.
        drop(rejected);
        self.clear_peer_on_queue(peer_id);
        false
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
            if !this.0.ivars().radio_enabled.load(Ordering::Acquire) {
                return;
            }
            this.0.reap_closed_state_on_queue();
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
            if !this.0.ivars().radio_enabled.load(Ordering::Acquire) {
                return;
            }
            this.0.reap_closed_state_on_queue();
            let mut pending = this.0.ivars().pending_l2cap.borrow_mut();
            if !can_arm_l2cap(&pending, peer_id, this.0.ivars().pending_l2cap_capacity) {
                crate::diagnostic_log::warn!(
                    "bluetooth: refusing L2CAP waiter for {:02x?} — pending-peer capacity is full ({}/{})",
                    peer_id.address().octets(),
                    pending.len(),
                    this.0.ivars().pending_l2cap_capacity
                );
                return;
            }
            let entry = pending.entry(peer_id).or_default();
            if !entry.arm(tx) {
                crate::diagnostic_log::warn!(
                    "bluetooth: refusing L2CAP waiter for {:02x?} — this exact peer's waiter capacity is full",
                    peer_id.address().octets()
                );
            }
            if entry.is_empty() {
                pending.remove(&peer_id);
            }
        });
    }

    pub(super) fn set_advertising(&self, mode: AdvertisingMode) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            if mode.is_on() && !this.0.ivars().radio_enabled.load(Ordering::Acquire) {
                return;
            }
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

    /// Queue-confined logical radio transition. The published GATT service and L2CAP PSM remain
    /// available for re-enable, while disabling stops airtime and releases every live session,
    /// pending waiter, and buffered channel.
    pub(super) fn set_radio_enabled(&self, enabled: bool) {
        self.ivars().radio_enabled.store(enabled, Ordering::Release);
        if enabled {
            return;
        }
        if let Some(manager) = self
            .ivars()
            .manager
            .borrow()
            .as_ref()
            .map(|manager| manager.0.clone())
        {
            // SAFETY: this method is called only on the peripheral manager's serial queue.
            unsafe { manager.stopAdvertising() };
        }
        self.ivars().sessions.borrow_mut().clear();
        self.ivars().pending_l2cap.borrow_mut().clear();
    }

    /// Queue-confined: call only from the CoreBluetooth serial dispatch queue.
    fn clear_closed_peer_on_queue(&self, address: BleAddress) {
        let mut sessions = self.ivars().sessions.borrow_mut();
        let mut pending = self.ivars().pending_l2cap.borrow_mut();
        reap_closed_sessions(
            &mut sessions,
            &mut pending,
            Some(address),
            PeripheralPeerSession::data_receiver_closed,
        );
        // Central-role waiters have no peripheral-role session. Reap only entries whose waiter or
        // delivered channel is independently known closed; never use the synthetic address as an
        // identity substitute.
        reap_stale_pending_l2cap(&mut pending);
    }

    /// Queue-confined: call only from the CoreBluetooth serial dispatch queue.
    fn reap_closed_state_on_queue(&self) {
        let mut sessions = self.ivars().sessions.borrow_mut();
        let mut pending = self.ivars().pending_l2cap.borrow_mut();
        reap_closed_sessions(
            &mut sessions,
            &mut pending,
            None,
            PeripheralPeerSession::data_receiver_closed,
        );
        reap_stale_pending_l2cap(&mut pending);
    }

    /// Queue-confined: remove only the state owned by this exact CoreBluetooth peer.
    fn clear_peer_on_queue(&self, peer_id: CoreBluetoothPeerId) {
        self.ivars().sessions.borrow_mut().remove(&peer_id);
        self.ivars().pending_l2cap.borrow_mut().remove(&peer_id);
    }

    pub(super) fn clear_closed_peer(&self, address: BleAddress) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            this.0.clear_closed_peer_on_queue(address);
        });
    }
}
