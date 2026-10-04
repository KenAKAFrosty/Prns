#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use core::time::Duration;
use proptest::prelude::*;

struct Action<F>(F);

impl<F: FnMut() -> std::io::Result<()>> Reactor for Action<F> {
    type Reaction = ();
    type Failure = std::io::Error;

    fn wait_for_reaction(&mut self) -> Result<(), Self::Failure> {
        (self.0)()
    }
}

fn submitted(sender: &MioSessionSender, message: DeviceSessionMessage) {
    assert!(matches!(
        sender.submit(message).unwrap(),
        MioSessionSubmission::Submitted
    ));
}

fn message(tag: bool) -> DeviceSessionMessage {
    if tag {
        DeviceSessionMessage::Inspect
    } else {
        DeviceSessionMessage::Disconnect
    }
}

fn tag(message: DeviceSessionMessage) -> bool {
    match message {
        DeviceSessionMessage::Inspect => true,
        DeviceSessionMessage::Disconnect => false,
        DeviceSessionMessage::Connect
        | DeviceSessionMessage::Refresh
        | DeviceSessionMessage::Control(_)
        | DeviceSessionMessage::Prns(_) => panic!("unexpected message"),
    }
}

#[test]
fn accepted_messages_drain_before_closed_and_rejections_preserve_the_input() {
    let (mut reactor, sender) = MioSessionReactor::try_open().unwrap();
    let clone = sender.clone();
    submitted(&sender, message(true));
    let MioSessionSubmission::Busy { message: rejected } = clone.submit(message(false)).unwrap()
    else {
        panic!("full mailbox accepted a second message")
    };
    assert!(!tag(rejected));
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        MioSessionReaction::MessageReady
    );
    submitted(&clone, message(false));
    drop(sender);
    drop(clone);
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        MioSessionReaction::MessageReady
    );
    assert!(tag(reactor.take_message().unwrap()));
    assert!(reactor.take_message().is_none());
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        MioSessionReaction::MessageReady
    );
    assert!(!tag(reactor.take_message().unwrap()));
    for _ in 0..2 {
        assert_eq!(
            reactor.wait_for_reaction().unwrap(),
            MioSessionReaction::SendersClosed
        );
    }
    let (reactor, sender) = MioSessionReactor::try_open().unwrap();
    drop(reactor);
    let MioSessionSubmission::Closed { message: rejected } = sender.submit(message(true)).unwrap()
    else {
        panic!("closed mailbox accepted a message")
    };
    assert!(tag(rejected));
}

#[test]
fn interrupted_and_spurious_polls_retry_but_other_errors_preserve_their_source() {
    let (original, sender) = MioSessionReactor::try_open().unwrap();
    let mut turns = 0;
    let mut sender = Some(sender);
    let mut reactor = MioSessionReactor {
        physical: Action(|| {
            turns += 1;
            match turns {
                1 => Err(ErrorKind::Interrupted.into()),
                2 => Ok(()),
                3 => Err(std::io::Error::from_raw_os_error(5)),
                4 => {
                    submitted(sender.as_ref().unwrap(), message(true));
                    Ok(())
                }
                _ => {
                    drop(sender.take());
                    Ok(())
                }
            }
        }),
        inbox: original.inbox,
        pending: None,
    };
    assert_eq!(
        reactor.wait_for_reaction().unwrap_err().raw_os_error(),
        Some(5)
    );
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        MioSessionReaction::MessageReady
    );
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        MioSessionReaction::MessageReady
    );
    assert!(tag(reactor.take_message().unwrap()));
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        MioSessionReaction::SendersClosed
    );
}

#[test]
fn real_poll_observes_wakes_and_last_sender_drop_after_the_empty_check() {
    for close in [false, true] {
        let poll = Poll::new().unwrap();
        let waker = Arc::new(Waker::new(poll.registry(), Token(0)).unwrap());
        let (outbox, inbox) = sync_channel(1);
        let sender = MioSessionSender::new(outbox, Arc::clone(&waker));
        let (ready, entered) = sync_channel(1);
        let (finished, finish) = sync_channel(1);
        let mut physical = MioSessionPoll {
            poll,
            events: Events::with_capacity(1),
        };
        let mut reactor = MioSessionReactor {
            physical: Action(|| {
                ready.send(()).unwrap();
                physical.wait_for_reaction()?;
                assert!(
                    physical
                        .events
                        .iter()
                        .any(|event| event.token() == Token(0))
                );
                Ok(())
            }),
            inbox,
            pending: None,
        };
        std::thread::scope(|scope| {
            let producer = scope.spawn(move || {
                entered.recv_timeout(Duration::from_secs(2)).unwrap();
                if close {
                    drop(sender);
                } else {
                    submitted(&sender, message(true));
                }
                let rescued = finish.recv_timeout(Duration::from_secs(2)).is_err();
                if rescued {
                    waker.wake().unwrap();
                }
                assert!(!rescued, "mailbox failed to wake Mio");
            });
            assert_eq!(
                reactor.wait_for_reaction().unwrap(),
                if close {
                    MioSessionReaction::SendersClosed
                } else {
                    MioSessionReaction::MessageReady
                }
            );
            finished.send(()).unwrap();
            producer.join().unwrap();
        });
    }
}

proptest! {
    #[test]
    fn arbitrary_mailbox_turns_match_a_bounded_fifo(actions in prop::collection::vec((0u8..3, any::<bool>()), 0..100)) {
        let (original, sender) = MioSessionReactor::try_open().unwrap();
        let mut polls = 0usize;
        let poll_limit = actions.len();
        let mut reactor = MioSessionReactor {
            physical: Action(|| {
                polls = polls.checked_add(1).unwrap();
                assert!(polls <= poll_limit, "empty mailbox exceeded one poll per action");
                Err(ErrorKind::WouldBlock.into())
            }),
            inbox: original.inbox,
            pending: None,
        };
        let mut queued = None;
        let mut pending = None;
        for (action, value) in actions {
            match action {
                0 => match sender.submit(message(value)).unwrap() {
                    MioSessionSubmission::Submitted => { prop_assert!(queued.is_none()); queued = Some(value); }
                    MioSessionSubmission::Busy { message } => { prop_assert!(queued.is_some()); prop_assert_eq!(tag(message), value); }
                    MioSessionSubmission::Closed { .. } => prop_assert!(false),
                },
                1 => {
                    if pending.is_none() { pending = queued.take(); }
                    match pending {
                        Some(_) => prop_assert_eq!(reactor.wait_for_reaction().unwrap(), MioSessionReaction::MessageReady),
                        None => prop_assert_eq!(reactor.wait_for_reaction().unwrap_err().kind(), ErrorKind::WouldBlock),
                    }
                }
                _ => prop_assert_eq!(reactor.take_message().map(tag), pending.take()),
            }
        }
    }
}
