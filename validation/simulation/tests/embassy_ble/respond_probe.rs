use std::cell::RefCell;
use std::rc::Rc;

use personal_rns::engine::{CommandId, RespondFailure, Settlement};
use personal_rns::runtime::{Diagnostic, PrnsEvent};
use tokio::sync::Notify;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Responded {
    pub command: CommandId,
    pub result: Result<(), RespondFailure>,
    pub at: tokio::time::Instant,
}

enum State {
    Idle,
    Armed,
    Observed(Responded),
}

struct Inner {
    state: RefCell<State>,
    changed: Notify,
}

// Arm only when the scenario has one outstanding Respond command. Observing
// its real settlement avoids substituting a guessed sleep for sender cleanup.
#[derive(Clone)]
pub(super) struct RespondProbe(Rc<Inner>);

impl RespondProbe {
    pub(super) fn new() -> Self {
        Self(Rc::new(Inner {
            state: RefCell::new(State::Idle),
            changed: Notify::new(),
        }))
    }

    pub(super) fn arm(&self) {
        let mut state = self.0.state.borrow_mut();
        assert!(
            matches!(*state, State::Idle),
            "one outstanding Respond watch"
        );
        *state = State::Armed;
    }

    pub(super) fn is_idle(&self) -> bool {
        matches!(*self.0.state.borrow(), State::Idle)
    }

    pub(super) fn observe(&self, event: &PrnsEvent<'_>) {
        let PrnsEvent::Diagnostic(Diagnostic::CommandSettled {
            id,
            settlement: Settlement::Respond(result),
        }) = event
        else {
            return;
        };
        let mut state = self.0.state.borrow_mut();
        match *state {
            State::Idle => return,
            State::Armed => {}
            State::Observed(_) => unreachable!("Respond observation must not be overwritten"),
        }
        *state = State::Observed(Responded {
            command: *id,
            result: *result,
            at: tokio::time::Instant::now(),
        });
        self.0.changed.notify_one();
    }

    pub(super) async fn take(&self) -> Responded {
        loop {
            let changed = self.0.changed.notified();
            {
                let mut state = self.0.state.borrow_mut();
                match *state {
                    State::Idle => unreachable!("arm before observing Respond settlement"),
                    State::Armed => {}
                    State::Observed(_) => {
                        let State::Observed(observed) = std::mem::replace(&mut *state, State::Idle)
                        else {
                            unreachable!()
                        };
                        return observed;
                    }
                }
            }
            changed.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::engine::{SendRequestFailure, SendResourceFailure};
    use std::future::{poll_fn, Future};
    use std::task::Poll;
    use std::time::Duration;

    fn event(settlement: Settlement) -> PrnsEvent<'static> {
        PrnsEvent::Diagnostic(Diagnostic::CommandSettled {
            id: CommandId(7),
            settlement,
        })
    }

    #[tokio::test(start_paused = true)]
    async fn settlement_before_or_after_wait_is_exact_and_reusable() {
        let probe = RespondProbe::new();
        let started = tokio::time::Instant::now();
        probe.observe(&event(Settlement::Respond(Ok(()))));
        assert!(probe.is_idle());
        for result in [
            Ok(()),
            Err(RespondFailure::Resource(SendResourceFailure::Timeout)),
        ] {
            probe.arm();
            probe.observe(&event(Settlement::SendRequest(Err(
                SendRequestFailure::Timeout,
            ))));
            let (observed, ()) = tokio::join!(biased; probe.take(), async {
                probe.observe(&event(Settlement::Respond(result)));
            });
            assert_eq!(
                observed,
                Responded {
                    command: CommandId(7),
                    result,
                    at: started
                }
            );
            assert!(probe.is_idle());
            probe.arm();
            probe.observe(&event(Settlement::Respond(result)));
            assert_eq!(
                probe.take().await,
                Responded {
                    command: CommandId(7),
                    result,
                    at: started
                }
            );
        }
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_wait_keeps_its_observation() {
        let probe = RespondProbe::new();
        probe.arm();
        let mut wait = Box::pin(probe.take());
        poll_fn(|cx| {
            assert!(wait.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(wait);
        let at = tokio::time::Instant::now();
        probe.observe(&event(Settlement::Respond(Ok(()))));
        assert_eq!(
            probe.take().await,
            Responded {
                command: CommandId(7),
                result: Ok(()),
                at
            }
        );
        assert!(probe.is_idle());
    }

    #[test]
    #[should_panic(expected = "one outstanding Respond watch")]
    fn rearming_cannot_discard_a_watch() {
        let probe = RespondProbe::new();
        probe.arm();
        probe.arm();
    }

    #[tokio::test(start_paused = true)]
    #[should_panic(expected = "Respond observation must not be overwritten")]
    async fn duplicate_settlement_cannot_replace_unconsumed_evidence() {
        let probe = RespondProbe::new();
        probe.arm();
        probe.observe(&event(Settlement::Respond(Ok(()))));
        probe.observe(&event(Settlement::Respond(Ok(()))));
    }
}
