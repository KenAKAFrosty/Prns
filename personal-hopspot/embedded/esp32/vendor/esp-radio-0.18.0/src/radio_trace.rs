use core::fmt;

use portable_atomic::{AtomicUsize, Ordering};

const RADIO_EVENT_CAPACITY: usize = 24;

#[derive(Debug, Eq, PartialEq)]
#[repr(usize)]
pub(crate) enum RadioEventKind {
    Vacant = 0,
    WifiDriverInitStarted = 1,
    WifiDriverInitialized = 2,
    WifiDriverDeinitStarted = 3,
    WifiDriverDeinitialized = 4,
    WifiDriverStarted = 5,
    WifiDriverStopped = 6,
    WifiPhyTransitionRepeated = 7,
    WifiRxBlockedByTx = 11,
    WifiRxUnblockedByTx = 12,
    WifiTxSaturated = 13,
    WifiTxAvailable = 14,
    WifiTxSubmitRefused = 15,
    WifiTxCompletionFailed = 16,
    WifiTxCompletionUnmatched = 17,
    BleControllerInitStarted = 18,
    BleControllerInitialized = 19,
    BleControllerEnabled = 20,
    BleControllerDeinitialized = 21,
    WifiTxCreditRecovered = 24,
}

impl RadioEventKind {
    fn from_raw(raw: usize) -> Self {
        match raw {
            1 => Self::WifiDriverInitStarted,
            2 => Self::WifiDriverInitialized,
            3 => Self::WifiDriverDeinitStarted,
            4 => Self::WifiDriverDeinitialized,
            5 => Self::WifiDriverStarted,
            6 => Self::WifiDriverStopped,
            7 => Self::WifiPhyTransitionRepeated,
            11 => Self::WifiRxBlockedByTx,
            12 => Self::WifiRxUnblockedByTx,
            13 => Self::WifiTxSaturated,
            14 => Self::WifiTxAvailable,
            15 => Self::WifiTxSubmitRefused,
            16 => Self::WifiTxCompletionFailed,
            17 => Self::WifiTxCompletionUnmatched,
            18 => Self::BleControllerInitStarted,
            19 => Self::BleControllerInitialized,
            20 => Self::BleControllerEnabled,
            21 => Self::BleControllerDeinitialized,
            24 => Self::WifiTxCreditRecovered,
            _ => Self::Vacant,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct RadioEvent {
    sequence: usize,
    kind: RadioEventKind,
    value: usize,
}

impl RadioEvent {
    fn vacant() -> Self {
        Self {
            sequence: 0,
            kind: RadioEventKind::Vacant,
            value: 0,
        }
    }
}

struct RadioEventSlot {
    sequence: AtomicUsize,
    kind: AtomicUsize,
    value: AtomicUsize,
}

impl RadioEventSlot {
    const fn new() -> Self {
        Self {
            sequence: AtomicUsize::new(0),
            kind: AtomicUsize::new(0),
            value: AtomicUsize::new(0),
        }
    }

    fn read(&self, sequence: usize) -> RadioEvent {
        if self.sequence.load(Ordering::Acquire) != sequence {
            return RadioEvent::vacant();
        }
        let kind = self.kind.load(Ordering::Relaxed);
        let value = self.value.load(Ordering::Relaxed);
        if self.sequence.load(Ordering::Acquire) != sequence {
            return RadioEvent::vacant();
        }
        RadioEvent {
            sequence,
            kind: RadioEventKind::from_raw(kind),
            value,
        }
    }
}

static RADIO_EVENT_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static RADIO_EVENTS: [RadioEventSlot; RADIO_EVENT_CAPACITY] =
    [const { RadioEventSlot::new() }; RADIO_EVENT_CAPACITY];
static BLE_COEX_STATUS_SET: AtomicUsize = AtomicUsize::new(0);
static BLE_COEX_STATUS_CLEARED: AtomicUsize = AtomicUsize::new(0);
static BLE_COEX_LAST_SET: AtomicUsize = AtomicUsize::new(0);
static BLE_COEX_LAST_CLEARED: AtomicUsize = AtomicUsize::new(0);
static WIFI_PHY_ENABLED: AtomicUsize = AtomicUsize::new(0);
static WIFI_PHY_DISABLED: AtomicUsize = AtomicUsize::new(0);
static WIFI_PHY_STATE: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct RadioTraceSnapshot {
    ble_coex_status_set: usize,
    ble_coex_status_cleared: usize,
    ble_coex_last_set: usize,
    ble_coex_last_cleared: usize,
    wifi_phy_enabled: usize,
    wifi_phy_disabled: usize,
    wifi_phy_active: bool,
    events: [RadioEvent; RADIO_EVENT_CAPACITY],
}

struct RecordedEvents<'a>(&'a [RadioEvent; RADIO_EVENT_CAPACITY]);

impl fmt::Debug for RecordedEvents<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(
                self.0
                    .iter()
                    .filter(|event| event.kind != RadioEventKind::Vacant),
            )
            .finish()
    }
}

impl fmt::Debug for RadioTraceSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RadioTraceSnapshot")
            .field("ble_coex_status_set", &self.ble_coex_status_set)
            .field("ble_coex_status_cleared", &self.ble_coex_status_cleared)
            .field("ble_coex_last_set", &self.ble_coex_last_set)
            .field("ble_coex_last_cleared", &self.ble_coex_last_cleared)
            .field("wifi_phy_enabled", &self.wifi_phy_enabled)
            .field("wifi_phy_disabled", &self.wifi_phy_disabled)
            .field("wifi_phy_active", &self.wifi_phy_active)
            .field("events", &RecordedEvents(&self.events))
            .finish()
    }
}

impl PartialEq for RadioTraceSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.events == other.events
    }
}

impl Eq for RadioTraceSnapshot {}

pub(crate) fn record(kind: RadioEventKind, value: usize) {
    let sequence = RADIO_EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    let slot = &RADIO_EVENTS[(sequence - 1) % RADIO_EVENT_CAPACITY];
    slot.sequence.store(0, Ordering::Release);
    slot.kind.store(kind as usize, Ordering::Relaxed);
    slot.value.store(value, Ordering::Relaxed);
    slot.sequence.store(sequence, Ordering::Release);
}

#[cfg(any(esp32c3, esp32s3))]
pub(crate) fn record_ble_coex_status(set: bool, typ: i32, status: i32) {
    let value = ((typ as usize) << 16) | status as usize;
    if set {
        BLE_COEX_STATUS_SET.fetch_add(1, Ordering::Relaxed);
        BLE_COEX_LAST_SET.store(value, Ordering::Relaxed);
    } else {
        BLE_COEX_STATUS_CLEARED.fetch_add(1, Ordering::Relaxed);
        BLE_COEX_LAST_CLEARED.store(value, Ordering::Relaxed);
    }
}

pub(crate) fn record_wifi_phy_state(enabled: bool) {
    let state = usize::from(enabled) + 1;
    if enabled {
        WIFI_PHY_ENABLED.fetch_add(1, Ordering::Relaxed);
    } else {
        WIFI_PHY_DISABLED.fetch_add(1, Ordering::Relaxed);
    }
    let previous = WIFI_PHY_STATE.swap(state, Ordering::Relaxed);
    if previous == state {
        record(
            RadioEventKind::WifiPhyTransitionRepeated,
            usize::from(enabled),
        );
    }
}

pub(crate) fn snapshot() -> RadioTraceSnapshot {
    let newest = RADIO_EVENT_SEQUENCE.load(Ordering::Acquire);
    let available = newest.min(RADIO_EVENT_CAPACITY);
    let oldest = newest.saturating_sub(available).saturating_add(1);
    RadioTraceSnapshot {
        ble_coex_status_set: BLE_COEX_STATUS_SET.load(Ordering::Relaxed),
        ble_coex_status_cleared: BLE_COEX_STATUS_CLEARED.load(Ordering::Relaxed),
        ble_coex_last_set: BLE_COEX_LAST_SET.load(Ordering::Relaxed),
        ble_coex_last_cleared: BLE_COEX_LAST_CLEARED.load(Ordering::Relaxed),
        wifi_phy_enabled: WIFI_PHY_ENABLED.load(Ordering::Relaxed),
        wifi_phy_disabled: WIFI_PHY_DISABLED.load(Ordering::Relaxed),
        wifi_phy_active: WIFI_PHY_STATE.load(Ordering::Relaxed) == 2,
        events: core::array::from_fn(|offset| {
            if offset >= available {
                return RadioEvent::vacant();
            }
            let sequence = oldest + offset;
            RADIO_EVENTS[(sequence - 1) % RADIO_EVENT_CAPACITY].read(sequence)
        }),
    }
}
