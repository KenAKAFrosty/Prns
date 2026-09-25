use std::collections::VecDeque;

use crate::fault::TransmissionOrdinal;
use crate::medium::EndpointId;
use crate::time::SimulationTick;
use crate::Reachability;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryCopy {
    Original,
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceptionDropReason {
    ScheduledFault,
    PendingCapacityReached,
    ReceiveQueueFull,
    EndpointClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediumEvent {
    ReachabilityChanged {
        first: EndpointId,
        second: EndpointId,
        reachability: Reachability,
        at: SimulationTick,
    },
    EndpointAttached {
        endpoint: EndpointId,
        channel_tag: Vec<u8>,
    },
    EndpointDetached {
        endpoint: EndpointId,
    },
    TimeAdvanced {
        from: SimulationTick,
        to: SimulationTick,
    },
    TransmissionAccepted {
        ordinal: TransmissionOrdinal,
        from: EndpointId,
        at: SimulationTick,
        frame: Vec<u8>,
    },
    ReceptionScheduled {
        ordinal: TransmissionOrdinal,
        to: EndpointId,
        copy: DeliveryCopy,
        deliver_at: SimulationTick,
    },
    ReceptionQueued {
        ordinal: TransmissionOrdinal,
        to: EndpointId,
        copy: DeliveryCopy,
        at: SimulationTick,
    },
    ReceptionDropped {
        ordinal: TransmissionOrdinal,
        to: EndpointId,
        copy: DeliveryCopy,
        at: SimulationTick,
        intended_for: SimulationTick,
        reason: ReceptionDropReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceSnapshot {
    pub discarded_events: u64,
    pub events: Vec<MediumEvent>,
}

pub(crate) struct TraceBuffer {
    capacity: usize,
    discarded_events: u64,
    events: VecDeque<MediumEvent>,
}

impl TraceBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            discarded_events: 0,
            events: VecDeque::with_capacity(capacity),
        }
    }

    pub(crate) fn push(&mut self, event: MediumEvent) {
        if self.events.len() == self.capacity {
            let _ = self.events.pop_front();
            self.discarded_events = self.discarded_events.saturating_add(1);
        }
        self.events.push_back(event);
    }

    pub(crate) fn snapshot(&self) -> TraceSnapshot {
        TraceSnapshot {
            discarded_events: self.discarded_events,
            events: self.events.iter().cloned().collect(),
        }
    }
}
