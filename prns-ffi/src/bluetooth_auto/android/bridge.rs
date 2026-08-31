use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Notify;

use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, RadioMode, ScanningMode, DEFAULT_GROUP_TAG, GROUP_TAG_LEN,
};
use prns_core::interfaces::bluetooth_auto::{BleAddress, BleIdentity, PeerProtocol};

use super::outbound::{BoundedByteQueue, BoundedMessageQueue};
use super::{RADIO_ADVERTISING, RADIO_ENABLED, RADIO_SCANNING};

const CONTROL_IN_DEPTH: usize = 8;
const DATA_IN_DEPTH: usize = 16;
const OUTBOUND_BYTE_CAP: usize = 8 * prns_core::interfaces::bluetooth_auto::BLE_HW_MTU;
const OUTBOUND_FRAME_DEPTH: usize = 16;
pub(super) const PEER_CAPACITY: usize = 7;
const LIFECYCLE_EVENT_DEPTH: usize = 3 * PEER_CAPACITY;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AndroidBleIngressAdmission {
    Accepted,
    Full,
    Closed,
}

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
    address: BleAddress,
    control_in_tx: Sender<Vec<u8>>,
    l2cap_in_tx: Sender<Vec<u8>>,
    data_in_tx: Sender<Vec<u8>>,
    pub(super) control_out: Arc<BoundedMessageQueue>,
    l2cap_out: Arc<BoundedByteQueue>,
    pub(super) data_out: Arc<BoundedMessageQueue>,
    l2cap_up: Arc<LinkSignal>,
}

impl Endpoints {
    fn close(&self) {
        self.control_out.close();
        self.l2cap_out.close();
        self.data_out.close();
    }
}

pub(super) enum LinkRecord {
    Active(Endpoints),
    Closing { address: BleAddress },
}

impl LinkRecord {
    fn address(&self) -> BleAddress {
        match self {
            Self::Active(endpoints) => endpoints.address,
            Self::Closing { address } => *address,
        }
    }

    pub(super) fn active(&self) -> Option<&Endpoints> {
        match self {
            Self::Active(endpoints) => Some(endpoints),
            Self::Closing { .. } => None,
        }
    }

    fn request_close(&mut self) -> bool {
        let Self::Active(endpoints) = self else {
            return false;
        };
        let address = endpoints.address;
        let previous = core::mem::replace(self, Self::Closing { address });
        if let Self::Active(endpoints) = previous {
            endpoints.close();
        }
        true
    }

    fn close(self) {
        if let Self::Active(endpoints) = self {
            endpoints.close();
        }
    }
}

#[derive(Default)]
pub(super) struct CloseRequests {
    pending: VecDeque<u32>,
}

impl CloseRequests {
    fn enqueue(&mut self, conn_id: u32) -> bool {
        if self.pending.contains(&conn_id) {
            return true;
        }
        if self.pending.len() >= PEER_CAPACITY {
            return false;
        }
        self.pending.push_back(conn_id);
        true
    }

    fn remove(&mut self, conn_id: u32) {
        self.pending.retain(|pending| *pending != conn_id);
    }

    fn next(&mut self) -> Option<u32> {
        self.pending.pop_front()
    }

    fn clear(&mut self) {
        self.pending.clear();
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
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
    pub(super) control_out: Arc<BoundedMessageQueue>,
    pub(super) l2cap_out: Arc<BoundedByteQueue>,
    pub(super) data_out: Arc<BoundedMessageQueue>,
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
    local_group_tag: Mutex<[u8; GROUP_TAG_LEN]>,
    pub(super) psm: Mutex<Option<u16>>,
    psm_ready: Notify,
    pub(super) links: Mutex<HashMap<u32, LinkRecord>>,
    pub(super) events: Mutex<VecDeque<Event>>,
    pub(super) events_ready: Notify,
    pub(super) dial_requests: Mutex<VecDeque<[u8; 6]>>,
    pub(super) close_requests: Mutex<CloseRequests>,
    pub(super) l2cap_opens: Arc<Mutex<VecDeque<(u32, u16)>>>,
    work: Arc<WorkSignal>,
    ingress_pressure_events: AtomicU64,
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
                local_group_tag: Mutex::new(DEFAULT_GROUP_TAG),
                psm: Mutex::new(None),
                psm_ready: Notify::new(),
                links: Mutex::new(HashMap::new()),
                events: Mutex::new(VecDeque::new()),
                events_ready: Notify::new(),
                dial_requests: Mutex::new(VecDeque::new()),
                close_requests: Mutex::new(CloseRequests::default()),
                l2cap_opens: Arc::new(Mutex::new(VecDeque::new())),
                work: Arc::new(WorkSignal::default()),
                ingress_pressure_events: AtomicU64::new(0),
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

    pub fn set_local_group_tag(&self, tag: [u8; GROUP_TAG_LEN]) {
        let changed = if let Ok(mut slot) = self.shared.local_group_tag.lock() {
            if *slot == tag {
                false
            } else {
                *slot = tag;
                true
            }
        } else {
            false
        };
        if changed {
            // The advertised manufacturer payload is snapshotted when advertising starts.
            // Wake the radio pump so Kotlin rebuilds it without cycling the radio.
            self.shared.work.wake();
        }
    }

    pub fn local_group_tag(&self, out: &mut [u8]) -> usize {
        if out.len() < GROUP_TAG_LEN {
            return 0;
        }
        let tag = self
            .shared
            .local_group_tag
            .lock()
            .map(|slot| *slot)
            .unwrap_or(DEFAULT_GROUP_TAG);
        out[..GROUP_TAG_LEN].copy_from_slice(&tag);
        GROUP_TAG_LEN
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
            links.drain().for_each(|(_, link)| link.close());
        }
        if let Ok(mut events) = self.shared.events.lock() {
            events.clear();
        }
        if let Ok(mut requests) = self.shared.dial_requests.lock() {
            requests.clear();
        }
        if let Ok(mut requests) = self.shared.close_requests.lock() {
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
            let address = BleAddress::new(address);
            if let Some(Event::Sighting {
                rssi: existing_rssi,
                ..
            }) = events.iter_mut().find(|event| {
                matches!(event, Event::Sighting { address: existing, .. } if *existing == address)
            }) {
                *existing_rssi = rssi;
                return;
            }
            if events.len() >= LIFECYCLE_EVENT_DEPTH {
                return;
            }
            events.push_back(Event::Sighting { address, rssi });
        }
        self.shared.events_ready.notify_one();
    }

    pub fn dial_failed(&self, address: [u8; 6]) -> bool {
        if let Ok(mut events) = self.shared.events.lock() {
            let address = BleAddress::new(address);
            if events.iter().any(
                |event| matches!(event, Event::DialFailed { address: existing } if *existing == address),
            ) {
                return true;
            }
            if events.len() >= LIFECYCLE_EVENT_DEPTH && !remove_oldest_sighting(&mut events) {
                return false;
            }
            events.push_back(Event::DialFailed { address });
        } else {
            return false;
        }
        self.shared.events_ready.notify_one();
        true
    }

    pub fn link_up(&self, conn_id: u32, address: [u8; 6], rssi: Option<i8>, dialed: bool) -> bool {
        self.link_up_with_protocol(conn_id, address, rssi, dialed, PeerProtocol::Native, None)
    }

    pub fn columba_link_up(
        &self,
        conn_id: u32,
        address: [u8; 6],
        rssi: Option<i8>,
        dialed: bool,
        peer_identity: [u8; 16],
    ) -> bool {
        self.link_up_with_protocol(
            conn_id,
            address,
            rssi,
            dialed,
            PeerProtocol::Columba,
            Some(peer_identity),
        )
    }

    fn link_up_with_protocol(
        &self,
        conn_id: u32,
        address: [u8; 6],
        rssi: Option<i8>,
        dialed: bool,
        peer_protocol: PeerProtocol,
        peer_identity: Option<[u8; 16]>,
    ) -> bool {
        let (control_tx, control_rx) = channel::<Vec<u8>>(CONTROL_IN_DEPTH);
        let (l2cap_tx, l2cap_rx) = channel::<Vec<u8>>(DATA_IN_DEPTH);
        let (data_tx, data_rx) = channel::<Vec<u8>>(DATA_IN_DEPTH);
        let control_out = Arc::new(BoundedMessageQueue::with_byte_limit(OUTBOUND_BYTE_CAP));
        let l2cap_out = Arc::new(BoundedByteQueue::new(OUTBOUND_BYTE_CAP));
        let data_out = Arc::new(BoundedMessageQueue::with_count_limit(OUTBOUND_FRAME_DEPTH));
        let l2cap_up = Arc::new(LinkSignal {
            is_up: AtomicBool::new(false),
            notify: Notify::new(),
        });
        let peer_identity = peer_identity.map(BleIdentity::new);
        if let Ok(mut links) = self.shared.links.lock() {
            if links.len() >= PEER_CAPACITY || links.contains_key(&conn_id) {
                return false;
            }
            let replaced = links.insert(
                conn_id,
                LinkRecord::Active(Endpoints {
                    address: BleAddress::new(address),
                    control_in_tx: control_tx,
                    l2cap_in_tx: l2cap_tx,
                    data_in_tx: data_tx,
                    control_out: Arc::clone(&control_out),
                    l2cap_out: Arc::clone(&l2cap_out),
                    data_out: Arc::clone(&data_out),
                    l2cap_up: Arc::clone(&l2cap_up),
                }),
            );
            debug_assert!(replaced.is_none());
        } else {
            return false;
        }
        if let Ok(mut events) = self.shared.events.lock() {
            if events.len() >= LIFECYCLE_EVENT_DEPTH && !remove_oldest_sighting(&mut events) {
                if let Ok(mut links) = self.shared.links.lock() {
                    if let Some(link) = links.remove(&conn_id) {
                        link.close();
                    }
                }
                return false;
            }
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
        } else {
            if let Ok(mut links) = self.shared.links.lock() {
                if let Some(link) = links.remove(&conn_id) {
                    link.close();
                }
            }
            return false;
        }
        self.shared.events_ready.notify_one();
        true
    }

    pub fn control_in(&self, conn_id: u32, bytes: &[u8]) -> AndroidBleIngressAdmission {
        if let Ok(links) = self.shared.links.lock() {
            if let Some(ep) = links.get(&conn_id).and_then(LinkRecord::active) {
                return self.try_ingress(&ep.control_in_tx, bytes);
            }
        }
        AndroidBleIngressAdmission::Closed
    }

    pub fn control_out(&self, conn_id: u32, out: &mut [u8]) -> usize {
        match self.out_message_queue(conn_id, |ep| Arc::clone(&ep.control_out)) {
            Some(queue) => queue.peek(out),
            None => 0,
        }
    }

    pub fn commit_control_out(&self, conn_id: u32) -> bool {
        match self.out_message_queue(conn_id, |ep| Arc::clone(&ep.control_out)) {
            Some(queue) => queue.commit(),
            None => false,
        }
    }

    pub fn l2cap_in(&self, conn_id: u32, bytes: &[u8]) -> bool {
        let sender = self.shared.links.lock().ok().and_then(|links| {
            links
                .get(&conn_id)
                .and_then(LinkRecord::active)
                .map(|ep| ep.l2cap_in_tx.clone())
        });
        sender.is_some_and(|sender| sender.blocking_send(bytes.to_vec()).is_ok())
    }

    pub fn l2cap_out(&self, conn_id: u32, out: &mut [u8]) -> usize {
        match self.out_byte_queue(conn_id, |ep| Arc::clone(&ep.l2cap_out)) {
            Some(queue) => queue.drain(out),
            None => 0,
        }
    }

    pub fn data_in(&self, conn_id: u32, bytes: &[u8]) -> AndroidBleIngressAdmission {
        if let Ok(links) = self.shared.links.lock() {
            if let Some(ep) = links.get(&conn_id).and_then(LinkRecord::active) {
                return self.try_ingress(&ep.data_in_tx, bytes);
            }
        }
        AndroidBleIngressAdmission::Closed
    }

    pub fn data_out(&self, conn_id: u32, out: &mut [u8]) -> usize {
        let queue = self.shared.links.lock().ok().and_then(|links| {
            links
                .get(&conn_id)
                .and_then(LinkRecord::active)
                .map(|ep| Arc::clone(&ep.data_out))
        });
        let Some(queue) = queue else {
            return 0;
        };
        queue.peek(out)
    }

    pub fn commit_data_out(&self, conn_id: u32) -> bool {
        let queue = self.shared.links.lock().ok().and_then(|links| {
            links
                .get(&conn_id)
                .and_then(LinkRecord::active)
                .map(|ep| Arc::clone(&ep.data_out))
        });
        match queue {
            Some(queue) => queue.commit(),
            None => false,
        }
    }

    pub fn l2cap_up(&self, conn_id: u32) {
        let signal = self.out_signal(conn_id);
        if let Some(signal) = signal {
            signal.is_up.store(true, Ordering::Release);
            signal.notify.notify_one();
        }
    }

    pub fn disconnected(&self, conn_id: u32) {
        let removed = if let Ok(mut links) = self.shared.links.lock() {
            let removed = links.remove(&conn_id);
            if let Ok(mut closes) = self.shared.close_requests.lock() {
                closes.remove(conn_id);
            }
            removed
        } else {
            None
        };
        if let Some(link) = removed {
            link.close();
        }
        self.shared.work.wake();
    }

    /// Policy rejected a link: drop Rust endpoints and ask Java to tear down the physical radio.
    pub fn close_by_address(&self, address: [u8; 6]) -> bool {
        let target = BleAddress::new(address);
        let Ok(mut links) = self.shared.links.lock() else {
            return false;
        };
        let Ok(mut requests) = self.shared.close_requests.lock() else {
            return false;
        };
        let mut queued = false;
        let mut complete = true;
        for (conn_id, link) in links.iter_mut() {
            if link.address() == target && matches!(link, LinkRecord::Active(_)) {
                if requests.enqueue(*conn_id) {
                    queued |= link.request_close();
                } else {
                    complete = false;
                }
            }
        }
        drop(requests);
        drop(links);
        if queued {
            self.shared.work.wake();
        }
        complete
    }

    /// Close every active peer link without clearing the L2CAP PSM or radio mode.
    pub fn close_all_peer_links(&self) {
        if let Ok(mut requests) = self.shared.dial_requests.lock() {
            requests.clear();
        }
        if let Ok(mut events) = self.shared.events.lock() {
            events.clear();
        }
        let Ok(mut links) = self.shared.links.lock() else {
            return;
        };
        let Ok(mut closes) = self.shared.close_requests.lock() else {
            return;
        };
        let mut queued = false;
        for (conn_id, link) in links.iter_mut() {
            if matches!(link, LinkRecord::Active(_)) && closes.enqueue(*conn_id) {
                queued |= link.request_close();
            }
        }
        drop(closes);
        drop(links);
        if queued {
            self.shared.work.wake();
        }
    }

    pub fn next_close(&self) -> Option<u32> {
        self.shared
            .close_requests
            .lock()
            .ok()
            .and_then(|mut requests| requests.next())
    }

    pub fn push_dial(&self, address: [u8; 6]) -> bool {
        if let Ok(mut requests) = self.shared.dial_requests.lock() {
            if requests.contains(&address) {
                return true;
            }
            if requests.len() >= PEER_CAPACITY {
                return false;
            }
            requests.push_back(address);
        } else {
            return false;
        }
        self.shared.work.wake();
        true
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

    pub fn ingress_pressure_events(&self) -> u64 {
        self.shared.ingress_pressure_events.load(Ordering::Relaxed)
    }

    fn try_ingress(&self, sender: &Sender<Vec<u8>>, bytes: &[u8]) -> AndroidBleIngressAdmission {
        match sender.try_reserve() {
            Ok(permit) => {
                permit.send(bytes.to_vec());
                AndroidBleIngressAdmission::Accepted
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                let _ = self.shared.ingress_pressure_events.fetch_update(
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                    |count| Some(count.saturating_add(1)),
                );
                AndroidBleIngressAdmission::Full
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                AndroidBleIngressAdmission::Closed
            }
        }
    }

    fn out_message_queue(
        &self,
        conn_id: u32,
        pick: impl Fn(&Endpoints) -> Arc<BoundedMessageQueue>,
    ) -> Option<Arc<BoundedMessageQueue>> {
        self.shared
            .links
            .lock()
            .ok()
            .and_then(|links| links.get(&conn_id).and_then(LinkRecord::active).map(pick))
    }

    fn out_byte_queue(
        &self,
        conn_id: u32,
        pick: impl Fn(&Endpoints) -> Arc<BoundedByteQueue>,
    ) -> Option<Arc<BoundedByteQueue>> {
        self.shared
            .links
            .lock()
            .ok()
            .and_then(|links| links.get(&conn_id).and_then(LinkRecord::active).map(pick))
    }

    fn out_signal(&self, conn_id: u32) -> Option<Arc<LinkSignal>> {
        self.shared.links.lock().ok().and_then(|links| {
            links
                .get(&conn_id)
                .and_then(LinkRecord::active)
                .map(|ep| Arc::clone(&ep.l2cap_up))
        })
    }
}

impl Default for AndroidBleBridge {
    fn default() -> Self {
        Self::new()
    }
}

fn remove_oldest_sighting(events: &mut VecDeque<Event>) -> bool {
    let Some(index) = events
        .iter()
        .position(|event| matches!(event, Event::Sighting { .. }))
    else {
        return false;
    };
    events.remove(index);
    true
}
