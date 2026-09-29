use std::cell::RefCell;
use std::rc::Rc;

use personal_rns::routing::links::request::RequestId;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{Message, PrnsEvent};
use tokio::sync::Notify;

enum State {
    Idle,
    Armed {
        link: LinkId,
        path: RequestPathHash,
    },
    Observed {
        link: LinkId,
        path: RequestPathHash,
        request: RequestId,
    },
}

struct Inner {
    state: RefCell<State>,
    changed: Notify,
}

// Opt-in, single-request metadata only; ordinary traffic retains nothing.
#[derive(Clone)]
pub(super) struct RequestProbe(Rc<Inner>);

impl RequestProbe {
    pub(super) fn new() -> Self {
        Self(Rc::new(Inner {
            state: RefCell::new(State::Idle),
            changed: Notify::new(),
        }))
    }

    pub(super) fn arm(&self, link: LinkId, path: RequestPathHash) {
        let mut state = self.0.state.borrow_mut();
        assert!(
            matches!(*state, State::Idle),
            "one outstanding request observation"
        );
        *state = State::Armed { link, path };
    }

    pub(super) fn is_idle(&self) -> bool {
        matches!(*self.0.state.borrow(), State::Idle)
    }

    pub(super) fn observe(&self, event: &PrnsEvent<'_>) {
        let PrnsEvent::Message(Message::Request {
            link_id,
            request_id,
            path_hash,
            ..
        }) = event
        else {
            return;
        };
        let mut state = self.0.state.borrow_mut();
        let (link, path) = match *state {
            State::Idle => return,
            State::Armed { link, path } | State::Observed { link, path, .. } => (link, path),
        };
        if (link, path) != (*link_id, *path_hash) {
            return;
        }
        assert!(
            matches!(*state, State::Armed { .. }),
            "request observation must not be overwritten"
        );
        *state = State::Observed {
            link,
            path,
            request: *request_id,
        };
        self.0.changed.notify_one();
    }

    pub(super) async fn take(&self) -> (LinkId, RequestId) {
        loop {
            let changed = self.0.changed.notified();
            {
                let mut state = self.0.state.borrow_mut();
                match *state {
                    State::Observed { link, request, .. } => {
                        *state = State::Idle;
                        return (link, request);
                    }
                    State::Armed { .. } => {}
                    State::Idle => unreachable!("arm before observing a request"),
                }
            }
            changed.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::engine::InstantMillis;
    use personal_rns::units::RttMillis;
    use personal_rns::wire::DestinationHash;
    use std::future::{poll_fn, Future};
    use std::task::Poll;
    use std::time::Duration;

    const LINK: LinkId = LinkId::new([1; 16]);
    const REQUEST: RequestId = RequestId([2; 16]);

    fn request(link: LinkId, path: RequestPathHash) -> PrnsEvent<'static> {
        PrnsEvent::Message(Message::Request {
            destination: DestinationHash::new([3; 16]),
            link_id: link,
            request_id: REQUEST,
            requester: None,
            path_hash: path,
            requested_at: InstantMillis(100),
            rtt: RttMillis::new(10),
            data: b"not retained",
        })
    }

    #[tokio::test(start_paused = true)]
    async fn observation_is_scoped_consumed_and_reusable_without_time_advancing() {
        let probe = RequestProbe::new();
        let path = RequestPathHash::of("/watched");
        let started = tokio::time::Instant::now();
        probe.observe(&request(LINK, path));
        assert!(probe.is_idle());
        for _ in 0..2 {
            probe.arm(LINK, path);
            probe.observe(&request(LinkId::new([4; 16]), path));
            probe.observe(&request(LINK, RequestPathHash::of("/other")));
            let mut observation = Box::pin(probe.take());
            poll_fn(|cx| {
                assert!(observation.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            let (observed, ()) = tokio::time::timeout(Duration::from_secs(1), async {
                tokio::join!(biased; observation, async { probe.observe(&request(LINK, path)); })
            })
            .await
            .unwrap();
            assert_eq!(observed, (LINK, REQUEST));
            assert!(probe.is_idle());
        }
        probe.arm(LINK, path);
        probe.observe(&request(LINK, path));
        assert_eq!(probe.take().await, (LINK, REQUEST));
        assert!(probe.is_idle());
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[test]
    #[should_panic(expected = "request observation must not be overwritten")]
    fn a_second_matching_request_cannot_replace_unconsumed_metadata() {
        let probe = RequestProbe::new();
        let path = RequestPathHash::of("/watched");
        probe.arm(LINK, path);
        probe.observe(&request(LINK, path));
        probe.observe(&request(LINK, path));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_a_wait_does_not_discard_the_watch_or_its_later_observation() {
        let probe = RequestProbe::new();
        let path = RequestPathHash::of("/watched");
        probe.arm(LINK, path);
        let mut observation = Box::pin(probe.take());
        poll_fn(|cx| {
            assert!(observation.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(observation);
        probe.observe(&request(LINK, path));
        assert_eq!(probe.take().await, (LINK, REQUEST));
        assert!(probe.is_idle());
    }

    #[test]
    #[should_panic(expected = "one outstanding request observation")]
    fn arming_does_not_discard_an_existing_watch() {
        let probe = RequestProbe::new();
        let path = RequestPathHash::of("/watched");
        probe.arm(LINK, path);
        probe.arm(LINK, path);
    }
}
