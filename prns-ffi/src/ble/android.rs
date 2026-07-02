use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;

use prns_core::interfaces::bluetooth_auto::core::{
    encode_stream_frame, fragments_of, AndroidHost, BleAddress, BleIdentity, Control, Dialect,
    Endpoint, Fragment, L2capPlan, LinkCapabilities, Psm, Reassembler, StreamDeframer, BLE_HW_MTU,
    CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN, STREAM_FRAME_PREFIX_LEN,
};
use prns_core::interfaces::bluetooth_auto::limits;
use prns_core::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};

const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;
const GATT_REASSEMBLY_CAP: usize = 600;
const GATT_FRAGMENT_PAYLOAD: usize = 180;
pub const RADIO_ENABLED: u32 = 0x01;
pub const RADIO_ADVERTISING: u32 = 0x02;
pub const RADIO_SCANNING: u32 = 0x04;

#[derive(Debug)]
pub enum AndroidBleError {
    Closed,
    ControlTooLarge,
    FrameTooLarge,
}

struct LinkSignal {
    is_up: AtomicBool,
    notify: Notify,
}

struct Endpoints {
    control_in_tx: UnboundedSender<Vec<u8>>,
    l2cap_in_tx: UnboundedSender<Vec<u8>>,
    data_in_tx: UnboundedSender<Vec<u8>>,
    control_out: Arc<Mutex<VecDeque<u8>>>,
    l2cap_out: Arc<Mutex<VecDeque<u8>>>,
    data_out: Arc<Mutex<VecDeque<Vec<u8>>>>,
    l2cap_up: Arc<LinkSignal>,
}

struct PendingLink {
    conn_id: u32,
    address: BleAddress,
    rssi: Option<i8>,
    dialed: bool,
    dialect: Dialect,
    control_in: UnboundedReceiver<Vec<u8>>,
    l2cap_in: UnboundedReceiver<Vec<u8>>,
    data_in: UnboundedReceiver<Vec<u8>>,
    control_out: Arc<Mutex<VecDeque<u8>>>,
    l2cap_out: Arc<Mutex<VecDeque<u8>>>,
    data_out: Arc<Mutex<VecDeque<Vec<u8>>>>,
    l2cap_up: Arc<LinkSignal>,
    l2cap_opens: Arc<Mutex<VecDeque<(u32, u16)>>>,
}

enum Event {
    Sighting {
        address: BleAddress,
        rssi: Option<i8>,
    },
    Link(PendingLink),
    DialFailed {
        address: BleAddress,
    },
}

#[derive(Clone, Copy, Default)]
struct RadioState {
    enabled: bool,
    advertising: bool,
    scanning: bool,
}

impl RadioState {
    fn bits(self) -> u32 {
        if !self.enabled {
            return 0;
        }
        RADIO_ENABLED
            | if self.advertising {
                RADIO_ADVERTISING
            } else {
                0
            }
            | if self.scanning { RADIO_SCANNING } else { 0 }
    }
}

struct Shared {
    radio: Mutex<RadioState>,
    local_identity: Mutex<Option<[u8; 16]>>,
    psm: Mutex<Option<u16>>,
    psm_ready: Notify,
    links: Mutex<HashMap<u32, Endpoints>>,
    events: Mutex<VecDeque<Event>>,
    events_ready: Notify,
    dial_requests: Mutex<VecDeque<[u8; 6]>>,
    l2cap_opens: Arc<Mutex<VecDeque<(u32, u16)>>>,
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
                radio: Mutex::new(RadioState::default()),
                local_identity: Mutex::new(None),
                psm: Mutex::new(None),
                psm_ready: Notify::new(),
                links: Mutex::new(HashMap::new()),
                events: Mutex::new(VecDeque::new()),
                events_ready: Notify::new(),
                dial_requests: Mutex::new(VecDeque::new()),
                l2cap_opens: Arc::new(Mutex::new(VecDeque::new())),
            }),
        }
    }

    pub fn set_local_identity(&self, identity: BleIdentity) {
        if let Ok(mut slot) = self.shared.local_identity.lock() {
            *slot = Some(*identity.as_bytes());
        }
    }

    pub fn local_identity(&self, out: &mut [u8]) -> usize {
        if out.len() < 16 {
            return 0;
        }
        let identity = self
            .shared
            .local_identity
            .lock()
            .ok()
            .and_then(|slot| *slot);
        let Some(identity) = identity else {
            return 0;
        };
        out[..16].copy_from_slice(&identity);
        16
    }

    pub fn set_radio_enabled(&self, enabled: bool) {
        if let Ok(mut radio) = self.shared.radio.lock() {
            radio.enabled = enabled;
            if !enabled {
                radio.advertising = false;
                radio.scanning = false;
            }
        }
        if !enabled {
            self.clear_radio_state();
        }
    }

    pub fn set_advertising(&self, enabled: bool) {
        if let Ok(mut radio) = self.shared.radio.lock() {
            radio.advertising = enabled;
        }
    }

    pub fn set_scanning(&self, enabled: bool) {
        if let Ok(mut radio) = self.shared.radio.lock() {
            radio.scanning = enabled;
        }
    }

    pub fn radio_state(&self) -> u32 {
        self.shared
            .radio
            .lock()
            .map(|state| state.bits())
            .unwrap_or(0)
    }

    fn clear_radio_state(&self) {
        if let Ok(mut slot) = self.shared.psm.lock() {
            *slot = None;
        }
        if let Ok(mut links) = self.shared.links.lock() {
            links.clear();
        }
        if let Ok(mut events) = self.shared.events.lock() {
            events.clear();
        }
        if let Ok(mut requests) = self.shared.dial_requests.lock() {
            requests.clear();
        }
        if let Ok(mut opens) = self.shared.l2cap_opens.lock() {
            opens.clear();
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

    pub fn sighting(&self, address: [u8; 6], rssi: Option<i8>) {
        if let Ok(mut events) = self.shared.events.lock() {
            events.push_back(Event::Sighting {
                address: BleAddress::new(address),
                rssi,
            });
        }
        self.shared.events_ready.notify_one();
    }

    pub fn dial_failed(&self, address: [u8; 6]) {
        if let Ok(mut events) = self.shared.events.lock() {
            events.push_back(Event::DialFailed {
                address: BleAddress::new(address),
            });
        }
        self.shared.events_ready.notify_one();
    }

    pub fn link_up(&self, conn_id: u32, address: [u8; 6], rssi: Option<i8>, dialed: bool) {
        self.link_up_with_dialect(conn_id, address, rssi, dialed, Dialect::Native, None);
    }

    pub fn columba_link_up(
        &self,
        conn_id: u32,
        address: [u8; 6],
        rssi: Option<i8>,
        dialed: bool,
        peer_identity: [u8; 16],
    ) {
        self.link_up_with_dialect(
            conn_id,
            address,
            rssi,
            dialed,
            Dialect::Columba,
            Some(peer_identity),
        );
    }

    fn link_up_with_dialect(
        &self,
        conn_id: u32,
        address: [u8; 6],
        rssi: Option<i8>,
        dialed: bool,
        dialect: Dialect,
        peer_identity: Option<[u8; 16]>,
    ) {
        let (control_tx, control_rx) = unbounded_channel::<Vec<u8>>();
        let (l2cap_tx, l2cap_rx) = unbounded_channel::<Vec<u8>>();
        let (data_tx, data_rx) = unbounded_channel::<Vec<u8>>();
        let control_out = Arc::new(Mutex::new(VecDeque::new()));
        let l2cap_out = Arc::new(Mutex::new(VecDeque::new()));
        let data_out = Arc::new(Mutex::new(VecDeque::new()));
        let l2cap_up = Arc::new(LinkSignal {
            is_up: AtomicBool::new(false),
            notify: Notify::new(),
        });
        if let Some(identity) = peer_identity {
            let _ = control_tx.send(columba_greeting(dialed, identity, rssi));
        }
        if let Ok(mut links) = self.shared.links.lock() {
            links.insert(
                conn_id,
                Endpoints {
                    control_in_tx: control_tx,
                    l2cap_in_tx: l2cap_tx,
                    data_in_tx: data_tx,
                    control_out: Arc::clone(&control_out),
                    l2cap_out: Arc::clone(&l2cap_out),
                    data_out: Arc::clone(&data_out),
                    l2cap_up: Arc::clone(&l2cap_up),
                },
            );
        }
        if let Ok(mut events) = self.shared.events.lock() {
            events.push_back(Event::Link(PendingLink {
                conn_id,
                address: BleAddress::new(address),
                rssi,
                dialed,
                dialect,
                control_in: control_rx,
                l2cap_in: l2cap_rx,
                data_in: data_rx,
                control_out,
                l2cap_out,
                data_out,
                l2cap_up,
                l2cap_opens: Arc::clone(&self.shared.l2cap_opens),
            }));
        }
        self.shared.events_ready.notify_one();
    }

    pub fn control_in(&self, conn_id: u32, bytes: &[u8]) {
        if let Ok(links) = self.shared.links.lock() {
            if let Some(ep) = links.get(&conn_id) {
                let _ = ep.control_in_tx.send(bytes.to_vec());
            }
        }
    }

    pub fn control_out(&self, conn_id: u32, out: &mut [u8]) -> usize {
        match self.out_queue(conn_id, |ep| Arc::clone(&ep.control_out)) {
            Some(queue) => drain(&queue, out),
            None => 0,
        }
    }

    pub fn l2cap_in(&self, conn_id: u32, bytes: &[u8]) {
        if let Ok(links) = self.shared.links.lock() {
            if let Some(ep) = links.get(&conn_id) {
                let _ = ep.l2cap_in_tx.send(bytes.to_vec());
            }
        }
    }

    pub fn l2cap_out(&self, conn_id: u32, out: &mut [u8]) -> usize {
        match self.out_queue(conn_id, |ep| Arc::clone(&ep.l2cap_out)) {
            Some(queue) => drain(&queue, out),
            None => 0,
        }
    }

    pub fn data_in(&self, conn_id: u32, bytes: &[u8]) {
        if let Ok(links) = self.shared.links.lock() {
            if let Some(ep) = links.get(&conn_id) {
                let _ = ep.data_in_tx.send(bytes.to_vec());
            }
        }
    }

    pub fn data_out(&self, conn_id: u32, out: &mut [u8]) -> usize {
        let queue = self
            .shared
            .links
            .lock()
            .ok()
            .and_then(|links| links.get(&conn_id).map(|ep| Arc::clone(&ep.data_out)));
        let Some(queue) = queue else {
            return 0;
        };
        let Ok(mut queue) = queue.lock() else {
            return 0;
        };
        let Some(message) = queue.pop_front() else {
            return 0;
        };
        let n = message.len().min(out.len());
        out[..n].copy_from_slice(&message[..n]);
        n
    }

    pub fn l2cap_up(&self, conn_id: u32) {
        let signal = self.out_signal(conn_id);
        if let Some(signal) = signal {
            signal.is_up.store(true, Ordering::Release);
            signal.notify.notify_one();
        }
    }

    pub fn disconnected(&self, conn_id: u32) {
        if let Ok(mut links) = self.shared.links.lock() {
            links.remove(&conn_id);
        }
    }

    pub fn push_dial(&self, address: [u8; 6]) {
        if let Ok(mut requests) = self.shared.dial_requests.lock() {
            requests.push_back(address);
        }
    }

    pub fn next_dial(&self, out: &mut [u8]) -> bool {
        if out.len() < 6 {
            return false;
        }
        let address = match self.shared.dial_requests.lock() {
            Ok(mut requests) => requests.pop_front(),
            Err(_) => None,
        };
        match address {
            Some(address) => {
                out[..6].copy_from_slice(&address);
                true
            }
            None => false,
        }
    }

    pub fn next_l2cap_open(&self, out: &mut [u8]) -> bool {
        if out.len() < 6 {
            return false;
        }
        let request = match self.shared.l2cap_opens.lock() {
            Ok(mut requests) => requests.pop_front(),
            Err(_) => None,
        };
        match request {
            Some((conn_id, psm)) => {
                out[..4].copy_from_slice(&conn_id.to_be_bytes());
                out[4..6].copy_from_slice(&psm.to_be_bytes());
                true
            }
            None => false,
        }
    }

    fn out_queue(
        &self,
        conn_id: u32,
        pick: impl Fn(&Endpoints) -> Arc<Mutex<VecDeque<u8>>>,
    ) -> Option<Arc<Mutex<VecDeque<u8>>>> {
        self.shared
            .links
            .lock()
            .ok()
            .and_then(|links| links.get(&conn_id).map(pick))
    }

    fn out_signal(&self, conn_id: u32) -> Option<Arc<LinkSignal>> {
        self.shared
            .links
            .lock()
            .ok()
            .and_then(|links| links.get(&conn_id).map(|ep| Arc::clone(&ep.l2cap_up)))
    }
}

impl Default for AndroidBleBridge {
    fn default() -> Self {
        Self::new()
    }
}

fn columba_greeting(dialed: bool, peer_identity: [u8; 16], rssi: Option<i8>) -> Vec<u8> {
    let capabilities = LinkCapabilities {
        l2cap: None,
        link_mtu: BLE_HW_MTU as u16,
    };
    let peer = BleIdentity::new(peer_identity);
    let msg = if dialed {
        Control::Welcome {
            identity: peer,
            endpoint: Endpoint::Android(AndroidHost::Android),
            capabilities,
            peer_rssi: rssi,
        }
    } else {
        Control::Hello {
            identity: peer,
            endpoint: Endpoint::Android(AndroidHost::Android),
            capabilities,
            peer_rssi: rssi,
        }
    };
    let mut buf = [0u8; CONTROL_MAX_LEN];
    let len = msg
        .encode(&mut buf)
        .expect("Columba greeting fits the native control envelope");
    buf[..len].to_vec()
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
    const MAX_PEERS: usize = limits::ANDROID_MAX_PEERS;
    type Error = AndroidBleError;
    type Link = AndroidBleLink;

    async fn set_radio_enabled(&mut self, enabled: bool) -> Result<(), AndroidBleError> {
        self.bridge.set_radio_enabled(enabled);
        Ok(())
    }

    async fn local_capabilities(
        &mut self,
        mut configured: LinkCapabilities,
    ) -> Result<LinkCapabilities, AndroidBleError> {
        let psm = self.bridge.await_psm().await;
        configured.l2cap = Psm::new(psm);
        Ok(configured)
    }

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), AndroidBleError> {
        self.bridge.set_advertising(enabled);
        Ok(())
    }

    async fn set_scanning(&mut self, enabled: bool) -> Result<(), AndroidBleError> {
        self.bridge.set_scanning(enabled);
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<AndroidBleLink> {
        loop {
            let event = self
                .bridge
                .shared
                .events
                .lock()
                .ok()
                .and_then(|mut events| events.pop_front());
            match event {
                Some(Event::Sighting { address, rssi }) => {
                    return BleEvent::Sighting { address, rssi };
                }
                Some(Event::DialFailed { address }) => {
                    return BleEvent::DialFailed { address };
                }
                Some(Event::Link(pending)) => {
                    let dialed = pending.dialed;
                    let peer_rssi = pending.rssi;
                    let link = AndroidBleLink {
                        conn_id: pending.conn_id,
                        address: pending.address,
                        dialect: pending.dialect,
                        control_in: pending.control_in,
                        l2cap_in: Some(pending.l2cap_in),
                        data_in: Some(pending.data_in),
                        control_out: pending.control_out,
                        l2cap_out: pending.l2cap_out,
                        data_out: pending.data_out,
                        l2cap_up: pending.l2cap_up,
                        l2cap_opens: pending.l2cap_opens,
                    };
                    if dialed {
                        return BleEvent::LinkReady {
                            link,
                            origin: Origin::Dialed,
                            peer_rssi,
                        };
                    }
                    return BleEvent::Inbound(link);
                }
                None => self.bridge.shared.events_ready.notified().await,
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) {
        self.bridge.push_dial(*address.octets());
    }
}

pub struct AndroidBleLink {
    conn_id: u32,
    address: BleAddress,
    dialect: Dialect,
    control_in: UnboundedReceiver<Vec<u8>>,
    l2cap_in: Option<UnboundedReceiver<Vec<u8>>>,
    data_in: Option<UnboundedReceiver<Vec<u8>>>,
    control_out: Arc<Mutex<VecDeque<u8>>>,
    l2cap_out: Arc<Mutex<VecDeque<u8>>>,
    data_out: Arc<Mutex<VecDeque<Vec<u8>>>>,
    l2cap_up: Arc<LinkSignal>,
    l2cap_opens: Arc<Mutex<VecDeque<(u32, u16)>>>,
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_radio_exposes_no_android_ble_work() {
        let bridge = AndroidBleBridge::new();

        bridge.set_radio_enabled(true);
        bridge.set_advertising(true);
        bridge.set_scanning(true);
        bridge.set_psm(0x0080);
        assert_eq!(
            bridge.radio_state(),
            RADIO_ENABLED | RADIO_ADVERTISING | RADIO_SCANNING
        );

        bridge.set_radio_enabled(false);

        assert_eq!(bridge.radio_state(), 0);
        assert!(bridge.shared.psm.lock().unwrap().is_none());
        assert!(bridge.shared.links.lock().unwrap().is_empty());
        assert!(bridge.shared.events.lock().unwrap().is_empty());
        assert!(bridge.shared.dial_requests.lock().unwrap().is_empty());
        assert!(bridge.shared.l2cap_opens.lock().unwrap().is_empty());
    }

    #[test]
    fn advertising_or_scanning_without_enabled_stays_invisible() {
        let bridge = AndroidBleBridge::new();

        bridge.set_advertising(true);
        bridge.set_scanning(true);

        assert_eq!(bridge.radio_state(), 0);
    }
}
