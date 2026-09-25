use std::collections::VecDeque;

use super::{BleAdvertisement, BleRadioId, BleRadioPower, BleScanState};
use crate::{Reachability, SimulationDurationInTicks, SimulationTick};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleObservationDropReason {
    ObservationQueueFull,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BleSimulationEvent {
    ReachabilityChanged {
        first: BleRadioId,
        second: BleRadioId,
        reachability: Reachability,
        at: SimulationTick,
    },
    RadioAttached {
        radio: BleRadioId,
    },
    RadioDetached {
        radio: BleRadioId,
    },
    RadioPowerChanged {
        radio: BleRadioId,
        power: BleRadioPower,
    },
    ScanningChanged {
        radio: BleRadioId,
        scanning: BleScanState,
    },
    AdvertisingChanged {
        radio: BleRadioId,
        advertisement: Option<BleAdvertisement>,
        interval: Option<SimulationDurationInTicks>,
    },
    TimeAdvanced {
        from: SimulationTick,
        to: SimulationTick,
    },
    AdvertisementEmitted {
        radio: BleRadioId,
        at: SimulationTick,
        advertisement: BleAdvertisement,
    },
    ObservationQueued {
        advertiser: BleRadioId,
        scanner: BleRadioId,
        at: SimulationTick,
    },
    ObservationDropped {
        advertiser: BleRadioId,
        scanner: BleRadioId,
        at: SimulationTick,
        reason: BleObservationDropReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleTraceSnapshot {
    pub discarded_events: u64,
    pub events: Vec<BleSimulationEvent>,
}

pub(crate) struct BleTraceBuffer {
    capacity: usize,
    discarded_events: u64,
    events: VecDeque<BleSimulationEvent>,
}

impl BleTraceBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            discarded_events: 0,
            events: VecDeque::with_capacity(capacity),
        }
    }

    pub(crate) fn push(&mut self, event: BleSimulationEvent) {
        if self.events.len() == self.capacity {
            let _ = self.events.pop_front();
            self.discarded_events = self.discarded_events.saturating_add(1);
        }
        self.events.push_back(event);
    }

    pub(crate) fn snapshot(&self) -> BleTraceSnapshot {
        BleTraceSnapshot {
            discarded_events: self.discarded_events,
            events: self.events.iter().cloned().collect(),
        }
    }
}
