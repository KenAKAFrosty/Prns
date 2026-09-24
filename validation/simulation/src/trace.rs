use std::collections::VecDeque;

use crate::fault::TransmissionOrdinal;
use crate::medium::EndpointId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceptionDropReason {
    ScheduledFault,
    ReceiveQueueFull,
    EndpointClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediumEvent {
    EndpointAttached {
        endpoint: EndpointId,
        channel_tag: Vec<u8>,
    },
    EndpointDetached {
        endpoint: EndpointId,
    },
    TransmissionAccepted {
        ordinal: TransmissionOrdinal,
        from: EndpointId,
        frame: Vec<u8>,
    },
    ReceptionQueued {
        ordinal: TransmissionOrdinal,
        to: EndpointId,
    },
    ReceptionDropped {
        ordinal: TransmissionOrdinal,
        to: EndpointId,
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
