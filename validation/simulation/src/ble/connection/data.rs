use super::BleAddress;
use personal_rns::wire::WirePacketHeader;

/// The latest length-valid send attempt, not a delivery or authentication claim.
/// Retains only a parsed header, length and counter baseline; never packet payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleDataSendObservation {
    /// None means these bytes did not parse as a Reticulum header.
    pub header: Option<WirePacketHeader>,
    pub frame_length: usize,
    pub before: BleDataCounters,
}

#[derive(Default)]
pub(super) struct DirectionActivity {
    pub counters: BleDataCounters,
    pub last_send: Option<BleDataSendObservation>,
}

/// Cumulative data-characteristic activity, excluding handshake/control values.
/// Started minus completed sends is not an active-send count: cancellation is
/// allowed. Length-refused frames do not start; consumed malformed values still count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BleDataCounters {
    pub sends_started: u64,
    pub sends_completed: u64,
    pub fragments_queued: u64,
    pub values_consumed: u64,
    pub frames_reassembled: u64,
    /// At least one increment could not be represented. Exact-delta assertions
    /// must refuse this snapshot rather than treating saturated counts as exact.
    pub saturated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleConnectionDataSnapshot {
    pub dialer: BleAddress,
    pub listener: BleAddress,
    pub dialer_to_listener: BleDataCounters,
    pub listener_to_dialer: BleDataCounters,
    pub dialer_last_send: Option<BleDataSendObservation>,
    pub listener_last_send: Option<BleDataSendObservation>,
}

pub(in crate::ble) enum DataEvent {
    SendStarted,
    SendCompleted,
    FragmentQueued,
    ValueConsumed,
    FrameReassembled,
}

impl BleDataCounters {
    pub(super) fn record(&mut self, event: DataEvent) {
        let count = match event {
            DataEvent::SendStarted => &mut self.sends_started,
            DataEvent::SendCompleted => &mut self.sends_completed,
            DataEvent::FragmentQueued => &mut self.fragments_queued,
            DataEvent::ValueConsumed => &mut self.values_consumed,
            DataEvent::FrameReassembled => &mut self.frames_reassembled,
        };
        match count.checked_add(1) {
            Some(next) => *count = next,
            None => self.saturated = true,
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::ble) enum ConnectionSide {
    Dialer,
    Listener,
}

impl ConnectionSide {
    pub(super) fn outgoing(self) -> usize {
        match self {
            Self::Dialer => 0,
            Self::Listener => 1,
        }
    }

    pub(super) fn incoming(self) -> usize {
        match self {
            Self::Dialer => 1,
            Self::Listener => 0,
        }
    }
}
