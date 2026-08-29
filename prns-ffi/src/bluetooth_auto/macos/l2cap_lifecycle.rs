use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQueue, DispatchRetained};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use prns_core::interfaces::bluetooth_auto::{
    encode_stream_frame, Fragment, Reassembler, StreamDeframer, BLE_HW_MTU,
};

use super::data_plane::{flush, DataPlane, Outbound, PumpHandle, PumpPtr, L2CAP_SDU_LEN};
use super::gatt_link::GattInboundReceiver;
use super::MacosBleError;

const GATT_REASSEMBLY_CAP: usize = BLE_HW_MTU;
const L2CAP_OUTBOUND_CAP: usize = 8 * L2CAP_SDU_LEN;

#[derive(Clone, Copy)]
pub(super) enum FailurePolicy {
    /// A central-role GATT connection has an independently observable CoreBluetooth lifecycle, so
    /// a failed fast lane can fall back to that connection's GATT data plane.
    RetainGattFloor,
    /// CoreBluetooth may omit the peripheral-role unsubscribe callback after the peer disappears.
    /// End the owning link so its normal close path can retire the exact stale inbound session.
    EndInboundLink,
}

#[derive(Clone, Copy)]
pub(super) enum DataPlaneEnd {
    Terminated,
}

enum EndAction {
    RetainGattFloor,
    EndLink(oneshot::Sender<DataPlaneEnd>),
}

pub(super) struct PendingLane {
    pub(super) write_ready: oneshot::Receiver<WriteHalf>,
    pub(super) link_end: Option<oneshot::Receiver<DataPlaneEnd>>,
}

pub(super) fn start(
    gatt_inbound: Option<GattInboundReceiver>,
    l2cap_pending: Option<oneshot::Receiver<DataPlane>>,
    frames: tokio_mpsc::Sender<Box<[u8]>>,
    failure_policy: FailurePolicy,
) -> Option<PendingLane> {
    let gatt_merge = gatt_inbound.map(|inbound| spawn_gatt_merge(inbound, frames.clone()));
    match l2cap_pending {
        Some(pending) => Some(spawn_l2cap_lane(
            pending,
            frames,
            gatt_merge,
            failure_policy,
        )),
        None => {
            // Dropping a Tokio JoinHandle detaches the GATT floor task; the task remains owned by
            // the merged source channel until that source closes.
            drop(gatt_merge);
            None
        }
    }
}

fn spawn_gatt_merge(
    mut inbound: GattInboundReceiver,
    frames: tokio_mpsc::Sender<Box<[u8]>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
        loop {
            let message = tokio::select! {
                biased;
                _ = frames.closed() => break,
                message = inbound.recv() => message,
            };
            let Some(message) = message else {
                break;
            };
            let Some(fragment) = Fragment::decode(&message) else {
                continue;
            };
            if let Some(frame) = reassembler.absorb(&fragment) {
                if frames.send(Box::from(frame)).await.is_err() {
                    break;
                }
            }
        }
    })
}

async fn stop_gatt_merge(gatt_merge: Option<tokio::task::JoinHandle<()>>) {
    if let Some(gatt_merge) = gatt_merge {
        gatt_merge.abort();
        let _ = gatt_merge.await;
    }
}

fn spawn_l2cap_lane(
    pending: oneshot::Receiver<DataPlane>,
    frames: tokio_mpsc::Sender<Box<[u8]>>,
    gatt_merge: Option<tokio::task::JoinHandle<()>>,
    failure_policy: FailurePolicy,
) -> PendingLane {
    let (write_tx, write_ready) = oneshot::channel::<WriteHalf>();
    let (end_action, link_end) = match failure_policy {
        FailurePolicy::RetainGattFloor => (EndAction::RetainGattFloor, None),
        FailurePolicy::EndInboundLink => {
            let (end_tx, end_rx) = oneshot::channel();
            (EndAction::EndLink(end_tx), Some(end_rx))
        }
    };
    tokio::spawn(async move {
        let Ok(data) = pending.await else {
            return;
        };
        crate::diagnostic_log::debug!(
            "bluetooth: L2CAP fast lane up — data now rides the channel, GATT stays the floor"
        );
        let DataPlane {
            mut inbound_rx,
            outbound,
            queue,
            pump_ptr,
            pump,
        } = data;
        let _ = write_tx.send(WriteHalf {
            outbound,
            queue,
            pump_ptr,
            _pump: pump.clone(),
        });
        let _read_pump = pump;
        let mut deframer = StreamDeframer::<{ 2 * L2CAP_SDU_LEN }>::new();
        let mut frame = std::vec![0u8; 2 * L2CAP_SDU_LEN];
        'read: while let Some(chunk) = inbound_rx.recv().await {
            if !deframer.absorb(&chunk) {
                break;
            }
            while let Some(len) = deframer.next_frame(&mut frame) {
                if frames.send(Box::from(&frame[..len])).await.is_err() {
                    break 'read;
                }
            }
        }
        match end_action {
            EndAction::RetainGattFloor => {
                // The detached task continues to own the central-role GATT receive floor.
                drop(gatt_merge);
                crate::diagnostic_log::debug!(
                    "bluetooth: L2CAP reader exited — central-role link retains its GATT floor"
                );
            }
            EndAction::EndLink(end_tx) => {
                // Release the authoritative session's GATT receiver before the link owner observes
                // closure. Its existing close path can then distinguish this session from a newer
                // session for the same peer address without a second identity mechanism.
                stop_gatt_merge(gatt_merge).await;
                crate::diagnostic_log::warn!(
                    "bluetooth: L2CAP reader exited — inbound link teardown starting"
                );
                let _ = end_tx.send(DataPlaneEnd::Terminated);
            }
        }
    });
    PendingLane {
        write_ready,
        link_end,
    }
}

pub(super) struct WriteHalf {
    outbound: Arc<Mutex<Outbound>>,
    queue: DispatchRetained<DispatchQueue>,
    pump_ptr: PumpPtr,
    _pump: Arc<PumpHandle>,
}

impl WriteHalf {
    pub(super) fn send(&self, frame: &[u8]) -> Result<(), MacosBleError> {
        let mut framed = [0u8; L2CAP_SDU_LEN];
        let len = encode_stream_frame(frame, &mut framed).ok_or(MacosBleError::FrameTooLarge)?;
        {
            let Ok(mut out) = self.outbound.lock() else {
                return Err(MacosBleError::Closed);
            };
            if out.closed {
                return Err(MacosBleError::Closed);
            }
            if out.pending.len().saturating_add(len) > L2CAP_OUTBOUND_CAP {
                return Err(MacosBleError::QueueFull);
            }
            out.pending.extend(framed[..len].iter().copied());
        }
        let ptr = self.pump_ptr;
        self.queue.exec_async(move || {
            let ptr = ptr;
            // SAFETY: PumpHandle keeps this Box-backed pointer alive and all dereferences plus final
            // destruction are serialized on this same dispatch queue.
            flush(unsafe { &*ptr.0 });
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluetooth_auto::macos::gatt_link::gatt_inbound_channel;

    #[tokio::test]
    async fn inbound_teardown_releases_the_authoritative_gatt_receiver_before_signaling() {
        let (gatt_tx, gatt_rx) = gatt_inbound_channel();
        let (frames_tx, _frames_rx) = tokio_mpsc::channel(1);
        let gatt_merge = spawn_gatt_merge(gatt_rx, frames_tx);

        stop_gatt_merge(Some(gatt_merge)).await;

        assert!(gatt_tx.is_closed());
    }
}
