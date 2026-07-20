use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{channel, Receiver};

use prns_core::interfaces::bluetooth_auto::core::{
    encode_stream_frame, fragments_of, BleAddress, BleIdentity, Control, Fragment, L2capPlan,
    PeerProtocol, Reassembler, StreamDeframer, BLE_HW_MTU, CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN,
    STREAM_FRAME_PREFIX_LEN,
};
use prns_core::interfaces::bluetooth_auto::seam::{BleLink, BleSink, BleSource};

use super::bridge::{LinkSignal, WorkSignal};
use super::AndroidBleError;

const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;
const GATT_REASSEMBLY_CAP: usize = 600;
const GATT_FRAGMENT_PAYLOAD: usize = 180;
const MERGED_IN_DEPTH: usize = 16;
const OUTBOUND_BYTE_CAP: usize = 8 * BLE_HW_MTU;
const OUTBOUND_FRAME_DEPTH: usize = 16;

pub struct AndroidBleLink {
    pub(super) conn_id: u32,
    pub(super) address: BleAddress,
    pub(super) peer_protocol: PeerProtocol,
    pub(super) peer_identity: Option<BleIdentity>,
    pub(super) control_in: Receiver<Vec<u8>>,
    pub(super) l2cap_in: Option<Receiver<Vec<u8>>>,
    pub(super) data_in: Option<Receiver<Vec<u8>>>,
    pub(super) control_out: Arc<Mutex<VecDeque<u8>>>,
    pub(super) l2cap_out: Arc<Mutex<VecDeque<u8>>>,
    pub(super) data_out: Arc<Mutex<VecDeque<Vec<u8>>>>,
    pub(super) l2cap_up: Arc<LinkSignal>,
    pub(super) l2cap_opens: Arc<Mutex<VecDeque<(u32, u16)>>>,
    pub(super) work: Arc<WorkSignal>,
}

impl BleLink for AndroidBleLink {
    type Error = AndroidBleError;
    type Source = AndroidBleSource;
    type Sink = AndroidBleSink;

    fn peer_protocol(&self) -> PeerProtocol {
        self.peer_protocol
    }

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, AndroidBleError> {
        self.peer_identity.ok_or(AndroidBleError::Closed)
    }

    async fn send_columba_identity(
        &mut self,
        identity: BleIdentity,
    ) -> Result<(), AndroidBleError> {
        let mut out = self.data_out.lock().map_err(|_| AndroidBleError::Closed)?;
        if out.len() >= OUTBOUND_FRAME_DEPTH {
            return Err(AndroidBleError::QueueFull);
        }
        out.push_back(identity.as_bytes().to_vec());
        drop(out);
        self.work.wake();
        Ok(())
    }

    fn address(&self) -> BleAddress {
        self.address
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), AndroidBleError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg
            .encode(&mut buf)
            .ok_or(AndroidBleError::ControlTooLarge)?;
        let mut out = self
            .control_out
            .lock()
            .map_err(|_| AndroidBleError::Closed)?;
        if out.len().saturating_add(len) > OUTBOUND_BYTE_CAP {
            return Err(AndroidBleError::QueueFull);
        }
        out.extend(buf[..len].iter().copied());
        drop(out);
        self.work.wake();
        Ok(())
    }

    async fn control_recv(&mut self) -> Result<Control, AndroidBleError> {
        loop {
            let bytes = self
                .control_in
                .recv()
                .await
                .ok_or(AndroidBleError::Closed)?;
            if let Some(control) = Control::decode(&bytes) {
                return Ok(control);
            }
        }
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), AndroidBleError> {
        if self.peer_protocol == PeerProtocol::Columba {
            return Ok(());
        }
        if let L2capPlan::Open { psm } = plan {
            if let Ok(mut opens) = self.l2cap_opens.lock() {
                opens.push_back((self.conn_id, psm.get()));
            }
            self.work.wake();
        }
        Ok(())
    }

    fn into_data(self) -> (AndroidBleSource, AndroidBleSink) {
        let (merged_tx, merged_rx) = channel::<Vec<u8>>(MERGED_IN_DEPTH);

        if let Some(mut data_in) = self.data_in {
            let frames = merged_tx.clone();
            tokio::spawn(async move {
                let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
                while let Some(message) = data_in.recv().await {
                    if let Some(fragment) = Fragment::decode(&message) {
                        if let Some(frame) = reassembler.absorb(&fragment) {
                            if frames.send(frame.to_vec()).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }

        if let Some(mut l2cap_in) = self.l2cap_in {
            let frames = merged_tx.clone();
            tokio::spawn(async move {
                let mut deframer = StreamDeframer::<{ 2 * L2CAP_SDU_LEN }>::new();
                let mut frame = std::vec![0u8; 2 * L2CAP_SDU_LEN];
                while let Some(chunk) = l2cap_in.recv().await {
                    if !deframer.absorb(&chunk) {
                        break;
                    }
                    while let Some(len) = deframer.next_frame(&mut frame) {
                        if frames.send(frame[..len].to_vec()).await.is_err() {
                            return;
                        }
                    }
                }
            });
        }

        drop(merged_tx);
        (
            AndroidBleSource { inbound: merged_rx },
            AndroidBleSink {
                l2cap_out: self.l2cap_out,
                gatt_out: self.data_out,
                l2cap_up: self.l2cap_up,
                work: self.work,
            },
        )
    }
}

pub struct AndroidBleSource {
    inbound: Receiver<Vec<u8>>,
}

impl BleSource for AndroidBleSource {
    type Error = AndroidBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, AndroidBleError> {
        let frame = self.inbound.recv().await.ok_or(AndroidBleError::Closed)?;
        let n = frame.len().min(out.len());
        out[..n].copy_from_slice(&frame[..n]);
        Ok(n)
    }
}

pub struct AndroidBleSink {
    l2cap_out: Arc<Mutex<VecDeque<u8>>>,
    gatt_out: Arc<Mutex<VecDeque<Vec<u8>>>>,
    l2cap_up: Arc<LinkSignal>,
    work: Arc<WorkSignal>,
}

impl BleSink for AndroidBleSink {
    type Error = AndroidBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), AndroidBleError> {
        if self.l2cap_up.is_up.load(Ordering::Acquire) {
            let mut framed = [0u8; L2CAP_SDU_LEN];
            let len =
                encode_stream_frame(frame, &mut framed).ok_or(AndroidBleError::FrameTooLarge)?;
            let mut out = self.l2cap_out.lock().map_err(|_| AndroidBleError::Closed)?;
            if out.len().saturating_add(len) > OUTBOUND_BYTE_CAP {
                return Err(AndroidBleError::QueueFull);
            }
            out.extend(framed[..len].iter().copied());
        } else {
            let fragment_count = frame.len().div_ceil(GATT_FRAGMENT_PAYLOAD);
            let mut out = self.gatt_out.lock().map_err(|_| AndroidBleError::Closed)?;
            if out.len().saturating_add(fragment_count) > OUTBOUND_FRAME_DEPTH {
                return Err(AndroidBleError::QueueFull);
            }
            let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
            for fragment in fragments_of(frame, GATT_FRAGMENT_PAYLOAD) {
                let len = fragment
                    .encode(&mut buf)
                    .ok_or(AndroidBleError::FrameTooLarge)?;
                out.push_back(buf[..len].to_vec());
            }
        }
        self.work.wake();
        Ok(())
    }
}
