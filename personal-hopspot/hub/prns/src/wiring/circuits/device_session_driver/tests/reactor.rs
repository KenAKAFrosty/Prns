use super::*;
use alloc::{collections::VecDeque, task::Wake};
use core::sync::atomic::Ordering;
use std::io::ErrorKind;

struct Script {
    actions: VecDeque<Box<dyn FnOnce() -> std::io::Result<()>>>,
}

impl Reactor for Script {
    type Reaction = ();
    type Failure = std::io::Error;
    fn wait_for_reaction(&mut self) -> Result<(), Self::Failure> {
        self.actions
            .pop_front()
            .expect("reactor exceeded scripted polls")()
    }
}

#[test]
fn readiness_is_retained_and_interrupted_or_spurious_polls_retry_without_losing_failures() {
    let (driver, handle, connection, _work) = driver_fixture();
    let sender = handle.clone();
    let actions: VecDeque<Box<dyn FnOnce() -> std::io::Result<()>>> = alloc::vec![
        Box::new(|| Err(ErrorKind::Interrupted.into())) as Box<dyn FnOnce() -> std::io::Result<()>>,
        Box::new(|| Ok(())),
        Box::new(|| Err(std::io::Error::from_raw_os_error(5))),
        Box::new(move || {
            assert_eq!(
                sender.submit(DeviceSessionIntent::Inspect).unwrap(),
                DeviceSessionSubmission::Submitted
            );
            Ok(())
        }),
    ]
    .into();
    let mut reactor = with_script(driver.reactor, Script { actions });
    assert_eq!(
        reactor.wait_for_reaction().unwrap_err().raw_os_error(),
        Some(5)
    );
    for _ in 0..2 {
        assert_eq!(
            reactor.wait_for_reaction().unwrap(),
            DeviceDriverReaction::IntentReady
        );
    }
    assert_eq!(reactor.pending.take(), Some(DeviceSessionIntent::Inspect));
    reactor.active = Some(super::super::reactor::ActiveWork {
        input: PrnsDeviceIn::Close { connection },
        completion: Box::pin(async move { Ok(PrnsDeviceOut::StaleClose { connection }) }),
    });
    for _ in 0..2 {
        assert_eq!(
            reactor.wait_for_reaction().unwrap(),
            DeviceDriverReaction::WorkReady
        );
    }
    assert_eq!(
        reactor.completed.take().unwrap().unwrap(),
        PrnsDeviceOut::StaleClose { connection }
    );
    reactor.signal.clone().wake();
    reactor.signal.wake_by_ref();
    handle.shutdown().unwrap();
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        DeviceDriverReaction::Shutdown
    );
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        DeviceDriverReaction::Drained
    );
    assert!(reactor.signal.stopping.load(Ordering::Acquire));
}

#[test]
fn ready_work_is_not_starved_by_intents_and_shutdown_preempts_both() {
    let (driver, handle, connection, _work) = driver_fixture();
    let mut reactor = with_script(
        driver.reactor,
        Script {
            actions: VecDeque::new(),
        },
    );
    assert_eq!(
        handle.submit(DeviceSessionIntent::Inspect).unwrap(),
        DeviceSessionSubmission::Submitted
    );
    reactor.active = Some(super::super::reactor::ActiveWork {
        input: PrnsDeviceIn::Close { connection },
        completion: Box::pin(async move { Ok(PrnsDeviceOut::StaleClose { connection }) }),
    });
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        DeviceDriverReaction::WorkReady
    );
    drop(reactor.completed.take());
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        DeviceDriverReaction::IntentReady
    );
    handle.shutdown().unwrap();
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        DeviceDriverReaction::Shutdown
    );
    assert_eq!(reactor.pending.take(), Some(DeviceSessionIntent::Inspect));
}

fn with_script(reactor: DeviceDriverReactor, physical: Script) -> DeviceDriverReactor<Script> {
    DeviceDriverReactor {
        physical,
        inbox: reactor.inbox,
        signal: reactor.signal,
        pending: reactor.pending,
        active: reactor.active,
        completed: reactor.completed,
        stopping: reactor.stopping,
        admit_intents: reactor.admit_intents,
    }
}

#[test]
fn command_backlog_defers_intents_and_last_sender_closure_still_drains_pending_work() {
    let (driver, handle, connection, _work) = driver_fixture();
    let mut reactor = with_script(
        driver.reactor,
        Script {
            actions: alloc::vec![Box::new(|| Err(ErrorKind::WouldBlock.into()))
                as Box<dyn FnOnce() -> std::io::Result<()>>]
            .into(),
        },
    );
    reactor.admit_intents = false;
    reactor.active = Some(super::super::reactor::ActiveWork {
        input: PrnsDeviceIn::Close { connection },
        completion: Box::pin(core::future::pending()),
    });
    assert_eq!(
        handle.submit(DeviceSessionIntent::Inspect).unwrap(),
        DeviceSessionSubmission::Submitted
    );
    assert_eq!(
        reactor.wait_for_reaction().unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    assert!(reactor.pending.is_none());
    assert!(reactor.active.is_some());
    reactor.admit_intents = true;
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        DeviceDriverReaction::IntentReady
    );
    assert_eq!(reactor.pending.take(), Some(DeviceSessionIntent::Inspect));
    drop(handle);
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        DeviceDriverReaction::Shutdown
    );
    assert!(reactor.active.is_some());
    reactor.active = None;
    assert_eq!(
        reactor.wait_for_reaction().unwrap(),
        DeviceDriverReaction::Drained
    );
}

#[test]
fn idle_mio_poll_is_woken_by_last_handle_drop_stop_guard_and_owned_future_waker() {
    for mode in 0..3 {
        let (driver, handle, _connection, _work) = driver_fixture();
        let mut physical = driver.reactor.physical;
        let signal = Arc::clone(&driver.reactor.signal);
        let rescue = Arc::clone(&signal.wake);
        let (entered, entry) = sync_channel(1);
        let (done, finished) = sync_channel(1);
        let mut reactor = DeviceDriverReactor {
            physical: Script {
                actions: alloc::vec![Box::new(move || {
                    entered.send(()).unwrap();
                    physical.wait_for_reaction()
                })
                    as Box<dyn FnOnce() -> std::io::Result<()>>]
                .into(),
            },
            inbox: driver.reactor.inbox,
            signal: driver.reactor.signal,
            pending: None,
            active: None,
            completed: None,
            stopping: false,
            admit_intents: true,
        };
        std::thread::scope(|scope| {
            let producer = scope.spawn(move || {
                entry.recv_timeout(Duration::from_secs(2)).unwrap();
                match mode {
                    0 => drop(handle),
                    1 => drop(super::super::handle::StopOnDrop(signal)),
                    _ => {
                        signal.stopping.store(true, Ordering::Release);
                        signal.wake();
                    }
                }
                let rescued = finished.recv_timeout(Duration::from_secs(2)).is_err();
                if rescued {
                    rescue.wake().unwrap();
                }
                assert!(!rescued, "idle conductor was not woken");
            });
            assert_eq!(
                reactor.wait_for_reaction().unwrap(),
                DeviceDriverReaction::Shutdown
            );
            done.send(()).unwrap();
            producer.join().unwrap();
        });
    }
}
