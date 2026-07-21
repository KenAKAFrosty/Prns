use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Notify;

use prns_core::interfaces::bluetooth_auto::{AdvertisingMode, RadioMode, ScanningMode};
use prns_core::interfaces::bluetooth_auto::{BleAddress, BleIdentity, PeerProtocol};

use super::{RADIO_ADVERTISING, RADIO_ENABLED, RADIO_SCANNING};

const CONTROL_IN_DEPTH: usize = 8;
const DATA_IN_DEPTH: usize = 16;

pub(super) struct LinkSignal {
    pub(super) is_up: AtomicBool,
    notify: Notify,
}

#[derive(Default)]
pub(super) struct WorkSignal {
    generation: Mutex<u64>,
    ready: Condvar,
}

impl WorkSignal {
    fn generation(&self) -> u64 {
        self.generation.lock().map(|value| *value).unwrap_or(0)
    }

    pub(super) fn wake(&self) {
        if let Ok(mut generation) = self.generation.lock() {
            *generation = generation.wrapping_add(1);
            self.ready.notify_all();
        }
    }

    fn wait(&self, observed: u64, timeout: Option<Duration>) -> u64 {
        let Ok(mut generation) = self.generation.lock() else {
            return observed.wrapping_add(1);
        };
        if let Some(timeout) = timeout {
            if *generation == observed {
                let Ok((next, _)) = self.ready.wait_timeout(generation, timeout) else {
                    return observed.wrapping_add(1);
                };
                generation = next;
            }
        } else {
            while *generation == observed {
                let Ok(next) = self.ready.wait(generation) else {
                    return observed.wrapping_add(1);
                };
                generation = next;
            }
        }
        *generation
    }
}

pub(super) struct Endpoints {
    control_in_tx: Sender<Vec<u8>>,
    l2cap_in_tx: Sender<Vec<u8>>,
    data_in_tx: Sender<Vec<u8>>,
    control_out: Arc<Mutex<VecDeque<u8>>>,
    l2cap_out: Arc<Mutex<VecDeque<u8>>>,
    data_out: Arc<Mutex<VecDeque<Vec<u8>>>>,
    l2cap_up: Arc<LinkSignal>,
}

pub(super) struct PendingLink {
    pub(super) conn_id: u32,
    pub(super) address: BleAddress,
    pub(super) rssi: Option<i8>,
    pub(super) dialed: bool,
    pub(super) peer_protocol: PeerProtocol,
    pub(super) peer_identity: Option<BleIdentity>,
    pub(super) control_in: Receiver<Vec<u8>>,
    pub(super) l2cap_in: Receiver<Vec<u8>>,
    pub(super) data_in: Receiver<Vec<u8>>,
    pub(super) control_out: Arc<Mutex<VecDeque<u8>>>,
    pub(super) l2cap_out: Arc<Mutex<VecDeque<u8>>>,
    pub(super) data_out: Arc<Mutex<VecDeque<Vec<u8>>>>,
    pub(super) l2cap_up: Arc<LinkSignal>,
    pub(super) l2cap_opens: Arc<Mutex<VecDeque<(u32, u16)>>>,
    pub(super) work: Arc<WorkSignal>,
}

pub(super) enum Event {
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

pub(super) struct Shared {
    radio: Mutex<RadioState>,
    local_identity: Mutex<Option<[u8; 16]>>,
    pub(super) psm: Mutex<Option<u16>>,
    psm_ready: Notify,
    pub(super) links: Mutex<HashMap<u32, Endpoints>>,
    pub(super) events: Mutex<VecDeque<Event>>,
    pub(super) events_ready: Notify,
    pub(super) dial_requests: Mutex<VecDeque<[u8; 6]>>,
    pub(super) l2cap_opens: Arc<Mutex<VecDeque<(u32, u16)>>>,
    work: Arc<WorkSignal>,
}

pub struct AndroidBleBridge {
    pub(super) shared: Arc<Shared>,
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
                work: Arc::new(WorkSignal::default()),
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

    pub fn set_radio_mode(&self, mode: RadioMode) {
        let enabled = mode.is_on();
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
        self.shared.work.wake();
    }

    pub fn set_advertising(&self, mode: AdvertisingMode) {
        if let Ok(mut radio) = self.shared.radio.lock() {
            radio.advertising = mode.is_on();
        }
        self.shared.work.wake();
    }

    pub fn set_scanning(&self, mode: ScanningMode) {
        if let Ok(mut radio) = self.shared.radio.lock() {
            radio.scanning = mode.is_on();
        }
        self.shared.work.wake();
    }

    pub fn radio_state(&self) -> u32 {
        self.shared
            .radio
            .lock()
            .map(|state| state.bits())
            .unwrap_or(0)
    }

    pub fn work_generation(&self) -> u64 {
        self.shared.work.generation()
    }

    pub fn wait_for_work(&self, observed: u64, timeout_millis: u64) -> u64 {
        let timeout = (timeout_millis != 0).then(|| Duration::from_millis(timeout_millis));
        self.shared.work.wait(observed, timeout)
    }

    pub fn wake_work(&self) {
        self.shared.work.wake();
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
        self.link_up_with_protocol(conn_id, address, rssi, dialed, PeerProtocol::Native, None);
    }

    pub fn columba_link_up(
        &self,
        conn_id: u32,
        address: [u8; 6],
        rssi: Option<i8>,
        dialed: bool,
        peer_identity: [u8; 16],
    ) {
        self.link_up_with_protocol(
            conn_id,
            address,
            rssi,
            dialed,
            PeerProtocol::Columba,
            Some(peer_identity),
        );
    }

    fn link_up_with_protocol(
        &self,
        conn_id: u32,
        address: [u8; 6],
        rssi: Option<i8>,
        dialed: bool,
        peer_protocol: PeerProtocol,
        peer_identity: Option<[u8; 16]>,
    ) {
        let (control_tx, control_rx) = channel::<Vec<u8>>(CONTROL_IN_DEPTH);
        let (l2cap_tx, l2cap_rx) = channel::<Vec<u8>>(DATA_IN_DEPTH);
        let (data_tx, data_rx) = channel::<Vec<u8>>(DATA_IN_DEPTH);
        let control_out = Arc::new(Mutex::new(VecDeque::new()));
        let l2cap_out = Arc::new(Mutex::new(VecDeque::new()));
        let data_out = Arc::new(Mutex::new(VecDeque::new()));
        let l2cap_up = Arc::new(LinkSignal {
            is_up: AtomicBool::new(false),
            notify: Notify::new(),
        });
        let peer_identity = peer_identity.map(BleIdentity::new);
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
                peer_protocol,
                peer_identity,
                control_in: control_rx,
                l2cap_in: l2cap_rx,
                data_in: data_rx,
                control_out,
                l2cap_out,
                data_out,
                l2cap_up,
                l2cap_opens: Arc::clone(&self.shared.l2cap_opens),
                work: Arc::clone(&self.shared.work),
            }));
        }
        self.shared.events_ready.notify_one();
    }

    pub fn control_in(&self, conn_id: u32, bytes: &[u8]) -> bool {
        if let Ok(links) = self.shared.links.lock() {
            if let Some(ep) = links.get(&conn_id) {
                return ep
                    .control_in_tx
                    .try_reserve()
                    .map(|permit| permit.send(bytes.to_vec()))
                    .is_ok();
            }
        }
        false
    }

    pub fn control_out(&self, conn_id: u32, out: &mut [u8]) -> usize {
        match self.out_queue(conn_id, |ep| Arc::clone(&ep.control_out)) {
            Some(queue) => drain(&queue, out),
            None => 0,
        }
    }

    pub fn l2cap_in(&self, conn_id: u32, bytes: &[u8]) -> bool {
        if let Ok(links) = self.shared.links.lock() {
            if let Some(ep) = links.get(&conn_id) {
                return ep
                    .l2cap_in_tx
                    .try_reserve()
                    .map(|permit| permit.send(bytes.to_vec()))
                    .is_ok();
            }
        }
        false
    }

    pub fn l2cap_out(&self, conn_id: u32, out: &mut [u8]) -> usize {
        match self.out_queue(conn_id, |ep| Arc::clone(&ep.l2cap_out)) {
            Some(queue) => drain(&queue, out),
            None => 0,
        }
    }

    pub fn data_in(&self, conn_id: u32, bytes: &[u8]) -> bool {
        if let Ok(links) = self.shared.links.lock() {
            if let Some(ep) = links.get(&conn_id) {
                return ep
                    .data_in_tx
                    .try_reserve()
                    .map(|permit| permit.send(bytes.to_vec()))
                    .is_ok();
            }
        }
        false
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
        self.shared.work.wake();
    }

    pub fn push_dial(&self, address: [u8; 6]) {
        if let Ok(mut requests) = self.shared.dial_requests.lock() {
            requests.push_back(address);
        }
        self.shared.work.wake();
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
