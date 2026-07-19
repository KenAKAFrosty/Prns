use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use prns_core::interfaces::bluetooth_auto::core::{
    encode_stream_frame, fragments_of, BleAddress, Control, Dialect, Fragment, L2capPlan,
    Reassembler, StreamDeframer, BLE_HW_MTU, CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN,
    STREAM_FRAME_PREFIX_LEN,
};
use prns_core::interfaces::bluetooth_auto::seam::{BleLink, BleSink, BleSource};

use super::bridge::{LinkSignal, WorkSignal};
use super::AndroidBleError;

const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;
const GATT_REASSEMBLY_CAP: usize = 600;
const GATT_FRAGMENT_PAYLOAD: usize = 180;

pub struct AndroidBleLink {
    pub(super) conn_id: u32,
    pub(super) address: BleAddress,
    pub(super) dialect: Dialect,
    pub(super) control_in: UnboundedReceiver<Vec<u8>>,
    pub(super) l2cap_in: Option<UnboundedReceiver<Vec<u8>>>,
    pub(super) data_in: Option<UnboundedReceiver<Vec<u8>>>,
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

    fn dialect(&self) -> Dialect {
        self.dialect
    }

    fn address(&self) -> BleAddress {
        self.address
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), AndroidBleError> {
        if self.dialect == Dialect::Columba {
            if let Control::Hello { identity, .. } = msg {
                if let Ok(mut out) = self.data_out.lock() {
                    out.push_back(identity.as_bytes().to_vec());
                }
                self.work.wake();
            }
            return Ok(());
        }
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg
            .encode(&mut buf)
            .ok_or(AndroidBleError::ControlTooLarge)?;
        if let Ok(mut out) = self.control_out.lock() {
            out.extend(buf[..len].iter().copied());
        }
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
        if self.dialect == Dialect::Columba {
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
        let (merged_tx, merged_rx) = unbounded_channel::<Vec<u8>>();

        if let Some(mut data_in) = self.data_in {
            let frames = merged_tx.clone();
            tokio::spawn(async move {
                let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
                while let Some(message) = data_in.recv().await {
                    if let Some(fragment) = Fragment::decode(&message) {
                        if let Some(frame) = reassembler.absorb(&fragment) {
                            if frames.send(frame.to_vec()).is_err() {
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
                        if frames.send(frame[..len].to_vec()).is_err() {
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
    inbound: UnboundedReceiver<Vec<u8>>,
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
            if let Ok(mut out) = self.l2cap_out.lock() {
                out.extend(framed[..len].iter().copied());
            }
        } else {
            let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
            for fragment in fragments_of(frame, GATT_FRAGMENT_PAYLOAD) {
                let len = fragment
                    .encode(&mut buf)
                    .ok_or(AndroidBleError::FrameTooLarge)?;
                if let Ok(mut out) = self.gatt_out.lock() {
                    out.push_back(buf[..len].to_vec());
                }
            }
        }
        self.work.wake();
        Ok(())
    }
}
