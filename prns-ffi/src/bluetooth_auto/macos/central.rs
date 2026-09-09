use core::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
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
use super::gatt_link::{GattInboundSendError, GattInboundSender, GATT_INBOUND_BUDGET_BYTES};
use super::gatt_write::{
    write_admission, GattWriteAdmission, GattWriteMode, GattWriteRequest, GattWriteTarget,
    PendingAcknowledgedWrite,
};
use super::{
    cbuuid_eq, columba_identity_uuid, columba_rx_uuid, columba_tx_uuid, control_uuid,
    core_bluetooth_peer_id, data_uuid, service_uuid, CoreBluetoothPeerId, MacosBleError,
    ManagerSignalSender, SendCentralManager, SendCharacteristicRef, SendPeripheral, Sighting,
};

pub(super) fn is_system_connected(
    central: &CBCentralManager,
    peer_id: CoreBluetoothPeerId,
) -> bool {
    let uuid = service_uuid();
    let services = NSArray::from_slice(&[&*uuid]);
    // SAFETY: the live manager is queried on its serial dispatch queue and the retained service
    // UUID array remains alive for the synchronous CoreBluetooth call.
    let connected = unsafe { central.retrieveConnectedPeripheralsWithServices(&services) };
    connected
        .iter()
        .any(|peripheral| core_bluetooth_peer_id(&peripheral) == peer_id)
}

pub(super) fn discover_prns_services(peripheral: &CBPeripheral) {
    let uuid = service_uuid();
    let services = NSArray::from_slice(&[&*uuid]);
    // SAFETY: callers hold a live peripheral on the CoreBluetooth serial dispatch queue, and
    // `services` is a correctly typed, retained NSArray for the duration of the message.
    unsafe { peripheral.discoverServices(Some(&services)) };
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
    Rejected(DialRejection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DialRejection {
    YieldToSystemConnection,
    YieldToInboundSession,
    RestoredConnectionUnavailable,
    ExistingCentralSession,
    RadioOff,
}

pub(super) const CENTRAL_CONTROL_INBOUND_CAPACITY: usize = 8;

/// Value notifications received after CoreBluetooth restores a peripheral but before the Host
/// admits its central-role session. A buffer overflow marks the handoff failed: silently skipping
/// a control message or framed-data fragment could corrupt the restored protocol stream.
#[derive(Default)]
pub(super) struct RestoredCallbackBuffer {
    controls: VecDeque<Control>,
    data: VecDeque<Box<[u8]>>,
    charged_data_bytes: usize,
    failed: bool,
}

impl RestoredCallbackBuffer {
    pub(super) fn buffer_control(&mut self, control: Control) -> bool {
        if self.failed || self.controls.len() >= CENTRAL_CONTROL_INBOUND_CAPACITY {
            self.fail();
            return false;
        }
        self.controls.push_back(control);
        true
    }

    pub(super) fn buffer_data(&mut self, data: Box<[u8]>) -> bool {
        if self.failed {
            return false;
        }
        let charge = data.len().max(1);
        let Some(charged_data_bytes) = self.charged_data_bytes.checked_add(charge) else {
            self.fail();
            return false;
        };
        if charged_data_bytes > GATT_INBOUND_BUDGET_BYTES {
            self.fail();
            return false;
        }
        self.charged_data_bytes = charged_data_bytes;
        self.data.push_back(data);
        true
    }

    fn fail(&mut self) {
        self.failed = true;
        self.controls.clear();
        self.data.clear();
        self.charged_data_bytes = 0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RestoredAdmission {
    Admitted,
    Updated,
    AlreadyOwned,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestoredPhase {
    Queued,
    Offered,
    Claimed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CentralPeripheralState {
    Observed,
    Restored,
    Dialing,
    Active,
}

struct CentralPeripheral<P> {
    peer_id: CoreBluetoothPeerId,
    address: BleAddress,
    peripheral: P,
    rssi: Option<i8>,
    state: CentralPeripheralState,
    pending_sighting: bool,
}

struct RestoredPeer {
    peer_id: CoreBluetoothPeerId,
    phase: RestoredPhase,
    callbacks: RestoredCallbackBuffer,
}

pub(super) enum CentralDialCandidate<P> {
    Ready {
        peer_id: CoreBluetoothPeerId,
        peripheral: P,
        rssi: Option<i8>,
        restored: bool,
    },
    Busy,
    Missing,
}

pub(super) enum RestoredBufferResult<T> {
    Buffered,
    Overflowed,
    AlreadyFailed,
    NotRestored(T),
}

/// Bounded central-role ownership confined to CoreBluetooth's serial queue. The async backend
/// requests handoffs through that queue instead of locking this state from callback threads.
///
/// Restored peers retain their admission slot while queued, offered, and claimed, so popping a
/// synthetic sighting never creates an unaccounted callback buffer. Candidate entries use a
/// larger, independently supplied capacity and evict only advisory observations; live, dialing,
/// and restored peers are never evicted.
pub(super) struct CentralPeerRegistry<P> {
    peripheral_capacity: usize,
    restored_capacity: usize,
    peripherals: VecDeque<CentralPeripheral<P>>,
    restored: VecDeque<RestoredPeer>,
}

impl<P> CentralPeerRegistry<P> {
    pub(super) fn new(peripheral_capacity: usize, restored_capacity: usize) -> Self {
        Self {
            peripheral_capacity,
            restored_capacity,
            peripherals: VecDeque::new(),
            restored: VecDeque::new(),
        }
    }

    pub(super) fn admit_restored(
        &mut self,
        peer_id: CoreBluetoothPeerId,
        peripheral: P,
    ) -> RestoredAdmission {
        if let Some(restored) = self
            .restored
            .iter()
            .find(|restored| restored.peer_id == peer_id)
        {
            if restored.phase != RestoredPhase::Queued {
                return RestoredAdmission::AlreadyOwned;
            }
            if let Some(entry) = self
                .peripherals
                .iter_mut()
                .find(|entry| entry.peer_id == peer_id)
            {
                entry.peripheral = peripheral;
                entry.rssi = None;
                return RestoredAdmission::Updated;
            }
            return RestoredAdmission::Full;
        }
        if self.restored.len() >= self.restored_capacity {
            return RestoredAdmission::Full;
        }

        if let Some(position) = self
            .peripherals
            .iter()
            .position(|entry| entry.peer_id == peer_id)
        {
            if self.peripherals[position].state != CentralPeripheralState::Observed {
                return RestoredAdmission::AlreadyOwned;
            }
            self.peripherals.remove(position);
        }

        let address = peer_id.address();
        if let Some(position) = self
            .peripherals
            .iter()
            .position(|entry| entry.address == address)
        {
            if self.peripherals[position].state != CentralPeripheralState::Observed {
                return RestoredAdmission::Full;
            }
            self.peripherals.remove(position);
        }
        if !self.make_peripheral_room() {
            return RestoredAdmission::Full;
        }

        self.peripherals.push_back(CentralPeripheral {
            peer_id,
            address,
            peripheral,
            rssi: None,
            state: CentralPeripheralState::Restored,
            pending_sighting: false,
        });
        self.restored.push_back(RestoredPeer {
            peer_id,
            phase: RestoredPhase::Queued,
            callbacks: RestoredCallbackBuffer::default(),
        });
        RestoredAdmission::Admitted
    }

    /// Updates the most recent handle and signal for one synthetic address. Address collisions are
    /// deterministic: an advisory observation replaces an older advisory observation, while a
    /// restored, dialing, or active owner rejects the colliding newcomer.
    pub(super) fn observe(
        &mut self,
        peer_id: CoreBluetoothPeerId,
        peripheral: P,
        rssi: Option<i8>,
    ) -> bool {
        if let Some(position) = self
            .peripherals
            .iter()
            .position(|entry| entry.peer_id == peer_id)
        {
            if self.peripherals[position].state != CentralPeripheralState::Observed {
                return false;
            }
            self.peripherals.remove(position);
        }

        let address = peer_id.address();
        if let Some(position) = self
            .peripherals
            .iter()
            .position(|entry| entry.address == address)
        {
            if self.peripherals[position].state != CentralPeripheralState::Observed {
                return false;
            }
            self.peripherals.remove(position);
        }
        if !self.make_peripheral_room() {
            return false;
        }
        self.peripherals.push_back(CentralPeripheral {
            peer_id,
            address,
            peripheral,
            rssi,
            state: CentralPeripheralState::Observed,
            pending_sighting: true,
        });
        true
    }

    fn make_peripheral_room(&mut self) -> bool {
        if self.peripheral_capacity == 0 {
            return false;
        }
        if self.peripherals.len() < self.peripheral_capacity {
            return true;
        }
        let Some(position) = self
            .peripherals
            .iter()
            .position(|entry| entry.state == CentralPeripheralState::Observed)
        else {
            return false;
        };
        self.peripherals.remove(position);
        true
    }

    pub(super) fn offer_restored(&mut self) -> Option<CoreBluetoothPeerId> {
        let peer = self
            .restored
            .iter_mut()
            .find(|peer| peer.phase == RestoredPhase::Queued)?;
        peer.phase = RestoredPhase::Offered;
        Some(peer.peer_id)
    }

    pub(super) fn claim(&mut self, address: BleAddress) -> CentralDialCandidate<P>
    where
        P: Clone,
    {
        let Some(position) = self
            .peripherals
            .iter()
            .position(|entry| entry.address == address)
        else {
            return CentralDialCandidate::Missing;
        };
        let peer_id = self.peripherals[position].peer_id;
        let restored = match self.peripherals[position].state {
            CentralPeripheralState::Observed => false,
            CentralPeripheralState::Restored => {
                let Some(restored) = self
                    .restored
                    .iter_mut()
                    .find(|restored| restored.peer_id == peer_id)
                else {
                    return CentralDialCandidate::Busy;
                };
                if restored.phase != RestoredPhase::Offered {
                    return CentralDialCandidate::Busy;
                }
                restored.phase = RestoredPhase::Claimed;
                true
            }
            CentralPeripheralState::Dialing | CentralPeripheralState::Active => {
                return CentralDialCandidate::Busy;
            }
        };
        self.peripherals[position].state = CentralPeripheralState::Dialing;
        self.peripherals[position].pending_sighting = false;
        CentralDialCandidate::Ready {
            peer_id,
            peripheral: self.peripherals[position].peripheral.clone(),
            rssi: self.peripherals[position].rssi,
            restored,
        }
    }

    pub(super) fn begin_session(
        &mut self,
        peer_id: CoreBluetoothPeerId,
    ) -> Result<Option<RestoredCallbackBuffer>, ()> {
        let Some(peripheral) = self
            .peripherals
            .iter_mut()
            .find(|entry| entry.peer_id == peer_id)
        else {
            return Err(());
        };
        if peripheral.state != CentralPeripheralState::Dialing {
            return Err(());
        }
        peripheral.state = CentralPeripheralState::Active;
        let Some(position) = self
            .restored
            .iter()
            .position(|peer| peer.peer_id == peer_id)
        else {
            return Ok(None);
        };
        if self.restored[position].phase != RestoredPhase::Claimed {
            peripheral.state = CentralPeripheralState::Dialing;
            return Err(());
        }
        let restored = self.restored.remove(position).ok_or(())?;
        Ok(Some(restored.callbacks))
    }

    pub(super) fn mark_active(&mut self, peer_id: CoreBluetoothPeerId) {
        if let Some(entry) = self
            .peripherals
            .iter_mut()
            .find(|entry| entry.peer_id == peer_id)
        {
            entry.state = CentralPeripheralState::Active;
        }
    }

    pub(super) fn remove_peer(&mut self, peer_id: CoreBluetoothPeerId) -> Option<P> {
        let peripheral = self
            .peripherals
            .iter()
            .position(|entry| entry.peer_id == peer_id)
            .and_then(|position| self.peripherals.remove(position))
            .map(|entry| entry.peripheral);
        self.restored.retain(|peer| peer.peer_id != peer_id);
        peripheral
    }

    /// Remove all registry state and return only peripherals for which this backend owns a
    /// restored, dialing, or active CoreBluetooth connection. Advisory observations are dropped
    /// without cancellation because no connection was started for them.
    pub(super) fn drain_owned(&mut self) -> Vec<P> {
        self.restored.clear();
        self.peripherals
            .drain(..)
            .filter_map(|entry| {
                (entry.state != CentralPeripheralState::Observed).then_some(entry.peripheral)
            })
            .collect()
    }

    pub(super) fn pop_sighting(&mut self) -> (Option<Sighting>, bool) {
        let sighting = self
            .peripherals
            .iter_mut()
            .find(|entry| entry.pending_sighting)
            .map(|entry| {
                entry.pending_sighting = false;
                Sighting {
                    address: entry.address,
                    rssi: entry.rssi,
                }
            });
        let more = self.peripherals.iter().any(|entry| entry.pending_sighting);
        (sighting, more)
    }

    pub(super) fn buffer_restored_data(
        &mut self,
        peer_id: CoreBluetoothPeerId,
        data: Box<[u8]>,
    ) -> RestoredBufferResult<Box<[u8]>> {
        let Some(restored) = self
            .restored
            .iter_mut()
            .find(|restored| restored.peer_id == peer_id)
        else {
            return RestoredBufferResult::NotRestored(data);
        };
        let was_failed = restored.callbacks.failed;
        if restored.callbacks.buffer_data(data) {
            RestoredBufferResult::Buffered
        } else if was_failed {
            RestoredBufferResult::AlreadyFailed
        } else {
            RestoredBufferResult::Overflowed
        }
    }

    pub(super) fn buffer_restored_control(
        &mut self,
        peer_id: CoreBluetoothPeerId,
        control: Control,
    ) -> RestoredBufferResult<Control> {
        let Some(restored) = self
            .restored
            .iter_mut()
            .find(|restored| restored.peer_id == peer_id)
        else {
            return RestoredBufferResult::NotRestored(control);
        };
        let was_failed = restored.callbacks.failed;
        if restored.callbacks.buffer_control(control) {
            RestoredBufferResult::Buffered
        } else if was_failed {
            RestoredBufferResult::AlreadyFailed
        } else {
            RestoredBufferResult::Overflowed
        }
    }

    #[cfg(test)]
    pub(super) fn peripheral_len(&self) -> usize {
        self.peripherals.len()
    }

    #[cfg(test)]
    pub(super) fn restored_len(&self) -> usize {
        self.restored.len()
    }
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

// CoreBluetooth can preserve GAP while the process-owned Prns handshake and
// framing state are lost. Once the native profile is identified, replace that
// connection exactly once under the original dial reservation and deadline.
// Columba and non-iOS restoration keep their existing callback handoff.
#[derive(Default)]
enum RestorationRecovery {
    #[default]
    NotRequired,
    InspectingProfile(RestoredCallbackBuffer),
    Disconnecting,
    Connecting,
    DiscoveringServices,
    DiscoveringCharacteristics,
    Subscribing,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RestorationProfileAction {
    Continue,
    Disconnect,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RestorationDisconnectAction {
    Reconnect,
    Ignore,
    Close,
}

pub(super) struct CentralPeerSession {
    address: BleAddress,
    control_tx: tokio_mpsc::Sender<Control>,
    completion_tx: Option<oneshot::Sender<DialCompletion>>,
    data_tx: GattInboundSender,
    profile: CentralProfile,
    restoration: RestorationRecovery,
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
            restoration: RestorationRecovery::NotRequired,
            acknowledged_write: None,
            unacknowledged_write: None,
        }
    }

    pub(super) fn configure_restoration_recovery(&mut self, ios: bool, initially_connected: bool) {
        if ios && initially_connected {
            self.restoration =
                RestorationRecovery::InspectingProfile(RestoredCallbackBuffer::default());
        }
    }

    pub(super) fn restoration_profile(
        &mut self,
        protocol: PeerProtocol,
    ) -> Result<RestorationProfileAction, ()> {
        if matches!(
            self.restoration,
            RestorationRecovery::InspectingProfile(_)
                | RestorationRecovery::DiscoveringCharacteristics
        ) && !self.live_dial()
        {
            return Err(());
        }
        match self.restoration {
            RestorationRecovery::InspectingProfile(_) => {
                let previous = core::mem::replace(
                    &mut self.restoration,
                    if protocol == PeerProtocol::Native {
                        RestorationRecovery::Disconnecting
                    } else {
                        RestorationRecovery::NotRequired
                    },
                );
                let RestorationRecovery::InspectingProfile(callbacks) = previous else {
                    return Err(());
                };
                if protocol == PeerProtocol::Native {
                    // Neither a stale Welcome nor partial old DATA belongs to the
                    // fresh protocol incarnation. Drop both, without replaying.
                    Ok(RestorationProfileAction::Disconnect)
                } else if self.restore_callbacks(callbacks) {
                    Ok(RestorationProfileAction::Continue)
                } else {
                    Err(())
                }
            }
            RestorationRecovery::DiscoveringCharacteristics => {
                if protocol != PeerProtocol::Native {
                    return Err(());
                }
                self.restoration = RestorationRecovery::Subscribing;
                Ok(RestorationProfileAction::Continue)
            }
            RestorationRecovery::NotRequired => Ok(RestorationProfileAction::Continue),
            _ => Ok(RestorationProfileAction::Ignore),
        }
    }

    pub(super) fn restoration_disconnected(&mut self) -> RestorationDisconnectAction {
        if !self.live_dial() {
            return RestorationDisconnectAction::Close;
        }
        match self.restoration {
            RestorationRecovery::Disconnecting => {
                self.restoration = RestorationRecovery::Connecting;
                RestorationDisconnectAction::Reconnect
            }
            RestorationRecovery::Connecting => RestorationDisconnectAction::Ignore,
            _ => RestorationDisconnectAction::Close,
        }
    }

    pub(super) fn restoration_connected(&mut self) -> bool {
        match self.restoration {
            RestorationRecovery::Connecting if self.live_dial() => {
                self.restoration = RestorationRecovery::DiscoveringServices;
                true
            }
            RestorationRecovery::NotRequired | RestorationRecovery::InspectingProfile(_) => true,
            _ => false,
        }
    }

    pub(super) fn restoration_services_discovered(&mut self) -> bool {
        match self.restoration {
            RestorationRecovery::DiscoveringServices if self.live_dial() => {
                self.restoration = RestorationRecovery::DiscoveringCharacteristics;
                true
            }
            RestorationRecovery::NotRequired
            | RestorationRecovery::InspectingProfile(_)
            | RestorationRecovery::Finished => true,
            _ => false,
        }
    }

    pub(super) fn restoration_characteristics_expected(&self) -> bool {
        matches!(
            self.restoration,
            RestorationRecovery::NotRequired | RestorationRecovery::Finished
        ) || (self.live_dial()
            && matches!(
                self.restoration,
                RestorationRecovery::InspectingProfile(_)
                    | RestorationRecovery::DiscoveringCharacteristics
            ))
    }

    pub(super) fn restoration_native_subscribed(&mut self) -> bool {
        match self.restoration {
            RestorationRecovery::Subscribing if self.live_dial() => {
                self.restoration = RestorationRecovery::Finished;
                true
            }
            RestorationRecovery::NotRequired => true,
            _ => false,
        }
    }

    pub(super) fn cancellation_pending(&self) -> bool {
        matches!(self.restoration, RestorationRecovery::Disconnecting)
    }

    fn live_dial(&self) -> bool {
        !self.data_receiver_closed()
            && self
                .completion_tx
                .as_ref()
                .is_some_and(|completion| !completion.is_closed())
    }

    fn discards_old_values(&self) -> bool {
        matches!(
            self.restoration,
            RestorationRecovery::Disconnecting
                | RestorationRecovery::Connecting
                | RestorationRecovery::DiscoveringServices
                | RestorationRecovery::DiscoveringCharacteristics
                | RestorationRecovery::Subscribing
        )
    }

    fn notification_state_expected(&self) -> bool {
        matches!(
            self.restoration,
            RestorationRecovery::NotRequired
                | RestorationRecovery::Subscribing
                | RestorationRecovery::Finished
        )
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
        if !matches!(self.profile, CentralProfile::Native { .. })
            || !self.restoration_native_subscribed()
        {
            return;
        }
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

    pub(super) fn restore_callbacks(&mut self, callbacks: RestoredCallbackBuffer) -> bool {
        if callbacks.failed {
            return false;
        }
        if let RestorationRecovery::InspectingProfile(buffer) = &mut self.restoration {
            *buffer = callbacks;
            return true;
        }
        for control in callbacks.controls {
            if self.control_tx.try_send(control).is_err() {
                return false;
            }
        }
        for data in callbacks.data {
            if self.data_tx.try_send(data).is_err() {
                return false;
            }
        }
        true
    }

    pub(super) fn enqueue_control(
        &mut self,
        control: Control,
    ) -> Result<(), tokio_mpsc::error::TrySendError<Control>> {
        if let RestorationRecovery::InspectingProfile(buffer) = &mut self.restoration {
            return if buffer.buffer_control(control) {
                Ok(())
            } else {
                Err(tokio_mpsc::error::TrySendError::Full(control))
            };
        }
        if self.discards_old_values() {
            return Ok(());
        }
        self.control_tx.try_send(control)
    }

    pub(super) fn enqueue_data(&mut self, data: Box<[u8]>) -> Result<(), GattInboundSendError> {
        if let RestorationRecovery::InspectingProfile(buffer) = &mut self.restoration {
            return if buffer.buffer_data(data) {
                Ok(())
            } else {
                Err(GattInboundSendError::BudgetExceeded)
            };
        }
        if self.discards_old_values() {
            return Ok(());
        }
        self.data_tx.try_send(data)
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

    // All terminal cleanup paths consume this same owner. A reset's cancellation
    // is already in flight; Stop or the original dial timeout must not issue it twice.
    pub(super) fn retire(self) -> bool {
        let cancel = !self.cancellation_pending();
        self.fail();
        cancel
    }

    pub(super) fn reject(mut self, rejection: DialRejection) {
        if let Some(completion_tx) = self.completion_tx.take() {
            let _ = completion_tx.send(DialCompletion::Rejected(rejection));
        }
    }

    pub(super) fn data_receiver_closed(&self) -> bool {
        // The control receiver is handshake-only and closes when a link settles. The data
        // receiver is retained by the attached member for the full lifetime of this role.
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

pub(super) fn closed_central_session_ids(
    sessions: &HashMap<CoreBluetoothPeerId, CentralPeerSession>,
) -> Vec<CoreBluetoothPeerId> {
    sessions
        .iter()
        .filter_map(|(peer_id, session)| session.data_receiver_closed().then_some(*peer_id))
        .collect()
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
    manager_signals: ManagerSignalSender,
    sighting_wake: tokio_mpsc::Sender<()>,
    manager: RefCell<Option<SendCentralManager>>,
    radio_enabled: Arc<AtomicBool>,
    registry: RefCell<CentralPeerRegistry<SendPeripheral>>,
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
            *self.ivars().manager.borrow_mut() = Some(SendCentralManager(central.retain()));
            // SAFETY: CoreBluetooth supplied this live manager to its delegate on the configured
            // serial dispatch queue.
            if unsafe { central.state() } == CBManagerState::PoweredOn {
                self.ivars().manager_signals.central_powered();
            }
        }

        #[unsafe(method(centralManager:willRestoreState:))]
        fn will_restore_state(
            &self,
            central: &CBCentralManager,
            dict: &NSDictionary<NSString, AnyObject>,
        ) {
            *self.ivars().manager.borrow_mut() = Some(SendCentralManager(central.retain()));
            self.reap_closed_sessions(central);
            // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
            let key: &NSString = unsafe { CBCentralManagerRestoredStatePeripheralsKey };
            let Some(restored) = dict.objectForKey(key) else {
                return;
            };
            // SAFETY: CoreBluetooth documents this restoration dictionary value as an NSArray of
            // CBPeripheral objects; `restored` retains the array for the duration of the borrow.
            let peripherals: &NSArray<CBPeripheral> =
                unsafe { &*(Retained::as_ptr(&restored) as *const NSArray<CBPeripheral>) };
            if !self.ivars().radio_enabled.load(Ordering::Acquire) {
                for peripheral in peripherals.iter() {
                    let peer_id = core_bluetooth_peer_id(&peripheral);
                    // Remove any earlier exact owner before cancellation so the queued shutdown
                    // drain cannot repeat this cancellation.
                    self.ivars().registry.borrow_mut().remove_peer(peer_id);
                    // SAFETY: CoreBluetooth supplied the restored peripheral and manager together
                    // on their serial queue. A logically disabled backend adopts no restored link.
                    unsafe {
                        peripheral.setDelegate(None);
                        central.cancelPeripheralConnection(&peripheral);
                    }
                }
                return;
            }
            for peripheral in peripherals.iter() {
                let peer_id = core_bluetooth_peer_id(&peripheral);
                let address = peer_id.address();
                let admission = self
                    .ivars()
                    .registry
                    .borrow_mut()
                    .admit_restored(peer_id, SendPeripheral(peripheral.retain()));
                match admission {
                    RestoredAdmission::Admitted | RestoredAdmission::Updated => {}
                    RestoredAdmission::AlreadyOwned => {
                        crate::diagnostic_log::debug!(
                            "bluetooth: ignoring duplicate restoration for already-owned peripheral {:02x?}",
                            address.octets()
                        );
                        continue;
                    }
                    RestoredAdmission::Full => {
                        crate::diagnostic_log::warn!(
                            "bluetooth: rejecting restored peripheral {:02x?} — restored-peer capacity is full",
                            address.octets()
                        );
                        // SAFETY: CoreBluetooth supplied both live objects on the manager's serial
                        // queue; the registry's mutable borrow ended before this framework call.
                        unsafe { central.cancelPeripheralConnection(&peripheral) };
                        continue;
                    }
                }
                crate::diagnostic_log::debug!(
                    "bluetooth: restored peripheral {:02x?} from a background relaunch — re-adopting",
                    address.octets()
                );
                // CoreBluetooth may deliver the value notification that caused this relaunch as
                // soon as the restoration delegate method returns. Install the peripheral
                // delegate now and buffer those values until the Host admits this central-role
                // session. Discovery, subscriptions, and identity reads are re-driven afterward.
                // SAFETY: CoreBluetooth supplied this retained peripheral on the manager's serial
                // queue, and `self` remains retained as the manager delegate.
                unsafe {
                    peripheral.setDelegate(Some(ProtocolObject::from_ref(self)));
                }
            }
            // Wake the async backend without waiting for queue capacity. The capacity-one signal
            // is only advisory; restored ownership remains in the queue-confined registry until
            // the backend offers and claims it.
            let _ = self.ivars().sighting_wake.try_send(());
        }

        #[unsafe(method(centralManager:didDiscoverPeripheral:advertisementData:RSSI:))]
        fn did_discover(
            &self,
            central: &CBCentralManager,
            peripheral: &CBPeripheral,
            advertisement_data: &NSDictionary<NSString, AnyObject>,
            rssi: &NSNumber,
        ) {
            if !self.ivars().radio_enabled.load(Ordering::Acquire) {
                return;
            }
            self.ivars().scan_activity.store(true, Ordering::Relaxed);
            let peer_id = core_bluetooth_peer_id(peripheral);
            let now = Instant::now();
            self.reap_closed_sessions_at(central, now);
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
            let observed = self.ivars().registry.borrow_mut().observe(
                peer_id,
                SendPeripheral(peripheral.retain()),
                rssi,
            );
            if !observed {
                return;
            }
            let _ = self.ivars().sighting_wake.try_send(());
        }

        #[unsafe(method(centralManager:didConnectPeripheral:))]
        fn did_connect(&self, central: &CBCentralManager, peripheral: &CBPeripheral) {
            if !self.ivars().radio_enabled.load(Ordering::Acquire) {
                // A connect completion can race the queued logical shutdown. Remove the exact
                // registry owner before cancelling so the later drain cannot cancel it twice; if
                // shutdown already drained it, this intentionally does nothing.
                self.cancel_registered_peer(central, core_bluetooth_peer_id(peripheral));
                return;
            }
            self.reap_closed_sessions(central);
            let peer_id = core_bluetooth_peer_id(peripheral);
            let discover = self
                .ivars()
                .sessions
                .borrow_mut()
                .get_mut(&peer_id)
                .is_some_and(CentralPeerSession::restoration_connected);
            if !discover {
                return;
            }
            crate::diagnostic_log::debug!(
                "bluetooth: dial connected over LE, discovering Prns service"
            );
            discover_prns_services(peripheral);
        }

        #[unsafe(method(centralManager:didFailToConnectPeripheral:error:))]
        fn did_fail_to_connect(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            crate::diagnostic_log::warn!("bluetooth: dial connect FAILED: {error:?}");
            self.disconnect_peer(core_bluetooth_peer_id(peripheral));
        }

        #[unsafe(method(centralManager:didDisconnectPeripheral:error:))]
        fn did_disconnect(
            &self,
            central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            self.ivars()
                .discovery_guard
                .borrow_mut()
                .clear_stale_cancellation(peer_id);
            crate::diagnostic_log::warn!(
                "bluetooth: central-role peripheral disconnected: {error:?}"
            );
            let action = if self.ivars().radio_enabled.load(Ordering::Acquire) {
                self.ivars().sessions.borrow_mut().get_mut(&peer_id).map_or(
                    RestorationDisconnectAction::Close,
                    CentralPeerSession::restoration_disconnected,
                )
            } else {
                RestorationDisconnectAction::Close
            };
            match action {
                RestorationDisconnectAction::Reconnect => {
                    // Keep the exact registry owner and original dial completion. This
                    // is one continuation, not another sighting, retry timer, or dial.
                    // CoreBluetooth only guarantees a local disconnect; another app
                    // can still retain GAP. The ordinary handshake remains bounded.
                    crate::diagnostic_log::debug!(
                        "bluetooth: reconnecting restored native peripheral after local disconnect"
                    );
                    // SAFETY: CoreBluetooth supplied this retained peer and manager
                    // on their serial queue; the live pending session still owns it.
                    unsafe { central.connectPeripheral_options(peripheral, None) };
                    return;
                }
                RestorationDisconnectAction::Ignore => return,
                RestorationDisconnectAction::Close => {}
            }
            self.disconnect_peer(peer_id);
        }
    }

    unsafe impl CBPeripheralDelegate for CentralDelegate {
        #[unsafe(method(peripheral:didDiscoverServices:))]
        fn did_discover_services(&self, peripheral: &CBPeripheral, error: Option<&NSError>) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            let expected = self
                .ivars()
                .sessions
                .borrow_mut()
                .get_mut(&peer_id)
                .is_some_and(CentralPeerSession::restoration_services_discovered);
            if !expected {
                return;
            }
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
            let expected = self
                .ivars()
                .sessions
                .borrow()
                .get(&peer_id)
                .is_some_and(CentralPeerSession::restoration_characteristics_expected);
            if !expected {
                return;
            }
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
                if !self.select_restoration_profile(peripheral, peer_id, PeerProtocol::Native) {
                    return;
                }
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
            if !self.select_restoration_profile(peripheral, peer_id, PeerProtocol::Columba) {
                return;
            }
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
            let expected = self
                .ivars()
                .sessions
                .borrow()
                .get(&peer_id)
                .is_some_and(CentralPeerSession::notification_state_expected);
            if !expected {
                return;
            }
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
            if self
                .ivars()
                .sessions
                .borrow()
                .get(&peer_id)
                .is_some_and(CentralPeerSession::discards_old_values)
            {
                return;
            }
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
                let data = value.to_vec().into_boxed_slice();
                let restored = self
                    .ivars()
                    .registry
                    .borrow_mut()
                    .buffer_restored_data(peer_id, data);
                let data = match restored {
                    RestoredBufferResult::Buffered | RestoredBufferResult::AlreadyFailed => {
                        return;
                    }
                    RestoredBufferResult::Overflowed => {
                        crate::diagnostic_log::warn!(
                            "bluetooth: restored GATT notification buffer exceeded for {:02x?}",
                            peer_id.address().octets()
                        );
                        return;
                    }
                    RestoredBufferResult::NotRestored(data) => data,
                };
                let enqueue_error = self
                    .ivars()
                    .sessions
                    .borrow_mut()
                    .get_mut(&peer_id)
                    .and_then(|session| session.enqueue_data(data).err());
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
            let restored = self
                .ivars()
                .registry
                .borrow_mut()
                .buffer_restored_control(peer_id, control);
            let control = match restored {
                RestoredBufferResult::Buffered | RestoredBufferResult::AlreadyFailed => return,
                RestoredBufferResult::Overflowed => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: restored control buffer exceeded for {:02x?}",
                        peer_id.address().octets()
                    );
                    return;
                }
                RestoredBufferResult::NotRestored(control) => control,
            };
            let enqueue_error = self
                .ivars()
                .sessions
                .borrow_mut()
                .get_mut(&peer_id)
                .and_then(|session| session.enqueue_control(control).err());
            if let Some(error) = enqueue_error {
                crate::diagnostic_log::warn!(
                    "bluetooth: control inbox failed for {:02x?}: {error:?}",
                    peer_id.address().octets()
                );
                self.fail_peer(peer_id);
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
    fn select_restoration_profile(
        &self,
        peripheral: &CBPeripheral,
        peer_id: CoreBluetoothPeerId,
        protocol: PeerProtocol,
    ) -> bool {
        let action = self
            .ivars()
            .sessions
            .borrow_mut()
            .get_mut(&peer_id)
            .map(|session| session.restoration_profile(protocol));
        match action {
            Some(Ok(RestorationProfileAction::Continue)) => true,
            Some(Ok(RestorationProfileAction::Disconnect)) => {
                let central = self
                    .ivars()
                    .manager
                    .borrow()
                    .as_ref()
                    .map(|manager| manager.0.clone());
                if let Some(central) = central {
                    crate::diagnostic_log::debug!(
                        "bluetooth: replacing restored native connection before a fresh handshake"
                    );
                    // SAFETY: the original live dial still owns this peripheral.
                    // The manager and peer are used only on their serial queue;
                    // no RefCell borrow remains across the framework call.
                    unsafe { central.cancelPeripheralConnection(peripheral) };
                } else {
                    self.fail_peer(peer_id);
                }
                false
            }
            Some(Err(())) => {
                self.fail_peer(peer_id);
                false
            }
            Some(Ok(RestorationProfileAction::Ignore)) | None => false,
        }
    }

    pub(super) fn new(
        manager_signals: ManagerSignalSender,
        sighting_wake: tokio_mpsc::Sender<()>,
        scan_activity: Arc<AtomicBool>,
        radio_enabled: Arc<AtomicBool>,
        peripheral_capacity: usize,
        restored_capacity: usize,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CentralDelegateIvars {
            manager_signals,
            sighting_wake,
            manager: RefCell::new(None),
            radio_enabled,
            registry: RefCell::new(CentralPeerRegistry::new(
                peripheral_capacity,
                restored_capacity,
            )),
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
        central: &CBCentralManager,
        peer_id: CoreBluetoothPeerId,
        mut session: CentralPeerSession,
    ) -> bool {
        *self.ivars().manager.borrow_mut() = Some(SendCentralManager(central.retain()));
        if !self.ivars().radio_enabled.load(Ordering::Acquire) {
            self.cancel_registered_peer(central, peer_id);
            session.reject(DialRejection::RadioOff);
            return false;
        }
        self.reap_closed_sessions(central);
        if self.ivars().sessions.borrow().contains_key(&peer_id) {
            self.ivars().registry.borrow_mut().mark_active(peer_id);
            session.reject(DialRejection::ExistingCentralSession);
            return false;
        }
        let callbacks = self.ivars().registry.borrow_mut().begin_session(peer_id);
        let Ok(callbacks) = callbacks else {
            self.cancel_registered_peer(central, peer_id);
            session.fail();
            return false;
        };
        if let Some(callbacks) = callbacks {
            if !session.restore_callbacks(callbacks) {
                self.cancel_registered_peer(central, peer_id);
                session.fail();
                return false;
            }
        }
        self.ivars().sessions.borrow_mut().insert(peer_id, session);
        true
    }

    /// Queue-confined restored-event handoff for the async backend.
    pub(super) fn offer_restored(&self) -> Option<CoreBluetoothPeerId> {
        if !self.ivars().radio_enabled.load(Ordering::Acquire) {
            return None;
        }
        self.ivars().registry.borrow_mut().offer_restored()
    }

    /// Queue-confined sighting handoff for the async backend.
    pub(super) fn pop_sighting(&self) -> Option<Sighting> {
        if !self.ivars().radio_enabled.load(Ordering::Acquire) {
            return None;
        }
        let (sighting, more) = self.ivars().registry.borrow_mut().pop_sighting();
        if more {
            let _ = self.ivars().sighting_wake.try_send(());
        }
        sighting
    }

    /// Queue-confined dial lookup. Closed sessions are removed before registry admission so a
    /// dropped link cannot keep either the per-peer entry or aggregate capacity pinned.
    pub(super) fn claim(
        &self,
        central: &CBCentralManager,
        address: BleAddress,
    ) -> CentralDialCandidate<SendPeripheral> {
        if !self.ivars().radio_enabled.load(Ordering::Acquire) {
            return CentralDialCandidate::Missing;
        }
        self.reap_closed_sessions(central);
        self.ivars().registry.borrow_mut().claim(address)
    }

    /// Queue-confined logical radio transition. Disabling fails every async session, clears all
    /// callback/admission state, and cancels each restored, dialing, or active peripheral exactly
    /// once. Published managers stay alive so a later enable does not rebuild CoreBluetooth state.
    pub(super) fn set_radio_enabled(&self, central: &CBCentralManager, enabled: bool) {
        self.ivars().radio_enabled.store(enabled, Ordering::Release);
        if enabled {
            return;
        }

        // SAFETY: the manager and delegate are confined to this serial CoreBluetooth queue.
        unsafe { central.stopScan() };
        self.ivars().scan_activity.store(false, Ordering::Relaxed);

        let sessions = core::mem::take(&mut *self.ivars().sessions.borrow_mut());
        let already_cancelling: Vec<_> = sessions
            .into_iter()
            .filter_map(|(peer_id, session)| (!session.retire()).then_some(peer_id))
            .collect();
        let owned = self.ivars().registry.borrow_mut().drain_owned();
        *self.ivars().discovery_guard.borrow_mut() = DiscoveryGuard::default();
        for peripheral in owned {
            // SAFETY: the retained peripheral, manager, and delegate all belong to this serial
            // queue. Detaching first prevents late characteristic callbacks from reviving work.
            unsafe {
                peripheral.0.setDelegate(None);
                if !already_cancelling.contains(&core_bluetooth_peer_id(&peripheral.0)) {
                    central.cancelPeripheralConnection(&peripheral.0);
                }
            }
        }
    }

    pub(super) fn discard_peer(&self, peer_id: CoreBluetoothPeerId) {
        self.ivars().registry.borrow_mut().remove_peer(peer_id);
    }

    fn cancel_registered_peer(&self, central: &CBCentralManager, peer_id: CoreBluetoothPeerId) {
        let peripheral = self.ivars().registry.borrow_mut().remove_peer(peer_id);
        let Some(peripheral) = peripheral else {
            return;
        };
        if self
            .ivars()
            .sessions
            .borrow()
            .get(&peer_id)
            .is_some_and(CentralPeerSession::cancellation_pending)
        {
            return;
        }
        // SAFETY: the manager, retained peripheral, and this delegate are confined to the same
        // serial CoreBluetooth queue.
        unsafe { central.cancelPeripheralConnection(&peripheral.0) };
    }

    fn finish_peer(&self, peer_id: CoreBluetoothPeerId, cancel_connection: bool) {
        let session = self.ivars().sessions.borrow_mut().remove(&peer_id);
        let peripheral = self.ivars().registry.borrow_mut().remove_peer(peer_id);
        let needs_cancel = session.is_none_or(CentralPeerSession::retire);
        if cancel_connection && needs_cancel {
            let central = self
                .ivars()
                .manager
                .borrow()
                .as_ref()
                .map(|manager| manager.0.clone());
            if let (Some(central), Some(peripheral)) = (central, peripheral) {
                // SAFETY: every caller and both retained framework objects are confined to the
                // central manager's serial dispatch queue.
                unsafe { central.cancelPeripheralConnection(&peripheral.0) };
            }
        }
    }

    fn disconnect_peer(&self, peer_id: CoreBluetoothPeerId) {
        self.finish_peer(peer_id, false);
    }

    pub(super) fn reap_closed_sessions(&self, central: &CBCentralManager) {
        self.reap_closed_sessions_at(central, Instant::now());
    }

    fn reap_closed_sessions_at(&self, central: &CBCentralManager, now: Instant) {
        let closed = closed_central_session_ids(&self.ivars().sessions.borrow());
        for peer_id in closed {
            let needs_cancel = self
                .ivars()
                .sessions
                .borrow_mut()
                .remove(&peer_id)
                .is_none_or(CentralPeerSession::retire);
            let peripheral = self.ivars().registry.borrow_mut().remove_peer(peer_id);
            let Some(peripheral) = peripheral else {
                continue;
            };
            self.ivars()
                .discovery_guard
                .borrow_mut()
                .record_stale_cancellation(peer_id, now);
            crate::diagnostic_log::debug!(
                "bluetooth: reaping closed central session for {:02x?}",
                peer_id.address().octets()
            );
            if needs_cancel {
                // SAFETY: this method and all callers are confined to the manager's serial dispatch
                // queue; both retained framework objects remain live through the cancellation call.
                unsafe { central.cancelPeripheralConnection(&peripheral.0) };
            }
        }
    }

    pub(super) fn submit_write(
        &self,
        peripheral: &CBPeripheral,
        peer_id: CoreBluetoothPeerId,
        request: GattWriteRequest,
    ) {
        if !self.ivars().radio_enabled.load(Ordering::Acquire) {
            request.complete(Err(MacosBleError::Closed));
            return;
        }
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

    pub(super) fn fail_peer(&self, peer_id: CoreBluetoothPeerId) {
        self.finish_peer(peer_id, true);
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
