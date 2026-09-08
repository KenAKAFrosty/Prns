use core::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_bluetooth::{
    CBCentralManager, CBCentralManagerDelegate, CBCentralManagerRestoredStatePeripheralsKey,
    CBCharacteristic, CBManagerState, CBPeripheral, CBPeripheralDelegate, CBService,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use prns_core::interfaces::bluetooth_auto::{BleAddress, BleIdentity, Control, PeerProtocol};

use super::discovery::{
    advertisement_candidate_strength, discover_disposition, DiscoverDisposition, DiscoveryGuard,
    PeripheralLinkState, SessionPresence, StaleCancellation, StaleLinkRecovery,
};
use super::gatt_link::GattInboundSender;
use super::gatt_write::{
    write_admission, GattWriteAdmission, GattWriteMode, GattWriteRequest, GattWriteTarget,
    PendingAcknowledgedWrite,
};
use super::{
    cbuuid_eq, columba_identity_uuid, columba_rx_uuid, columba_tx_uuid, control_uuid,
    core_bluetooth_peer_id, data_uuid, service_uuid, CoreBluetoothPeerId, Event, MacosBleError,
    PeripheralTable, RestoredPeripherals, SendCharacteristicRef, SendPeripheral,
};

pub(super) fn connected_peripheral(
    central: &CBCentralManager,
    peer_id: CoreBluetoothPeerId,
) -> Option<Retained<CBPeripheral>> {
    let uuid = service_uuid();
    let services = NSArray::from_slice(&[&*uuid]);
    // SAFETY: the live manager is queried on its serial dispatch queue and the retained service
    // UUID array remains alive for the synchronous CoreBluetooth call.
    let connected = unsafe { central.retrieveConnectedPeripheralsWithServices(&services) };
    connected
        .iter()
        .find(|peripheral| core_bluetooth_peer_id(peripheral) == peer_id)
        .map(|peripheral| peripheral.retain())
}

pub(super) fn is_system_connected(
    central: &CBCentralManager,
    peer_id: CoreBluetoothPeerId,
) -> bool {
    connected_peripheral(central, peer_id).is_some()
}

pub(super) fn cancel_system_connection(central: &CBCentralManager, peer_id: CoreBluetoothPeerId) {
    if let Some(peripheral) = connected_peripheral(central, peer_id) {
        // SAFETY: both retained objects remain alive through this call and are messaged only on
        // the CoreBluetooth serial dispatch queue.
        unsafe { central.cancelPeripheralConnection(&peripheral) };
    }
}

pub(super) struct DialChars {
    pub(super) peer_protocol: PeerProtocol,
    pub(super) peer_identity: Option<BleIdentity>,
    pub(super) control: SendCharacteristicRef,
    pub(super) data: Option<GattWriteTarget>,
}

pub(super) enum DialCompletion {
    Ready(DialChars),
    Failed,
    Rejected,
}

enum ColumbaReadiness {
    AwaitingIdentityAndSubscription,
    Subscribed,
    Identified(BleIdentity),
}

enum CentralProfile {
    Discovering,
    Native {
        data: Option<GattWriteTarget>,
    },
    Columba {
        write: GattWriteTarget,
        notify: SendCharacteristicRef,
        readiness: ColumbaReadiness,
    },
    Ready,
}

pub(super) struct CentralPeerSession {
    address: BleAddress,
    control_tx: tokio_mpsc::Sender<Control>,
    completion_tx: Option<oneshot::Sender<DialCompletion>>,
    data_tx: GattInboundSender,
    profile: CentralProfile,
    acknowledged_write: Option<PendingAcknowledgedWrite>,
    unacknowledged_write: Option<GattWriteRequest>,
}

impl CentralPeerSession {
    pub(super) fn new(
        address: BleAddress,
        control_tx: tokio_mpsc::Sender<Control>,
        completion_tx: oneshot::Sender<DialCompletion>,
        data_tx: GattInboundSender,
    ) -> Self {
        Self {
            address,
            control_tx,
            completion_tx: Some(completion_tx),
            data_tx,
            profile: CentralProfile::Discovering,
            acknowledged_write: None,
            unacknowledged_write: None,
        }
    }

    fn select_native(&mut self, data: Option<GattWriteTarget>) {
        self.profile = CentralProfile::Native { data };
    }

    fn select_columba(&mut self, write: GattWriteTarget, notify: SendCharacteristicRef) {
        self.profile = CentralProfile::Columba {
            write,
            notify,
            readiness: ColumbaReadiness::AwaitingIdentityAndSubscription,
        };
    }

    fn native_ready(&mut self, control: SendCharacteristicRef) {
        let profile = core::mem::replace(&mut self.profile, CentralProfile::Ready);
        let CentralProfile::Native { data } = profile else {
            self.profile = profile;
            return;
        };
        self.complete(DialChars {
            peer_protocol: PeerProtocol::Native,
            peer_identity: None,
            control,
            data,
        });
    }

    fn columba_identity(&mut self, identity: BleIdentity) {
        let profile = core::mem::replace(&mut self.profile, CentralProfile::Ready);
        let CentralProfile::Columba {
            write,
            notify,
            readiness,
        } = profile
        else {
            self.profile = profile;
            return;
        };
        match readiness {
            ColumbaReadiness::AwaitingIdentityAndSubscription | ColumbaReadiness::Identified(_) => {
                self.profile = CentralProfile::Columba {
                    write,
                    notify,
                    readiness: ColumbaReadiness::Identified(identity),
                };
            }
            ColumbaReadiness::Subscribed => {
                self.complete_columba(write, notify, identity);
            }
        }
    }

    fn columba_subscribed(&mut self) {
        let profile = core::mem::replace(&mut self.profile, CentralProfile::Ready);
        let CentralProfile::Columba {
            write,
            notify,
            readiness,
        } = profile
        else {
            self.profile = profile;
            return;
        };
        match readiness {
            ColumbaReadiness::AwaitingIdentityAndSubscription | ColumbaReadiness::Subscribed => {
                self.profile = CentralProfile::Columba {
                    write,
                    notify,
                    readiness: ColumbaReadiness::Subscribed,
                };
            }
            ColumbaReadiness::Identified(identity) => {
                self.complete_columba(write, notify, identity);
            }
        }
    }

    fn complete_columba(
        &mut self,
        write: GattWriteTarget,
        _notify: SendCharacteristicRef,
        identity: BleIdentity,
    ) {
        let control = SendCharacteristicRef(write.characteristic.0.clone());
        self.complete(DialChars {
            peer_protocol: PeerProtocol::Columba,
            peer_identity: Some(identity),
            data: Some(write),
            control,
        });
    }

    fn complete(&mut self, chars: DialChars) {
        self.profile = CentralProfile::Ready;
        if let Some(completion_tx) = self.completion_tx.take() {
            let _ = completion_tx.send(DialCompletion::Ready(chars));
        }
    }

    fn fail(mut self) {
        if let Some(completion_tx) = self.completion_tx.take() {
            let _ = completion_tx.send(DialCompletion::Failed);
        }
    }

    pub(super) fn reject(mut self) {
        if let Some(completion_tx) = self.completion_tx.take() {
            let _ = completion_tx.send(DialCompletion::Rejected);
        }
    }

    #[cfg(test)]
    pub(super) fn data_receiver_closed(&self) -> bool {
        self.data_tx.is_closed()
    }

    fn begin_acknowledged_write(
        &mut self,
        pending: PendingAcknowledgedWrite,
    ) -> Result<(), PendingAcknowledgedWrite> {
        if self.acknowledged_write.is_some() {
            return Err(pending);
        }
        self.acknowledged_write = Some(pending);
        Ok(())
    }

    fn finish_acknowledged_write(
        &mut self,
        characteristic: &CBCharacteristic,
        result: Result<(), MacosBleError>,
    ) -> bool {
        let Some(pending) = self.acknowledged_write.as_ref() else {
            return false;
        };
        if !core::ptr::eq(&*pending.characteristic.0, characteristic) {
            return false;
        }
        if let Some(pending) = self.acknowledged_write.take() {
            pending.complete(result);
        }
        true
    }

    fn hold_unacknowledged_write(
        &mut self,
        request: GattWriteRequest,
    ) -> Result<(), GattWriteRequest> {
        if self.unacknowledged_write.is_some() {
            return Err(request);
        }
        self.unacknowledged_write = Some(request);
        Ok(())
    }
}

pub(super) struct DialCommand {
    pub(super) central: Retained<CBCentralManager>,
    pub(super) delegate: Retained<CentralDelegate>,
    pub(super) peripheral: Retained<CBPeripheral>,
    pub(super) peer_id: CoreBluetoothPeerId,
    pub(super) session: CentralPeerSession,
}
// SAFETY: every retained CoreBluetooth object in the command is transferred to and consumed on
// the single serial CoreBluetooth dispatch queue; the embedded session is Send.
unsafe impl Send for DialCommand {}

pub(super) struct CentralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
    peripherals: PeripheralTable,
    restored: RestoredPeripherals,
    scan_activity: Arc<AtomicBool>,
    sessions: RefCell<HashMap<CoreBluetoothPeerId, CentralPeerSession>>,
    discovery_guard: RefCell<DiscoveryGuard>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = CentralDelegateIvars]
    pub(super) struct CentralDelegate;

    unsafe impl NSObjectProtocol for CentralDelegate {}

    unsafe impl CBCentralManagerDelegate for CentralDelegate {
        #[unsafe(method(centralManagerDidUpdateState:))]
        fn did_update_state(&self, central: &CBCentralManager) {
            // SAFETY: CoreBluetooth supplied this live manager to its delegate on the configured
            // serial dispatch queue.
            if unsafe { central.state() } == CBManagerState::PoweredOn {
                let _ = self.ivars().events.send(Event::CentralPowered);
            }
        }

        #[unsafe(method(centralManager:willRestoreState:))]
        fn will_restore_state(
            &self,
            _central: &CBCentralManager,
            dict: &NSDictionary<NSString, AnyObject>,
        ) {
            // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
            let key: &NSString = unsafe { CBCentralManagerRestoredStatePeripheralsKey };
            let Some(restored) = dict.objectForKey(key) else {
                return;
            };
            // SAFETY: CoreBluetooth documents this restoration dictionary value as an NSArray of
            // CBPeripheral objects; `restored` retains the array for the duration of the borrow.
            let peripherals: &NSArray<CBPeripheral> =
                unsafe { &*(Retained::as_ptr(&restored) as *const NSArray<CBPeripheral>) };
            for peripheral in peripherals.iter() {
                let peer_id = core_bluetooth_peer_id(&peripheral);
                let address = peer_id.address();
                crate::diagnostic_log::debug!(
                    "bluetooth: restored peripheral {:02x?} from a background relaunch — re-adopting",
                    address.octets()
                );
                if let Ok(mut map) = self.ivars().peripherals.lock() {
                    map.insert(peer_id, (SendPeripheral(peripheral.retain()), None));
                }
                if let Ok(mut queue) = self.ivars().restored.lock() {
                    queue.push_back(peer_id);
                }
            }
        }

        #[unsafe(method(centralManager:didDiscoverPeripheral:advertisementData:RSSI:))]
        fn did_discover(
            &self,
            central: &CBCentralManager,
            peripheral: &CBPeripheral,
            advertisement_data: &NSDictionary<NSString, AnyObject>,
            rssi: &NSNumber,
        ) {
            self.ivars().scan_activity.store(true, Ordering::Relaxed);
            let peer_id = core_bluetooth_peer_id(peripheral);
            let now = Instant::now();
            let strength = advertisement_candidate_strength(advertisement_data);
            if cfg!(target_os = "macos")
                && !self
                    .ivars()
                    .discovery_guard
                    .borrow_mut()
                    .admit_candidate(peer_id, strength, now)
            {
                return;
            }
            // SAFETY: CoreBluetooth supplied this live peripheral to its delegate on the manager
            // queue, so querying immutable framework state is valid.
            let state = PeripheralLinkState::from(unsafe { peripheral.state() });
            let session = if self.ivars().sessions.borrow().contains_key(&peer_id) {
                SessionPresence::Present
            } else {
                SessionPresence::Absent
            };
            let cancellation = if self
                .ivars()
                .discovery_guard
                .borrow_mut()
                .cancellation_recent(peer_id, now)
            {
                StaleCancellation::InFlight
            } else {
                StaleCancellation::Idle
            };
            let recovery = if cfg!(target_os = "macos") {
                StaleLinkRecovery::Enabled
            } else {
                StaleLinkRecovery::Disabled
            };
            let disposition = discover_disposition(state, session, cancellation, recovery);
            match disposition {
                DiscoverDisposition::IgnoreOwned | DiscoverDisposition::WaitForDisconnect => {
                    return;
                }
                DiscoverDisposition::CancelStale => {
                    self.ivars()
                        .discovery_guard
                        .borrow_mut()
                        .record_stale_cancellation(peer_id, now);
                    crate::diagnostic_log::debug!(
                        "bluetooth: cancelling stale CoreBluetooth link to {:02x?} after Prns session closed",
                        peer_id.address().octets()
                    );
                    // SAFETY: CoreBluetooth supplied both live objects on the central manager's
                    // serial queue and this app is cancelling only its local connection request.
                    unsafe { central.cancelPeripheralConnection(peripheral) };
                    return;
                }
                DiscoverDisposition::Adopt => {}
            }
            let dbm = rssi.integerValue();
            let rssi = if dbm == 127 {
                None
            } else {
                i8::try_from(dbm).ok()
            };
            let address = peer_id.address();
            if let Ok(mut map) = self.ivars().peripherals.lock() {
                map.insert(peer_id, (SendPeripheral(peripheral.retain()), rssi));
            }
            let _ = self.ivars().events.send(Event::Sighting { address, rssi });
        }

        #[unsafe(method(centralManager:didConnectPeripheral:))]
        fn did_connect(&self, _central: &CBCentralManager, peripheral: &CBPeripheral) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if !self.ivars().sessions.borrow().contains_key(&peer_id) {
                return;
            }
            crate::diagnostic_log::debug!(
                "bluetooth: dial connected over LE, discovering Prns service"
            );
            let uuid = service_uuid();
            let services = NSArray::from_slice(&[&*uuid]);
            // SAFETY: `peripheral` is live for this callback and `services` is a correctly typed,
            // retained NSArray for the duration of the Objective-C message.
            unsafe { peripheral.discoverServices(Some(&services)) };
        }

        #[unsafe(method(centralManager:didFailToConnectPeripheral:error:))]
        fn did_fail_to_connect(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            crate::diagnostic_log::warn!("bluetooth: dial connect FAILED: {error:?}");
            self.fail_peer(core_bluetooth_peer_id(peripheral));
        }

        #[unsafe(method(centralManager:didDisconnectPeripheral:error:))]
        fn did_disconnect(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            self.ivars()
                .discovery_guard
                .borrow_mut()
                .clear_stale_cancellation(peer_id);
            crate::diagnostic_log::warn!("bluetooth: dialed peripheral disconnected: {error:?}");
            self.fail_peer(peer_id);
        }
    }

    unsafe impl CBPeripheralDelegate for CentralDelegate {
        #[unsafe(method(peripheral:didDiscoverServices:))]
        fn did_discover_services(&self, peripheral: &CBPeripheral, error: Option<&NSError>) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if let Some(error) = error {
                crate::diagnostic_log::warn!("bluetooth: service discovery FAILED: {error:?}");
                self.fail_peer(peer_id);
                return;
            }
            let expected_service_id = service_uuid();
            // SAFETY: the callback occurs only after CoreBluetooth completed service discovery;
            // the peripheral retains the returned service array and each service retains its UUID.
            let service = unsafe { peripheral.services() }.and_then(|services| {
                services.iter().find(|service| {
                    // SAFETY: `service` remains retained by the array for this comparison.
                    let uuid = unsafe { service.UUID() };
                    cbuuid_eq(&uuid, &expected_service_id)
                })
            });
            let Some(service) = service else {
                if cfg!(target_os = "macos") {
                    self.ivars()
                        .discovery_guard
                        .borrow_mut()
                        .record_service_miss(peer_id, Instant::now());
                }
                crate::diagnostic_log::warn!(
                    "bluetooth: no Prns service on peripheral — dropping dial and backing off repeated weak candidates"
                );
                self.fail_peer(peer_id);
                return;
            };
            // SAFETY: `service` belongs to this live peripheral and both are retained throughout
            // the discovery message.
            unsafe { peripheral.discoverCharacteristics_forService(None, &service) };
        }

        #[unsafe(method(peripheral:didDiscoverCharacteristicsForService:error:))]
        fn did_discover_characteristics(
            &self,
            peripheral: &CBPeripheral,
            service: &CBService,
            error: Option<&NSError>,
        ) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if let Some(error) = error {
                crate::diagnostic_log::warn!(
                    "bluetooth: characteristic discovery FAILED: {error:?}"
                );
                self.fail_peer(peer_id);
                return;
            }
            // SAFETY: this delegate callback follows characteristic discovery and CoreBluetooth
            // retains the characteristic collection on the live service.
            let Some(characteristics) = (unsafe { service.characteristics() }) else {
                crate::diagnostic_log::warn!(
                    "bluetooth: no characteristics on Prns service — dropping dial"
                );
                self.fail_peer(peer_id);
                return;
            };
            let control_id = control_uuid();
            let data_id = data_uuid();
            let columba_rx_id = columba_rx_uuid();
            let columba_tx_id = columba_tx_uuid();
            let columba_identity_id = columba_identity_uuid();
            let mut control = None;
            let mut data = None;
            let mut columba_rx = None;
            let mut columba_tx = None;
            let mut columba_identity = None;
            for characteristic in characteristics.iter() {
                // SAFETY: the characteristic is retained by the framework collection during this
                // iteration and UUID is a CoreBluetooth-owned immutable property.
                let uuid = unsafe { characteristic.UUID() };
                if cbuuid_eq(&uuid, &control_id) {
                    control = Some(characteristic);
                } else if cbuuid_eq(&uuid, &data_id) {
                    data = Some(characteristic);
                } else if cbuuid_eq(&uuid, &columba_rx_id) {
                    columba_rx = Some(characteristic);
                } else if cbuuid_eq(&uuid, &columba_tx_id) {
                    columba_tx = Some(characteristic);
                } else if cbuuid_eq(&uuid, &columba_identity_id) {
                    columba_identity = Some(characteristic);
                }
            }
            if let Some(control) = control {
                let data_target = match data.as_deref() {
                    Some(data) => match GattWriteTarget::discover(
                        peripheral,
                        data,
                        GattWriteMode::WithResponse,
                    ) {
                        Ok(target) => Some(target),
                        Err(error) => {
                            crate::diagnostic_log::warn!(
                                "bluetooth: native DATA characteristic cannot be written safely: {error:?}"
                            );
                            self.fail_peer(peer_id);
                            return;
                        }
                    },
                    None => None,
                };
                let Some(()) = self
                    .ivars()
                    .sessions
                    .borrow_mut()
                    .get_mut(&peer_id)
                    .map(|session| session.select_native(data_target))
                else {
                    return;
                };
                if let Some(data) = data {
                    // SAFETY: this characteristic was discovered on `peripheral`; both remain live
                    // through the subscription message on the serial manager queue.
                    unsafe { peripheral.setNotifyValue_forCharacteristic(true, &data) };
                }
                crate::diagnostic_log::debug!(
                    "bluetooth: native control characteristic found, subscribing"
                );
                // SAFETY: `control` was discovered on `peripheral` and both objects are retained
                // throughout this queue-confined subscription call.
                unsafe { peripheral.setNotifyValue_forCharacteristic(true, &control) };
                return;
            }
            let (Some(rx), Some(tx), Some(identity)) = (columba_rx, columba_tx, columba_identity)
            else {
                crate::diagnostic_log::warn!(
                    "bluetooth: peer exposes neither a complete native nor Columba profile"
                );
                self.fail_peer(peer_id);
                return;
            };
            let write_target =
                match GattWriteTarget::discover(peripheral, &rx, GattWriteMode::WithoutResponse) {
                    Ok(target) => target,
                    Err(error) => {
                        crate::diagnostic_log::warn!(
                        "bluetooth: Columba RX characteristic cannot be written safely: {error:?}"
                    );
                        self.fail_peer(peer_id);
                        return;
                    }
                };
            let Some(()) = self
                .ivars()
                .sessions
                .borrow_mut()
                .get_mut(&peer_id)
                .map(|session| {
                    session.select_columba(write_target, SendCharacteristicRef(tx.retain()));
                })
            else {
                return;
            };
            // SAFETY: the identity and transmit characteristics were discovered on this retained
            // peripheral, and both messages execute on its CoreBluetooth dispatch queue.
            unsafe {
                peripheral.readValueForCharacteristic(&identity);
                peripheral.setNotifyValue_forCharacteristic(true, &tx);
            }
        }

        #[unsafe(method(peripheral:didUpdateNotificationStateForCharacteristic:error:))]
        fn did_update_notification_state(
            &self,
            peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if let Some(error) = error {
                crate::diagnostic_log::warn!("bluetooth: subscribe FAILED: {error:?}");
                self.fail_peer(peer_id);
                return;
            }
            // SAFETY: CoreBluetooth supplied a live characteristic to this delegate callback.
            let subscribed_uuid = unsafe { characteristic.UUID() };
            if cbuuid_eq(&subscribed_uuid, &control_uuid()) {
                let mut sessions = self.ivars().sessions.borrow_mut();
                let Some(session) = sessions.get_mut(&peer_id) else {
                    return;
                };
                crate::diagnostic_log::debug!(
                    "bluetooth: {:02x?} subscribed — native control ready",
                    session.address.octets()
                );
                session.native_ready(SendCharacteristicRef(characteristic.retain()));
                return;
            }
            if !cbuuid_eq(&subscribed_uuid, &columba_tx_uuid()) {
                return;
            }
            let mut sessions = self.ivars().sessions.borrow_mut();
            let Some(session) = sessions.get_mut(&peer_id) else {
                return;
            };
            crate::diagnostic_log::debug!(
                "bluetooth: {:02x?} subscribed — Columba data path ready",
                session.address.octets()
            );
            session.columba_subscribed();
        }

        #[unsafe(method(peripheral:didUpdateValueForCharacteristic:error:))]
        fn did_update_value(
            &self,
            peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if let Some(error) = error {
                crate::diagnostic_log::warn!("bluetooth: characteristic update FAILED: {error:?}");
                self.fail_peer(peer_id);
                return;
            }
            // SAFETY: CoreBluetooth supplied a live characteristic whose value is retained for the
            // duration of this value-update callback.
            let Some(value) = (unsafe { characteristic.value() }) else {
                return;
            };
            // SAFETY: CoreBluetooth supplied a live characteristic to this delegate callback.
            let updated_uuid = unsafe { characteristic.UUID() };
            if cbuuid_eq(&updated_uuid, &data_uuid())
                || cbuuid_eq(&updated_uuid, &columba_tx_uuid())
            {
                let enqueue_error =
                    self.ivars()
                        .sessions
                        .borrow()
                        .get(&peer_id)
                        .and_then(|session| {
                            session
                                .data_tx
                                .try_send(Box::from(&value.to_vec()[..]))
                                .err()
                        });
                if let Some(error) = enqueue_error {
                    crate::diagnostic_log::warn!(
                        "bluetooth: GATT notification inbox failed for {:02x?}: {error:?}",
                        peer_id.address().octets()
                    );
                    self.fail_peer(peer_id);
                }
                return;
            }
            if cbuuid_eq(&updated_uuid, &columba_identity_uuid()) {
                let bytes = value.to_vec();
                let Ok(identity) = <[u8; 16]>::try_from(bytes.as_slice()) else {
                    self.fail_peer(peer_id);
                    return;
                };
                let mut sessions = self.ivars().sessions.borrow_mut();
                let Some(session) = sessions.get_mut(&peer_id) else {
                    return;
                };
                session.columba_identity(BleIdentity::new(identity));
                return;
            }
            let Some(control) = Control::decode(&value.to_vec()) else {
                return;
            };
            if let Some(session) = self.ivars().sessions.borrow().get(&peer_id) {
                let _ = session.control_tx.try_send(control);
            }
        }

        #[unsafe(method(peripheral:didWriteValueForCharacteristic:error:))]
        fn did_write_value(
            &self,
            peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            let result = if let Some(error) = error {
                crate::diagnostic_log::warn!(
                    "bluetooth: acknowledged GATT write FAILED for {:02x?}: {error:?}",
                    peer_id.address().octets()
                );
                Err(MacosBleError::GattWriteFailed)
            } else {
                Ok(())
            };
            let completed = self
                .ivars()
                .sessions
                .borrow_mut()
                .get_mut(&peer_id)
                .is_some_and(|session| session.finish_acknowledged_write(characteristic, result));
            if !completed {
                crate::diagnostic_log::warn!(
                    "bluetooth: unexpected acknowledged-write callback for {:02x?}",
                    peer_id.address().octets()
                );
            }
        }

        #[unsafe(method(peripheralIsReadyToSendWriteWithoutResponse:))]
        fn is_ready_to_write_without_response(&self, peripheral: &CBPeripheral) {
            self.drain_unacknowledged_write(peripheral);
        }
    }
);

impl CentralDelegate {
    pub(super) fn new(
        events: tokio_mpsc::UnboundedSender<Event>,
        peripherals: PeripheralTable,
        restored: RestoredPeripherals,
        scan_activity: Arc<AtomicBool>,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CentralDelegateIvars {
            events,
            peripherals,
            restored,
            scan_activity,
            sessions: RefCell::new(HashMap::new()),
            discovery_guard: RefCell::new(DiscoveryGuard::default()),
        });
        // SAFETY: `this` is a freshly allocated CentralDelegate with fully initialized ivars;
        // forwarding to NSObject's designated initializer preserves its allocation identity.
        unsafe { msg_send![super(this), init] }
    }

    pub(super) fn begin_session(
        &self,
        peer_id: CoreBluetoothPeerId,
        session: CentralPeerSession,
    ) -> bool {
        if self.has_session(peer_id) {
            session.reject();
            return false;
        }
        self.ivars().sessions.borrow_mut().insert(peer_id, session);
        true
    }

    pub(super) fn has_session(&self, peer_id: CoreBluetoothPeerId) -> bool {
        self.ivars().sessions.borrow().contains_key(&peer_id)
    }

    pub(super) fn note_stale_cancellation(&self, peer_id: CoreBluetoothPeerId) {
        self.ivars()
            .discovery_guard
            .borrow_mut()
            .record_stale_cancellation(peer_id, Instant::now());
    }

    pub(super) fn remove_session(&self, peer_id: CoreBluetoothPeerId) {
        if let Some(session) = self.ivars().sessions.borrow_mut().remove(&peer_id) {
            session.fail();
        }
    }

    pub(super) fn submit_write(
        &self,
        peripheral: &CBPeripheral,
        peer_id: CoreBluetoothPeerId,
        request: GattWriteRequest,
    ) {
        let mut sessions = self.ivars().sessions.borrow_mut();
        let Some(session) = sessions.get_mut(&peer_id) else {
            request.complete(Err(MacosBleError::Closed));
            return;
        };
        let can_send_without_response = request.mode == GattWriteMode::WithoutResponse
            // SAFETY: this connection state query runs on the peripheral's CoreBluetooth queue
            // and does not mutate the retained peripheral.
            && unsafe { peripheral.canSendWriteWithoutResponse() };
        match write_admission(
            request.mode,
            session.acknowledged_write.is_some(),
            session.unacknowledged_write.is_some(),
            can_send_without_response,
        ) {
            GattWriteAdmission::Issue if request.mode == GattWriteMode::WithResponse => {
                let data = NSData::with_bytes(&request.bytes);
                let characteristic = request.characteristic.0.clone();
                let pending = request.into_acknowledged();
                if let Err(pending) = session.begin_acknowledged_write(pending) {
                    pending.complete(Err(MacosBleError::QueueFull));
                    return;
                }
                // SAFETY: the write is issued on the peripheral's CoreBluetooth queue, and the
                // retained characteristic and NSData remain live for the synchronous message.
                unsafe {
                    peripheral.writeValue_forCharacteristic_type(
                        &data,
                        &characteristic,
                        GattWriteMode::WithResponse.core_bluetooth_type(),
                    )
                };
            }
            GattWriteAdmission::Issue => issue_unacknowledged_write(peripheral, request),
            GattWriteAdmission::WaitForCapacity => {
                if let Err(request) = session.hold_unacknowledged_write(request) {
                    request.complete(Err(MacosBleError::QueueFull));
                }
            }
            GattWriteAdmission::Busy => request.complete(Err(MacosBleError::QueueFull)),
        }
    }

    fn drain_unacknowledged_write(&self, peripheral: &CBPeripheral) {
        let peer_id = core_bluetooth_peer_id(peripheral);
        let request = self
            .ivars()
            .sessions
            .borrow_mut()
            .get_mut(&peer_id)
            .and_then(|session| session.unacknowledged_write.take());
        let Some(request) = request else {
            return;
        };
        if request.receiver_closed() {
            return;
        }
        // SAFETY: the readiness callback and this authoritative re-check both execute on the
        // peripheral's CoreBluetooth queue.
        if !unsafe { peripheral.canSendWriteWithoutResponse() } {
            if let Some(session) = self.ivars().sessions.borrow_mut().get_mut(&peer_id) {
                if let Err(request) = session.hold_unacknowledged_write(request) {
                    request.complete(Err(MacosBleError::QueueFull));
                }
            }
            return;
        }
        issue_unacknowledged_write(peripheral, request);
    }

    fn fail_peer(&self, peer_id: CoreBluetoothPeerId) {
        self.remove_session(peer_id);
    }
}

fn issue_unacknowledged_write(peripheral: &CBPeripheral, request: GattWriteRequest) {
    let data = NSData::with_bytes(&request.bytes);
    // SAFETY: the write is issued on the peripheral's CoreBluetooth queue; the request retains its
    // discovered characteristic and NSData remains live for the synchronous message.
    unsafe {
        peripheral.writeValue_forCharacteristic_type(
            &data,
            &request.characteristic.0,
            GattWriteMode::WithoutResponse.core_bluetooth_type(),
        )
    };
    request.complete(Ok(()));
}
