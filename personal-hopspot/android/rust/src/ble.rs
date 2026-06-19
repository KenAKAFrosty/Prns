use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;

use personal_rns::interfaces::bluetooth_auto::core::{
    encode_stream_frame, BleAddress, Control, Dialect, StreamDeframer, Transport, BLE_HW_MTU,
    CONTROL_MAX_LEN, STREAM_FRAME_PREFIX_LEN,
};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource,
};

const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;

#[derive(Debug)]
pub enum AndroidBleError {
    Closed,
    ControlTooLarge,
    FrameTooLarge,
}

struct LinkSlot {
    address: BleAddress,
    control_in: UnboundedReceiver<Vec<u8>>,
    l2cap_in: UnboundedReceiver<Vec<u8>>,
}

struct Shared {
    psm: Mutex<Option<u16>>,
    psm_ready: Notify,
    control_in_tx: Mutex<Option<UnboundedSender<Vec<u8>>>>,
    l2cap_in_tx: Mutex<Option<UnboundedSender<Vec<u8>>>>,
    control_out: Mutex<VecDeque<u8>>,
    l2cap_out: Mutex<VecDeque<u8>>,
    l2cap_is_up: AtomicBool,
    l2cap_up: Notify,
    pending: Mutex<Option<LinkSlot>>,
    inbound_ready: Notify,
}

pub struct AndroidBleBridge {
    shared: Arc<Shared>,
}

impl Clone for AndroidBleBridge {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl AndroidBleBridge {
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                psm: Mutex::new(None),
                psm_ready: Notify::new(),
                control_in_tx: Mutex::new(None),
                l2cap_in_tx: Mutex::new(None),
                control_out: Mutex::new(VecDeque::new()),
                l2cap_out: Mutex::new(VecDeque::new()),
                l2cap_is_up: AtomicBool::new(false),
                l2cap_up: Notify::new(),
                pending: Mutex::new(None),
                inbound_ready: Notify::new(),
            }),
        }
    }

    pub fn set_psm(&self, psm: u16) {
        if let Ok(mut slot) = self.shared.psm.lock() {
            *slot = Some(psm);
        }
        self.shared.psm_ready.notify_one();
    }

    pub async fn await_psm(&self) -> u16 {
        loop {
            if let Ok(slot) = self.shared.psm.lock() {
                if let Some(psm) = *slot {
                    return psm;
                }
            }
            self.shared.psm_ready.notified().await;
        }
    }

    pub fn central_ready(&self, address: [u8; 6]) {
        let (control_tx, control_rx) = unbounded_channel::<Vec<u8>>();
        let (l2cap_tx, l2cap_rx) = unbounded_channel::<Vec<u8>>();
        if let Ok(mut tx) = self.shared.control_in_tx.lock() {
            *tx = Some(control_tx);
        }
        if let Ok(mut tx) = self.shared.l2cap_in_tx.lock() {
            *tx = Some(l2cap_tx);
        }
        if let Ok(mut out) = self.shared.control_out.lock() {
            out.clear();
        }
        if let Ok(mut out) = self.shared.l2cap_out.lock() {
            out.clear();
        }
        self.shared.l2cap_is_up.store(false, Ordering::Release);
        if let Ok(mut pending) = self.shared.pending.lock() {
            *pending = Some(LinkSlot {
                address: BleAddress::new(address),
                control_in: control_rx,
                l2cap_in: l2cap_rx,
            });
        }
        self.shared.inbound_ready.notify_one();
    }

    pub fn control_in(&self, bytes: &[u8]) {
        if let Ok(tx) = self.shared.control_in_tx.lock() {
            if let Some(tx) = tx.as_ref() {
                let _ = tx.send(bytes.to_vec());
            }
        }
    }

    pub fn control_out(&self, out: &mut [u8]) -> usize {
        drain(&self.shared.control_out, out)
    }

    pub fn l2cap_in(&self, bytes: &[u8]) {
        if let Ok(tx) = self.shared.l2cap_in_tx.lock() {
            if let Some(tx) = tx.as_ref() {
                let _ = tx.send(bytes.to_vec());
            }
        }
    }

    pub fn l2cap_out(&self, out: &mut [u8]) -> usize {
        drain(&self.shared.l2cap_out, out)
    }

    pub fn l2cap_up(&self) {
        self.shared.l2cap_is_up.store(true, Ordering::Release);
        self.shared.l2cap_up.notify_one();
    }

    pub fn disconnected(&self) {
        if let Ok(mut tx) = self.shared.control_in_tx.lock() {
            *tx = None;
        }
        if let Ok(mut tx) = self.shared.l2cap_in_tx.lock() {
            *tx = None;
        }
    }
}

impl Default for AndroidBleBridge {
    fn default() -> Self {
        Self::new()
    }
}

fn drain(queue: &Mutex<VecDeque<u8>>, out: &mut [u8]) -> usize {
    let Ok(mut queue) = queue.lock() else {
        return 0;
    };
    let mut written = 0;
    for slot in out.iter_mut() {
        let Some(byte) = queue.pop_front() else {
            break;
        };
        *slot = byte;
        written += 1;
    }
    written
}

pub struct AndroidBleBackend {
    bridge: AndroidBleBridge,
}

impl AndroidBleBackend {
    #[must_use]
    pub fn new(bridge: AndroidBleBridge) -> Self {
        Self { bridge }
    }
}

impl BleBackend for AndroidBleBackend {
    const MAX_PEERS: usize = 4;
    type Error = AndroidBleError;
    type Link = AndroidBleLink;

    async fn advertise(&mut self) -> Result<(), AndroidBleError> {
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<AndroidBleLink> {
        loop {
            let slot = self
                .bridge
                .shared
                .pending
                .lock()
                .ok()
                .and_then(|mut pending| pending.take());
            if let Some(slot) = slot {
                return BleEvent::Inbound(AndroidBleLink {
                    address: slot.address,
                    control_in: slot.control_in,
                    l2cap_in: Some(slot.l2cap_in),
                    shared: Arc::clone(&self.bridge.shared),
                });
            }
            self.bridge.shared.inbound_ready.notified().await;
        }
    }

    async fn dial(&mut self, _address: BleAddress) {}
}

pub struct AndroidBleLink {
    address: BleAddress,
    control_in: UnboundedReceiver<Vec<u8>>,
    l2cap_in: Option<UnboundedReceiver<Vec<u8>>>,
    shared: Arc<Shared>,
}

impl BleLink for AndroidBleLink {
    type Error = AndroidBleError;
    type Source = AndroidBleSource;
    type Sink = AndroidBleSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        self.address
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), AndroidBleError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg.encode(&mut buf).ok_or(AndroidBleError::ControlTooLarge)?;
        if let Ok(mut out) = self.shared.control_out.lock() {
            out.extend(buf[..len].iter().copied());
        }
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

    async fn upgrade(&mut self, transport: &Transport) -> Result<(), AndroidBleError> {
        match transport {
            Transport::L2cap { .. } => {
                while !self.shared.l2cap_is_up.load(Ordering::Acquire) {
                    self.shared.l2cap_up.notified().await;
                }
                Ok(())
            }
            Transport::Gatt => Ok(()),
        }
    }

    fn into_data(self) -> (AndroidBleSource, AndroidBleSink) {
        (
            AndroidBleSource {
                l2cap_in: self.l2cap_in,
                deframer: StreamDeframer::new(),
            },
            AndroidBleSink {
                shared: self.shared,
            },
        )
    }
}

pub struct AndroidBleSource {
    l2cap_in: Option<UnboundedReceiver<Vec<u8>>>,
    deframer: StreamDeframer<{ 2 * L2CAP_SDU_LEN }>,
}

impl BleSource for AndroidBleSource {
    type Error = AndroidBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, AndroidBleError> {
        let Some(rx) = self.l2cap_in.as_mut() else {
            return core::future::pending().await;
        };
        loop {
            if let Some(len) = self.deframer.next_frame(out) {
                return Ok(len);
            }
            let chunk = rx.recv().await.ok_or(AndroidBleError::Closed)?;
            if !self.deframer.absorb(&chunk) {
                return Err(AndroidBleError::FrameTooLarge);
            }
        }
    }
}

pub struct AndroidBleSink {
    shared: Arc<Shared>,
}

impl BleSink for AndroidBleSink {
    type Error = AndroidBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), AndroidBleError> {
        let mut framed = [0u8; L2CAP_SDU_LEN];
        let len = encode_stream_frame(frame, &mut framed).ok_or(AndroidBleError::FrameTooLarge)?;
        if let Ok(mut out) = self.shared.l2cap_out.lock() {
            out.extend(framed[..len].iter().copied());
        }
        Ok(())
    }
}
