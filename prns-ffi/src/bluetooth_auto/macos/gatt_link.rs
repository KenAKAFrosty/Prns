use core::time::Duration;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use dispatch2::{DispatchQueue, DispatchRetained};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use prns_core::interfaces::bluetooth_auto::{
    fragments_of, BleAddress, BleIdentity, Control, L2capPlan, PeerProtocol, BLE_HW_MTU,
    CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN,
};
use prns_core::interfaces::bluetooth_auto::{BleLink, BleSink, BleSource};

use super::data_plane::DataPlane;
use super::gatt_write::{GattWriteMode, GattWriteRequest, GattWriteTarget};
use super::l2cap_lifecycle::{self, DataPlaneEnd, FailurePolicy, WriteHalf};
use super::peripheral::ListenerCharacteristic;
use super::{
    CoreBluetoothPeerId, MacosBleError, SendCentralDelegate, SendCharacteristicRef, SendPeripheral,
    SendPeripheralDelegate,
};

const GATT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const GATT_INBOUND_BUDGET_BYTES: usize = 128 * 1024;

#[derive(Clone)]
pub(super) struct GattInboundSender {
    sender: tokio_mpsc::UnboundedSender<Box<[u8]>>,
    queued_bytes: Arc<AtomicUsize>,
    budget_bytes: usize,
}

pub(super) struct GattInboundReceiver {
    receiver: tokio_mpsc::UnboundedReceiver<Box<[u8]>>,
    queued_bytes: Arc<AtomicUsize>,
}

/// Capacity owned by an admitted write but not yet visible to the receiver.
pub(super) struct GattInboundReservation {
    sender: GattInboundSender,
    data: Option<Box<[u8]>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GattInboundSendError {
    Closed,
    BudgetExceeded,
}

pub(super) fn gatt_inbound_channel() -> (GattInboundSender, GattInboundReceiver) {
    gatt_inbound_channel_with_budget(GATT_INBOUND_BUDGET_BYTES)
}

pub(super) fn gatt_inbound_channel_with_budget(
    budget_bytes: usize,
) -> (GattInboundSender, GattInboundReceiver) {
    let (sender, receiver) = tokio_mpsc::unbounded_channel();
    let queued_bytes = Arc::new(AtomicUsize::new(0));
    (
        GattInboundSender {
            sender,
            queued_bytes: queued_bytes.clone(),
            budget_bytes,
        },
        GattInboundReceiver {
            receiver,
            queued_bytes,
        },
    )
}

impl GattInboundSender {
    pub(super) fn try_send(&self, data: Box<[u8]>) -> Result<(), GattInboundSendError> {
        self.try_reserve(data)?.publish()
    }

    pub(super) fn try_reserve(
        &self,
        data: Box<[u8]>,
    ) -> Result<GattInboundReservation, GattInboundSendError> {
        if self.sender.is_closed() {
            return Err(GattInboundSendError::Closed);
        }
        // Charge empty callbacks as one byte so the byte budget also bounds the number of queued
        // allocations. Empty values are not useful GATT fragments, but CoreBluetooth is an
        // external input and must not be able to grow an unbounded queue for free.
        let charge = data.len().max(1);
        let mut queued = self.queued_bytes.load(Ordering::Acquire);
        loop {
            let Some(next) = queued.checked_add(charge) else {
                return Err(GattInboundSendError::BudgetExceeded);
            };
            if next > self.budget_bytes {
                return Err(GattInboundSendError::BudgetExceeded);
            }
            match self.queued_bytes.compare_exchange_weak(
                queued,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => queued = actual,
            }
        }
        Ok(GattInboundReservation {
            sender: self.clone(),
            data: Some(data),
        })
    }

    pub(super) fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

impl GattInboundReservation {
    /// Publish after the whole batch has been admitted. A receiver disappearing
    /// after reservation is delivery loss, not a partial batch-admission failure.
    pub(super) fn commit(self) {
        let _ = self.publish();
    }

    fn publish(mut self) -> Result<(), GattInboundSendError> {
        let data = self.data.take().ok_or(GattInboundSendError::Closed)?;
        match self.sender.sender.send(data) {
            Ok(()) => Ok(()),
            Err(error) => {
                // Restore ownership so Drop refunds exactly this reservation.
                self.data = Some(error.0);
                Err(GattInboundSendError::Closed)
            }
        }
    }
}

impl Drop for GattInboundReservation {
    fn drop(&mut self) {
        if let Some(data) = &self.data {
            self.sender
                .queued_bytes
                .fetch_sub(data.len().max(1), Ordering::AcqRel);
        }
    }
}

impl GattInboundReceiver {
    pub(super) async fn recv(&mut self) -> Option<Box<[u8]>> {
        let data = self.receiver.recv().await?;
        self.queued_bytes
            .fetch_sub(data.len().max(1), Ordering::AcqRel);
        Some(data)
    }
}

pub(super) enum ControlPlane {
    Listener {
        peer_id: CoreBluetoothPeerId,
        delegate: SendPeripheralDelegate,
        gatt_mtu: usize,
    },
    Central {
        peer_id: CoreBluetoothPeerId,
        peripheral: SendPeripheral,
        characteristic: SendCharacteristicRef,
        data_characteristic: Option<GattWriteTarget>,
        central_delegate: SendCentralDelegate,
        queue: DispatchRetained<DispatchQueue>,
        peripheral_manager: SendPeripheralDelegate,
    },
}

impl ControlPlane {
    const fn l2cap_failure_policy(&self) -> FailurePolicy {
        match self {
            Self::Central { .. } => FailurePolicy::RetainGattFloor,
            Self::Listener { .. } => FailurePolicy::EndInboundLink,
        }
    }
}

enum GattWriter {
    Central {
        peer_id: CoreBluetoothPeerId,
        peripheral: SendPeripheral,
        characteristic: SendCharacteristicRef,
        central_delegate: SendCentralDelegate,
        queue: DispatchRetained<DispatchQueue>,
        plan: super::gatt_write::GattWritePlan,
    },
    Listener {
        peer_id: CoreBluetoothPeerId,
        delegate: SendPeripheralDelegate,
        fragment_mtu: usize,
    },
}

impl GattWriter {
    async fn send(&self, frame: &[u8]) -> Result<(), MacosBleError> {
        let fragment_mtu = match self {
            Self::Central { plan, .. } => plan.fragment_mtu(),
            Self::Listener { fragment_mtu, .. } => *fragment_mtu,
        };
        let mut buf = [0u8; FRAGMENT_HEADER_LEN + BLE_HW_MTU];
        for fragment in fragments_of(frame, fragment_mtu) {
            let len = fragment
                .encode(&mut buf)
                .ok_or(MacosBleError::FrameTooLarge)?;
            match self {
                GattWriter::Central {
                    peer_id,
                    peripheral,
                    characteristic,
                    central_delegate,
                    queue,
                    plan,
                    ..
                } => {
                    central_write(
                        *peer_id,
                        peripheral,
                        characteristic,
                        central_delegate,
                        queue,
                        plan.mode(),
                        &buf[..len],
                    )
                    .await?;
                }
                GattWriter::Listener {
                    peer_id, delegate, ..
                } => {
                    let sent =
                        delegate
                            .0
                            .notify(*peer_id, ListenerCharacteristic::Data, &buf[..len]);
                    if !sent {
                        crate::diagnostic_log::warn!(
                            "bluetooth: GATT-data notify queue full — fragment dropped, peer will retransmit"
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

async fn central_write(
    peer_id: CoreBluetoothPeerId,
    peripheral: &SendPeripheral,
    characteristic: &SendCharacteristicRef,
    central_delegate: &SendCentralDelegate,
    queue: &DispatchRetained<DispatchQueue>,
    mode: GattWriteMode,
    bytes: &[u8],
) -> Result<(), MacosBleError> {
    let (completion_tx, completion_rx) = oneshot::channel();
    let request = GattWriteRequest::new(
        SendCharacteristicRef(characteristic.0.clone()),
        Box::from(bytes),
        mode,
        completion_tx,
    );
    let peripheral = SendPeripheral(peripheral.0.clone());
    let delegate = SendCentralDelegate(central_delegate.0.clone());
    queue.exec_async(move || {
        let peripheral = peripheral;
        let delegate = delegate;
        delegate.0.submit_write(&peripheral.0, peer_id, request);
    });
    match tokio::time::timeout(GATT_WRITE_TIMEOUT, completion_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(MacosBleError::Closed),
        Err(_) => {
            crate::diagnostic_log::warn!(
                "bluetooth: {:02x?} {mode:?} GATT write timed out",
                peer_id.address().octets()
            );
            Err(MacosBleError::GattWriteTimeout)
        }
    }
}

pub struct GattLink {
    pub(super) peer_protocol: PeerProtocol,
    pub(super) peer_identity: Option<BleIdentity>,
    pub(super) control: ControlPlane,
    pub(super) control_rx: tokio_mpsc::Receiver<Control>,
    pub(super) address: BleAddress,
    pub(super) data_inbound_rx: Option<GattInboundReceiver>,
    pub(super) l2cap_pending: Option<oneshot::Receiver<DataPlane>>,
}

impl BleLink for GattLink {
    type Error = MacosBleError;
    type Source = GattSource;
    type Sink = GattSink;

    fn peer_protocol(&self) -> PeerProtocol {
        self.peer_protocol
    }

    fn address(&self) -> BleAddress {
        self.address
    }

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, MacosBleError> {
        self.peer_identity
            .ok_or(MacosBleError::MissingColumbaIdentity)
    }

    async fn send_columba_identity(&mut self, identity: BleIdentity) -> Result<(), MacosBleError> {
        let ControlPlane::Central {
            peer_id,
            peripheral,
            characteristic,
            central_delegate,
            queue,
            ..
        } = &self.control
        else {
            return Ok(());
        };
        central_write(
            *peer_id,
            peripheral,
            characteristic,
            central_delegate,
            queue,
            GattWriteMode::WithResponse,
            identity.as_bytes(),
        )
        .await
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), MacosBleError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg.encode(&mut buf).ok_or(MacosBleError::ControlTooLarge)?;
        match &self.control {
            ControlPlane::Listener {
                peer_id, delegate, ..
            } => {
                let sent =
                    delegate
                        .0
                        .notify(*peer_id, ListenerCharacteristic::Control, &buf[..len]);
                if sent {
                    crate::diagnostic_log::debug!(
                        "bluetooth: {:02x?} -> {msg:?}",
                        self.address.octets()
                    );
                    Ok(())
                } else {
                    crate::diagnostic_log::warn!(
                        "bluetooth: {:02x?} notify failed — control PDU did not reach the central, handshake will stall",
                        self.address.octets()
                    );
                    Err(MacosBleError::NotifyFailed)
                }
            }
            ControlPlane::Central {
                peer_id,
                peripheral,
                characteristic,
                central_delegate,
                queue,
                ..
            } => {
                central_write(
                    *peer_id,
                    peripheral,
                    characteristic,
                    central_delegate,
                    queue,
                    GattWriteMode::WithResponse,
                    &buf[..len],
                )
                .await?;
                crate::diagnostic_log::debug!(
                    "bluetooth: {:02x?} -> {msg:?}",
                    self.address.octets()
                );
                Ok(())
            }
        }
    }

    async fn control_recv(&mut self) -> Result<Control, MacosBleError> {
        let control = self.control_rx.recv().await.ok_or(MacosBleError::Closed)?;
        crate::diagnostic_log::debug!("bluetooth: {:02x?} <- {control:?}", self.address.octets());
        Ok(control)
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), MacosBleError> {
        if self.peer_protocol == PeerProtocol::Columba {
            return Ok(());
        }
        match plan {
            L2capPlan::Accept => {
                let (tx, rx) = oneshot::channel::<DataPlane>();
                match &self.control {
                    ControlPlane::Central {
                        peer_id,
                        peripheral_manager,
                        ..
                    } => peripheral_manager.0.arm_pending_channel(*peer_id, tx),
                    ControlPlane::Listener {
                        peer_id, delegate, ..
                    } => delegate.0.arm_pending_channel(*peer_id, tx),
                };
                self.l2cap_pending = Some(rx);
                crate::diagnostic_log::debug!(
                    "bluetooth: {:02x?} armed the L2CAP acceptor — the peer's CoC will upgrade the live GATT-floor link in the background",
                    self.address.octets()
                );
                Ok(())
            }
            L2capPlan::Open { .. } => {
                crate::diagnostic_log::warn!(
                    "bluetooth: {:02x?} asked to open a CoC, but the macOS backend is acceptor-only (a central-side open bonds) — staying on the GATT floor",
                    self.address.octets()
                );
                Ok(())
            }
            L2capPlan::None => Ok(()),
        }
    }

    fn into_data(self) -> (GattSource, GattSink) {
        let (merged_tx, merged_rx) = tokio_mpsc::channel::<Box<[u8]>>(16);
        let l2cap_failure_policy = self.control.l2cap_failure_policy();

        let l2cap_lane = l2cap_lifecycle::start(
            self.data_inbound_rx,
            self.l2cap_pending,
            merged_tx.clone(),
            l2cap_failure_policy,
        );
        let (l2cap_pending, l2cap_end) = match l2cap_lane {
            Some(lane) => (Some(lane.write_ready), lane.link_end),
            None => (None, None),
        };

        drop(merged_tx);
        (
            GattSource {
                inbound: merged_rx,
                l2cap_end,
            },
            GattSink {
                gatt: gatt_writer(&self.control),
                l2cap: None,
                l2cap_pending,
            },
        )
    }
}

fn gatt_writer(control: &ControlPlane) -> Option<GattWriter> {
    match control {
        ControlPlane::Central {
            peer_id,
            peripheral,
            data_characteristic: Some(data_characteristic),
            central_delegate,
            queue,
            ..
        } => {
            crate::diagnostic_log::debug!(
                "bluetooth: {:02x?} GATT writer selected {:?}, fragment MTU {}B",
                peer_id.address().octets(),
                data_characteristic.plan.mode(),
                data_characteristic.plan.fragment_mtu()
            );
            Some(GattWriter::Central {
                peer_id: *peer_id,
                peripheral: SendPeripheral(peripheral.0.clone()),
                characteristic: SendCharacteristicRef(data_characteristic.characteristic.0.clone()),
                central_delegate: SendCentralDelegate(central_delegate.0.clone()),
                queue: queue.clone(),
                plan: data_characteristic.plan,
            })
        }
        ControlPlane::Central {
            data_characteristic: None,
            ..
        } => None,
        ControlPlane::Listener {
            peer_id,
            delegate,
            gatt_mtu,
            ..
        } => Some(GattWriter::Listener {
            peer_id: *peer_id,
            delegate: SendPeripheralDelegate(delegate.0.clone()),
            fragment_mtu: *gatt_mtu,
        }),
    }
}

pub struct GattSource {
    inbound: tokio_mpsc::Receiver<Box<[u8]>>,
    l2cap_end: Option<oneshot::Receiver<DataPlaneEnd>>,
}

impl BleSource for GattSource {
    type Error = MacosBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, MacosBleError> {
        let frame = loop {
            let Some(l2cap_end) = self.l2cap_end.as_mut() else {
                break self.inbound.recv().await.ok_or(MacosBleError::Closed)?;
            };
            tokio::select! {
                biased;
                end = l2cap_end => {
                    self.l2cap_end = None;
                    if matches!(end, Ok(DataPlaneEnd::Terminated)) {
                        return Err(MacosBleError::Closed);
                    }
                }
                frame = self.inbound.recv() => {
                    break frame.ok_or(MacosBleError::Closed)?;
                }
            }
        };
        let len = frame.len().min(out.len());
        out[..len].copy_from_slice(&frame[..len]);
        Ok(len)
    }
}

pub struct GattSink {
    gatt: Option<GattWriter>,
    l2cap: Option<WriteHalf>,
    l2cap_pending: Option<oneshot::Receiver<WriteHalf>>,
}

impl BleSink for GattSink {
    type Error = MacosBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), MacosBleError> {
        if self.l2cap.is_none() {
            if let Some(pending) = self.l2cap_pending.as_mut() {
                match pending.try_recv() {
                    Ok(half) => {
                        self.l2cap = Some(half);
                        self.l2cap_pending = None;
                    }
                    Err(oneshot::error::TryRecvError::Closed) => self.l2cap_pending = None,
                    Err(oneshot::error::TryRecvError::Empty) => {}
                }
            }
        }
        if let Some(l2cap) = &self.l2cap {
            match l2cap.send(frame) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    self.l2cap = None;
                    if self.gatt.is_none() {
                        return Err(err);
                    }
                    crate::diagnostic_log::warn!(
                        "bluetooth: L2CAP send failed — the fast lane is down, frames fall back to the GATT floor"
                    );
                }
            }
        }
        if let Some(gatt) = &self.gatt {
            return gatt.send(frame).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod source_lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn inbound_l2cap_end_closes_source_while_gatt_floor_is_still_open() {
        let (gatt_tx, gatt_rx) = tokio_mpsc::channel(1);
        let (end_tx, end_rx) = oneshot::channel();
        let mut source = GattSource {
            inbound: gatt_rx,
            l2cap_end: Some(end_rx),
        };

        assert!(end_tx.send(DataPlaneEnd::Terminated).is_ok());

        let mut frame = [0; 1];
        assert!(matches!(
            source.recv_frame(&mut frame).await,
            Err(MacosBleError::Closed)
        ));
        assert!(!gatt_tx.is_closed());
    }

    #[tokio::test]
    async fn abandoned_l2cap_upgrade_keeps_the_gatt_floor_live() {
        let (gatt_tx, gatt_rx) = tokio_mpsc::channel(1);
        let (end_tx, end_rx) = oneshot::channel();
        let mut source = GattSource {
            inbound: gatt_rx,
            l2cap_end: Some(end_rx),
        };

        drop(end_tx);
        gatt_tx.send(Box::from(&[7, 8, 9][..])).await.unwrap();

        let mut frame = [0; 3];
        assert_eq!(source.recv_frame(&mut frame).await.unwrap(), 3);
        assert_eq!(frame, [7, 8, 9]);
        assert!(source.l2cap_end.is_none());
    }
}

#[cfg(test)]
mod inbound_reservation_tests {
    use super::*;

    #[tokio::test]
    async fn rollback_releases_unpublished_capacity_shared_by_sender_clones() {
        let (sender, mut receiver) = gatt_inbound_channel_with_budget(5);
        let other = sender.clone();
        sender.try_send(Box::from([1, 2])).unwrap();
        let reserved = other.try_reserve(Box::from([3, 4, 5])).unwrap();
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 5);
        assert!(matches!(
            sender.try_reserve(Box::from([6])),
            Err(GattInboundSendError::BudgetExceeded)
        ));
        assert_eq!(receiver.recv().await.unwrap().as_ref(), &[1, 2]);
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 3);
        assert_eq!(
            receiver.receiver.try_recv(),
            Err(tokio_mpsc::error::TryRecvError::Empty)
        );
        drop(reserved);
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 0);
        assert!(other.try_reserve(Box::from([0; 5])).is_ok());
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn commit_publishes_in_order_without_charging_twice() {
        let (sender, mut receiver) = gatt_inbound_channel_with_budget(3);
        let first = sender.try_reserve(Box::from([1, 2])).unwrap();
        let second = sender.try_reserve(Box::from([3])).unwrap();
        assert_eq!(
            receiver.receiver.try_recv(),
            Err(tokio_mpsc::error::TryRecvError::Empty)
        );
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 3);
        first.commit();
        second.commit();
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 3);
        assert_eq!(receiver.recv().await.unwrap().as_ref(), &[1, 2]);
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 1);
        assert_eq!(receiver.recv().await.unwrap().as_ref(), &[3]);
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn reservation_owns_its_sender_until_commit() {
        let (sender, mut receiver) = gatt_inbound_channel_with_budget(1);
        let reserved = sender.try_reserve(Box::from([7])).unwrap();
        drop(sender);
        assert_eq!(
            receiver.receiver.try_recv(),
            Err(tokio_mpsc::error::TryRecvError::Empty)
        );
        reserved.commit();
        assert_eq!(receiver.recv().await.unwrap().as_ref(), &[7]);
        assert_eq!(receiver.queued_bytes.load(Ordering::Acquire), 0);
        assert!(receiver.recv().await.is_none());

        let (sender, mut receiver) = gatt_inbound_channel_with_budget(1);
        let reserved = sender.try_reserve(Box::from([8])).unwrap();
        drop(sender);
        drop(reserved);
        assert_eq!(receiver.queued_bytes.load(Ordering::Acquire), 0);
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn empty_reservations_charge_one_byte_and_respect_zero_budget() {
        let (zero, _receiver) = gatt_inbound_channel_with_budget(0);
        assert!(matches!(
            zero.try_reserve(Box::from([])),
            Err(GattInboundSendError::BudgetExceeded)
        ));
        let (sender, mut receiver) = gatt_inbound_channel_with_budget(1);
        let reserved = sender.try_reserve(Box::from([])).unwrap();
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 1);
        assert_eq!(
            sender.try_send(Box::from([])),
            Err(GattInboundSendError::BudgetExceeded)
        );
        reserved.commit();
        assert!(receiver.recv().await.unwrap().is_empty());
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 0);
        drop(sender.try_reserve(Box::from([])).unwrap());
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn checked_reservation_charge_never_wraps() {
        let (sender, _receiver) = gatt_inbound_channel_with_budget(usize::MAX);
        // Model already-owned capacity without allocating an impossible payload.
        sender.queued_bytes.store(usize::MAX - 1, Ordering::Release);
        assert!(matches!(
            sender.try_reserve(Box::from([1, 2])),
            Err(GattInboundSendError::BudgetExceeded)
        ));
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), usize::MAX - 1);
        let last = sender.try_reserve(Box::from([])).unwrap();
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), usize::MAX);
        assert!(matches!(
            sender.try_reserve(Box::from([])),
            Err(GattInboundSendError::BudgetExceeded)
        ));
        drop(last);
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), usize::MAX - 1);
        sender.queued_bytes.store(0, Ordering::Release);
    }

    #[test]
    fn closed_receiver_rejects_new_reservations_before_charging() {
        let (sender, receiver) = gatt_inbound_channel_with_budget(0);
        drop(receiver);
        assert!(matches!(
            sender.try_reserve(Box::from([])),
            Err(GattInboundSendError::Closed)
        ));
        assert_eq!(
            sender.try_send(Box::from([1])),
            Err(GattInboundSendError::Closed)
        );
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn receiver_close_after_reservation_refunds_without_rejecting_commit() {
        let (sender, receiver) = gatt_inbound_channel_with_budget(2);
        let committed = sender.try_reserve(Box::from([1])).unwrap();
        let fallible = sender.try_reserve(Box::from([2])).unwrap();
        drop(receiver);
        committed.commit();
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 1);
        // try_send uses this same publication path and retains its Closed result.
        assert_eq!(fallible.publish(), Err(GattInboundSendError::Closed));
        assert_eq!(sender.queued_bytes.load(Ordering::Acquire), 0);
    }
}
