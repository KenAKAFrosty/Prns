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

/// Borrowed trace contents. Iteration does not clone retained packet buffers.
pub struct TraceView<'a> {
    pub discarded_events: u64,
    events: &'a VecDeque<MediumEvent>,
}

impl TraceView<'_> {
    pub fn events(&self) -> impl ExactSizeIterator<Item = &MediumEvent> + DoubleEndedIterator {
        self.events.iter()
    }
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

    pub(crate) fn view(&self) -> TraceView<'_> {
        TraceView {
            discarded_events: self.discarded_events,
            events: &self.events,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_view_preserves_wrapped_order_loss_count_and_frame_storage() {
        let mut buffer = TraceBuffer::new(2);
        for ordinal in 0..3 {
            buffer.push(MediumEvent::TransmissionAccepted {
                ordinal: TransmissionOrdinal::new(ordinal),
                from: EndpointId(0),
                at: SimulationTick::from_ticks(ordinal),
                frame: vec![ordinal as u8; 64],
            });
        }
        let snapshot = buffer.snapshot();
        let view = buffer.view();
        assert_eq!(view.discarded_events, 1);
        assert_eq!(view.events().cloned().collect::<Vec<_>>(), snapshot.events);
        assert_eq!(
            view.events().skip(1).collect::<Vec<_>>(),
            [&snapshot.events[1]]
        );
        for (borrowed, stored) in view.events().zip(&buffer.events) {
            let (
                MediumEvent::TransmissionAccepted {
                    frame: borrowed, ..
                },
                MediumEvent::TransmissionAccepted { frame: stored, .. },
            ) = (borrowed, stored)
            else {
                unreachable!("all retained events are frames")
            };
            assert_eq!(borrowed.as_ptr(), stored.as_ptr());
        }
        assert_eq!(
            view.events().rev().collect::<Vec<_>>(),
            snapshot.events.iter().rev().collect::<Vec<_>>()
        );
    }

    #[test]
    fn empty_borrowed_view_has_no_loss_or_events() {
        let buffer = TraceBuffer::new(1);
        let view = buffer.view();
        assert_eq!(view.discarded_events, 0);
        assert_eq!(view.events().len(), 0);
    }
}
