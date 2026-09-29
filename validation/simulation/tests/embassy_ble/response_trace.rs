use std::cell::RefCell;
use std::rc::Rc;

use personal_rns::engine::{CommandId, PacketReceiptDelivered, SendRequestFailure, Settlement};
use personal_rns::routing::links::request::RequestId;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{Diagnostic, Message, PrnsEvent};
use tokio::sync::Notify;

const EVENT_CAPACITY: usize = 8;
const MAX_RESPONSE_BODY_BYTES: usize = 2048;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ResponseEvent {
    Whole {
        link: LinkId,
        request: RequestId,
        bytes: Vec<u8>,
    },
    Segment {
        link: LinkId,
        request: RequestId,
        index: u64,
        total: u64,
        bytes: Vec<u8>,
    },
    Settled {
        command: CommandId,
        result: Result<PacketReceiptDelivered, SendRequestFailure>,
    },
}

struct Inner {
    events: RefCell<Vec<ResponseEvent>>,
    changed: Notify,
}

// Raw SendRequest commands leave their journals visible to the application;
// normal request futures consume theirs in the real runtime's completion path.
#[derive(Clone)]
pub(super) struct ResponseTrace(Rc<Inner>);

impl ResponseTrace {
    pub(super) fn new() -> Self {
        Self(Rc::new(Inner {
            events: RefCell::new(Vec::with_capacity(EVENT_CAPACITY)),
            changed: Notify::new(),
        }))
    }

    pub(super) fn observe(&self, event: &PrnsEvent<'_>) {
        let event = match event {
            PrnsEvent::Message(Message::Response {
                link_id,
                request_id,
                data,
            }) => {
                assert!(
                    data.len() <= MAX_RESPONSE_BODY_BYTES,
                    "bounded raw response"
                );
                ResponseEvent::Whole {
                    link: *link_id,
                    request: *request_id,
                    bytes: data.to_vec(),
                }
            }
            PrnsEvent::Message(Message::ResponseSegment {
                link_id,
                request_id,
                segment_index,
                total_segments,
                data,
            }) => {
                assert!(
                    data.len() <= MAX_RESPONSE_BODY_BYTES,
                    "bounded raw-response segment"
                );
                ResponseEvent::Segment {
                    link: *link_id,
                    request: *request_id,
                    index: *segment_index,
                    total: *total_segments,
                    bytes: data.to_vec(),
                }
            }
            PrnsEvent::Diagnostic(Diagnostic::CommandSettled {
                id,
                settlement: Settlement::SendRequest(result),
            }) => ResponseEvent::Settled {
                command: *id,
                result: *result,
            },
            _ => return,
        };
        let mut events = self.0.events.borrow_mut();
        assert!(
            events.len() < EVENT_CAPACITY,
            "bounded raw-response journal"
        );
        events.push(event);
        self.0.changed.notify_one();
    }

    pub(super) async fn completed(&self) -> Vec<ResponseEvent> {
        loop {
            let changed = self.0.changed.notified();
            if self
                .0
                .events
                .borrow()
                .iter()
                .any(|event| matches!(event, ResponseEvent::Settled { .. }))
            {
                return self.0.events.borrow_mut().drain(..).collect();
            }
            changed.await;
        }
    }

    pub(super) async fn next(&self) -> ResponseEvent {
        loop {
            let changed = self.0.changed.notified();
            if !self.is_empty() {
                return self.0.events.borrow_mut().remove(0);
            }
            changed.await;
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.events.borrow().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: LinkId = LinkId::new([0xA1; 16]);
    const REQUEST: RequestId = RequestId([0xB2; 16]);
    const WAIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

    fn whole(data: &[u8]) -> PrnsEvent<'_> {
        PrnsEvent::Message(Message::Response {
            link_id: LINK,
            request_id: REQUEST,
            data,
        })
    }

    #[test]
    fn whole_responses_remain_visible_at_the_observers_payload_boundary() {
        let trace = ResponseTrace::new();
        let bytes = vec![0x37; MAX_RESPONSE_BODY_BYTES];
        trace.observe(&whole(&bytes));
        assert_eq!(
            *trace.0.events.borrow(),
            [ResponseEvent::Whole {
                link: LINK,
                request: REQUEST,
                bytes
            }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn next_preserves_order_and_consumes_only_one_event() {
        let trace = ResponseTrace::new();
        for bytes in [b"first".as_slice(), b"second"] {
            trace.observe(&whole(bytes));
        }
        for bytes in [b"first".as_slice(), b"second"] {
            assert_eq!(
                tokio::time::timeout(WAIT_BUDGET, trace.next())
                    .await
                    .unwrap(),
                ResponseEvent::Whole {
                    link: LINK,
                    request: REQUEST,
                    bytes: bytes.to_vec()
                }
            );
        }
        assert!(trace.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn next_wakes_when_an_event_arrives_after_it_is_polled() {
        let trace = ResponseTrace::new();
        let started = tokio::time::Instant::now();
        let (observed, ()) = tokio::time::timeout(WAIT_BUDGET, async {
            tokio::join!(
                biased;
                trace.next(),
                async { trace.observe(&whole(b"later")); }
            )
        })
        .await
        .unwrap();
        assert_eq!(started.elapsed(), std::time::Duration::ZERO);
        assert_eq!(
            observed,
            ResponseEvent::Whole {
                link: LINK,
                request: REQUEST,
                bytes: b"later".to_vec()
            }
        );
        assert!(trace.is_empty());
    }

    #[test]
    #[should_panic(expected = "bounded raw response")]
    fn observer_refuses_oversized_bodies() {
        ResponseTrace::new().observe(&whole(&[0; MAX_RESPONSE_BODY_BYTES + 1]));
    }

    #[test]
    #[should_panic(expected = "bounded raw-response journal")]
    fn observer_refuses_trace_eviction() {
        let trace = ResponseTrace::new();
        for _ in 0..EVENT_CAPACITY {
            trace.observe(&whole(&[]));
        }
        assert_eq!(trace.0.events.borrow().len(), EVENT_CAPACITY);
        trace.observe(&whole(&[]));
    }
}
