#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::{
    convert::Infallible,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

type Task = Box<dyn FnOnce() + Send>;
fn receive<T>(receiver: &mpsc::Receiver<T>) -> T {
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("worker made progress")
}
fn worker(capacity: usize) -> Worker<Task, Infallible> {
    Worker::start(
        "storage-test",
        NonZeroUsize::new(capacity).unwrap(),
        |inbox: Inbox<Task>| {
            inbox.run(|task| task());
            Ok(())
        },
    )
    .unwrap()
}
fn submit(client: &Client<Task>, task: impl FnOnce() + Send + 'static) {
    assert!(client.try_submit(Box::new(task)).is_ok());
}
fn block(client: &Client<Task>) -> mpsc::Sender<()> {
    let (entered, entered_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    submit(client, move || {
        entered.send(()).unwrap();
        receive(&release_rx);
    });
    receive(&entered_rx);
    release
}

#[test]
fn close_fences_a_full_queue_and_drains_every_accepted_job() {
    let mut owner = worker(2);
    let client = owner.client();
    let release = block(&client);
    let (result, results) = mpsc::channel();
    for value in 0..2 {
        let result = result.clone();
        submit(&client, move || {
            result.send(value).unwrap();
        });
    }
    assert!(matches!(
        client.try_submit(Box::new(|| {})),
        Err(SubmitError::Full(_))
    ));
    owner.request_close();
    assert!(matches!(
        client.try_submit(Box::new(|| {})),
        Err(SubmitError::Closed(_))
    ));
    let closed = thread::spawn(move || {
        owner.close().unwrap();
        owner.close().unwrap();
    });
    release.send(()).unwrap();
    assert_eq!(receive(&results), 0);
    assert_eq!(receive(&results), 1);
    closed.join().unwrap();
}

#[test]
fn dropping_client_or_result_waiter_does_not_cancel_accepted_work() {
    let mut owner = worker(1);
    let client = owner.client();
    let release = block(&client);
    let (reply, waiter) = mpsc::channel::<()>();
    let committed = Arc::new(AtomicBool::new(false));
    let result = Arc::clone(&committed);
    submit(&client, move || {
        result.store(true, Ordering::SeqCst);
        let _ = reply.send(());
    });
    drop(waiter);
    drop(client);
    release.send(()).unwrap();
    owner.close().unwrap();
    assert!(committed.load(Ordering::SeqCst));
}

#[test]
fn clients_do_not_control_owner_lifetime() {
    let mut owner = worker(1);
    drop(owner.client());
    let (reply, waiter) = mpsc::channel();
    submit(&owner.client(), move || {
        reply.send(7).unwrap();
    });
    assert_eq!(receive(&waiter), 7);
    owner.close().unwrap();
}

#[test]
fn owner_drop_fences_without_waiting_and_worker_retains_accepted_jobs() {
    let owner = worker(1);
    let client = owner.client();
    let release = block(&client);
    let (reply, waiter) = mpsc::channel();
    submit(&client, move || {
        reply.send(()).unwrap();
    });
    drop(owner);
    assert!(matches!(
        client.try_submit(Box::new(|| {})),
        Err(SubmitError::Closed(_))
    ));
    release.send(()).unwrap();
    receive(&waiter);
}

#[test]
fn scope_drain_does_not_close_other_scopes_or_starve_accepted_work() {
    let mut owner = worker(3);
    let mut first = owner.scope();
    let second = owner.scope();
    let first_client = first.client();
    let second_client = second.client();
    let release = block(&first_client);
    let (reply, waiter) = mpsc::channel();
    submit(&second_client, move || {
        reply.send(1).unwrap();
    });
    submit(&first_client, || {});
    submit(&first_client, || {});
    assert!(matches!(
        first_client.try_submit(Box::new(|| {})),
        Err(SubmitError::Full(_))
    ));
    first.request_close();
    assert!(matches!(
        first_client.try_submit(Box::new(|| {})),
        Err(SubmitError::Closed(_))
    ));
    let drained = thread::spawn(move || first.close());
    release.send(()).unwrap();
    assert_eq!(receive(&waiter), 1);
    drained.join().unwrap().unwrap();
    let (reply, waiter) = mpsc::channel();
    submit(&second_client, move || {
        reply.send(2).unwrap();
    });
    assert_eq!(receive(&waiter), 2);
    owner.close().unwrap();
}

#[test]
fn dropped_scope_fences_clients_but_preserves_pending_work() {
    let mut owner = worker(1);
    let scope = owner.scope();
    let client = scope.client();
    let release = block(&client);
    let committed = Arc::new(AtomicBool::new(false));
    let result = Arc::clone(&committed);
    submit(&client, move || result.store(true, Ordering::SeqCst));
    drop(scope);
    assert!(matches!(
        client.try_submit(Box::new(|| {})),
        Err(SubmitError::Closed(_))
    ));
    release.send(()).unwrap();
    owner.close().unwrap();
    assert!(committed.load(Ordering::SeqCst));
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn initializer_failure_is_joined_after_resource_cleanup() {
    let dropped = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&dropped);
    let result = Worker::<(), _>::start("init-error", NonZeroUsize::MIN, move |_inbox| {
        let _resource = Dropped(observed);
        Err("cannot open")
    });
    assert!(matches!(
        result,
        Err(StartError::Initialization("cannot open"))
    ));
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn initializer_panic_and_missing_readiness_are_distinct() {
    let result = Worker::<(), Infallible>::start("init-panic", NonZeroUsize::MIN, |_inbox| {
        panic!("initialization failed")
    });
    assert!(matches!(result, Err(StartError::Panicked)));
    let result = Worker::<(), Infallible>::start("not-ready", NonZeroUsize::MIN, |_inbox| Ok(()));
    assert!(matches!(result, Err(StartError::NotReady)));
}

#[test]
fn handler_panic_fails_pending_scopes_and_close_stays_failed() {
    let mut owner = worker(2);
    let mut scope = owner.scope();
    let client = scope.client();
    let release = block(&client);
    submit(&client, || panic!("handler failed"));
    let calls = Arc::new(AtomicUsize::new(0));
    let result = Arc::clone(&calls);
    submit(&client, move || {
        result.fetch_add(1, Ordering::SeqCst);
    });
    release.send(()).unwrap();
    assert!(matches!(owner.close(), Err(CloseError::Panicked)));
    assert_eq!(scope.close(), Err(DrainError::WorkerFailed));
    assert!(matches!(owner.close(), Err(CloseError::Panicked)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        client.try_submit(Box::new(|| {})),
        Err(SubmitError::Closed(_))
    ));
}

#[test]
fn finalization_error_is_not_reported_as_success() {
    let mut owner = Worker::<(), _>::start("final-error", NonZeroUsize::MIN, |inbox| {
        inbox.run(|_| {});
        Err("close failed")
    })
    .unwrap();
    assert!(matches!(
        owner.close(),
        Err(CloseError::Failed(error)) if *error == "close failed"
    ));
    assert!(matches!(owner.close(), Err(CloseError::Failed(error)) if *error == "close failed"));
}

#[test]
fn worker_rejects_recursive_submission_and_scoped_self_drain() {
    let mut owner = worker(1);
    let client = owner.client();
    let recursive = client.clone();
    let mut scope = owner.scope();
    let (reply, waiter) = mpsc::channel();
    submit(&client, move || {
        let rejected = matches!(
            recursive.try_submit(Box::new(|| {})),
            Err(SubmitError::Reentrant(_))
        );
        reply.send((rejected, scope.close())).unwrap();
    });
    assert_eq!(receive(&waiter), (true, Err(DrainError::Reentrant)));
    owner.close().unwrap();
}

#[test]
fn lost_startup_receiver_does_not_leave_an_idle_worker_running() {
    let shared = Arc::new(Shared {
        close: CloseSignal(Arc::new(AtomicBool::new(false))),
        queue: Mutex::new(Queue {
            jobs: VecDeque::<Queued<()>>::new(),
            control: None,
            closed: false,
            progress: false,
            thread: None,
        }),
        available: Condvar::new(),
        capacity: NonZeroUsize::MIN,
    });
    let (ready, receiver) = mpsc::sync_channel(1);
    drop(receiver);
    Inbox {
        shared: Arc::clone(&shared),
        ready,
    }
    .run(|_| panic!("no accepted jobs"));
    assert!(lock(&shared.queue).closed);
}

struct ThreadWake(thread::Thread);
impl std::task::Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
fn await_completion<E>(completion: Completion<E>) -> Result<(), CloseError<E>> {
    use std::{future::Future, task::Context, task::Poll, time::Instant};
    let waker = std::task::Waker::from(Arc::new(ThreadWake(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut completion = std::pin::pin!(completion);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Poll::Ready(result) = completion.as_mut().poll(&mut context) {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "cleanup completed before timeout"
        );
        thread::park_timeout(Duration::from_millis(20));
    }
}

#[test]
fn abandoned_close_waiters_and_owner_do_not_cancel_inspectable_cleanup() {
    use std::{future::Future, task::Context, task::Poll};
    let dropped = Arc::new(AtomicBool::new(false));
    let resource = Arc::clone(&dropped);
    let owner =
        Worker::<Task, Infallible>::start("retained-close", NonZeroUsize::MIN, move |inbox| {
            let _resource = Dropped(resource);
            inbox.run(|task| task());
            Ok(())
        })
        .unwrap();
    let release = block(&owner.client());
    let completion = owner.closed();
    let other = completion.clone();
    let mut abandoned = completion.clone();
    let waker = std::task::Waker::from(Arc::new(ThreadWake(thread::current())));
    assert!(matches!(
        std::pin::Pin::new(&mut abandoned).poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));
    owner.request_close();
    drop(abandoned);
    drop(owner);
    assert!(completion.status().is_none());
    assert!(!dropped.load(Ordering::SeqCst));
    let observed = thread::spawn(move || await_completion(other));
    release.send(()).unwrap();
    await_completion(completion.clone()).unwrap();
    observed.join().unwrap().unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert!(matches!(completion.status(), Some(Ok(()))));
}

#[test]
fn cleanup_failure_is_retained_across_waiters_and_synchronous_close() {
    let mut owner = Worker::<(), _>::start("retained-error", NonZeroUsize::MIN, |inbox| {
        inbox.run(|_| {});
        Err("final checkpoint failed")
    })
    .unwrap();
    let first = owner.closed();
    let second = first.clone();
    owner.request_close();
    let Err(CloseError::Failed(first_error)) = await_completion(first) else {
        panic!("terminal error required");
    };
    let Err(CloseError::Failed(second_error)) = await_completion(second) else {
        panic!("same terminal error required");
    };
    let Err(CloseError::Failed(joined_error)) = owner.close() else {
        panic!("synchronous join retains the error");
    };
    assert_eq!(*first_error, "final checkpoint failed");
    assert!(Arc::ptr_eq(&first_error, &second_error));
    assert!(Arc::ptr_eq(&first_error, &joined_error));
}

#[test]
fn cleanup_observation_rejects_self_wait_without_closing_the_worker() {
    let mut owner = worker(1);
    let completion = owner.closed();
    let (reply, waiter) = mpsc::channel();
    submit(&owner.client(), move || {
        reply
            .send(matches!(
                await_completion(completion),
                Err(CloseError::Reentrant)
            ))
            .unwrap();
    });
    assert!(receive(&waiter));
    assert!(owner.closed().status().is_none());
    owner.close().unwrap();
}

#[test]
fn panic_completion_waits_for_resource_drop_and_remains_inspectable() {
    let dropped = Arc::new(AtomicBool::new(false));
    let resource = Arc::clone(&dropped);
    let owner =
        Worker::<Task, Infallible>::start("retained-panic", NonZeroUsize::MIN, move |inbox| {
            let _resource = Dropped(resource);
            inbox.run(|task| task());
            Ok(())
        })
        .unwrap();
    let completion = owner.closed();
    submit(&owner.client(), || panic!("domain handler panic"));
    drop(owner);
    assert!(matches!(
        await_completion(completion.clone()),
        Err(CloseError::Panicked)
    ));
    assert!(dropped.load(Ordering::SeqCst));
    assert!(matches!(
        completion.status(),
        Some(Err(CloseError::Panicked))
    ));
}

#[test]
fn managed_close_bypasses_full_queue_and_waits_for_retained_effects() {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    struct Effect {
        ready: Arc<AtomicBool>,
        wake: Arc<Mutex<Option<Waker>>>,
    }
    impl Future for Effect {
        type Output = ();
        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
            if self.ready.load(Ordering::SeqCst) {
                return Poll::Ready(());
            }
            *lock(&self.wake) = Some(context.waker().clone());
            if self.ready.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }
    let ready = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(Mutex::new(None));
    let effect = Effect {
        ready: ready.clone(),
        wake: wake.clone(),
    };
    let closing = Arc::new(AtomicBool::new(false));
    let observed_close = closing.clone();
    let dropped = Arc::new(AtomicBool::new(false));
    let resource = dropped.clone();
    let owner =
        Worker::<Task, Infallible>::start("owned-effects", NonZeroUsize::MIN, move |inbox| {
            let _resource = Dropped(resource);
            let mut effect = std::pin::pin!(effect);
            inbox.run_owned(|event, context| {
                match event {
                    OwnerEvent::Job(job) => job(),
                    OwnerEvent::Closing => {
                        observed_close.store(true, Ordering::SeqCst);
                    }
                    OwnerEvent::Progress => {}
                }
                if effect.as_mut().poll(context).is_ready() {
                    OwnerProgress::Drained
                } else {
                    OwnerProgress::Pending
                }
            });
            Ok(())
        })
        .unwrap();
    let client = owner.client();
    let release = block(&client);
    let (sent, received) = mpsc::channel();
    submit(&client, move || {
        sent.send(closing.load(Ordering::SeqCst)).unwrap();
    });
    assert!(matches!(
        client.try_submit(Box::new(|| {})),
        Err(SubmitError::Full(_))
    ));
    let completion = owner.closed();
    owner.request_close();
    drop(owner);
    assert!(completion.status().is_none());
    release.send(()).unwrap();
    assert!(
        receive(&received),
        "close signal precedes accepted queued work"
    );
    assert!(
        completion.status().is_none(),
        "drained queue is not drained effects"
    );
    assert!(!dropped.load(Ordering::SeqCst));
    ready.store(true, Ordering::SeqCst);
    let waker = lock(&wake).take().unwrap();
    waker.wake_by_ref();
    await_completion(completion).unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    // A retained native effect may still drop/wake its old notification handle.
    waker.wake();
}

#[test]
fn self_waking_effects_cannot_starve_accepted_jobs_or_close() {
    let (entered, entered_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (counts, observed) = mpsc::channel();
    let owner = Worker::<(), Infallible>::start(
        "fair-owner-progress",
        NonZeroUsize::new(2).unwrap(),
        move |inbox| {
            let mut polls = 0;
            let mut first = true;
            inbox.run_owned(|event, context| match event {
                OwnerEvent::Progress => {
                    polls += 1;
                    if first {
                        first = false;
                        entered.send(()).unwrap();
                        receive(&release_rx);
                    }
                    context.waker().wake_by_ref();
                    OwnerProgress::Pending
                }
                OwnerEvent::Job(()) => {
                    counts.send(polls).unwrap();
                    OwnerProgress::Pending
                }
                OwnerEvent::Closing => OwnerProgress::Drained,
            });
            Ok(())
        },
    )
    .unwrap();
    receive(&entered_rx);
    assert!(owner.client().try_submit(()).is_ok());
    assert!(owner.client().try_submit(()).is_ok());
    release.send(()).unwrap();
    assert_eq!(receive(&observed), 1);
    assert_eq!(receive(&observed), 2);
    let completion = owner.closed();
    drop(owner);
    await_completion(completion).unwrap();
}

#[test]
fn progress_hints_coalesce_without_taking_ordinary_queue_capacity() {
    let (initial, initial_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (polled, observed) = mpsc::channel();
    let mut owner = Worker::<(), Infallible>::start(
        "coalesced-owner-progress",
        NonZeroUsize::MIN,
        move |inbox| {
            let mut polls = 0;
            inbox.run_owned(|event, context| {
                match event {
                    OwnerEvent::Progress => {
                        polls += 1;
                        if polls == 1 {
                            initial.send(context.waker().clone()).unwrap();
                            receive(&release_rx);
                        }
                    }
                    OwnerEvent::Job(()) => {
                        polled.send(polls).unwrap();
                    }
                    OwnerEvent::Closing => {}
                }
                OwnerProgress::Drained
            });
            Ok(())
        },
    )
    .unwrap();
    let waker = receive(&initial_rx);
    assert!(owner.client().try_submit(()).is_ok());
    for _ in 0..1000 {
        waker.wake_by_ref();
    }
    assert!(matches!(
        owner.client().try_submit(()),
        Err(SubmitError::Full(()))
    ));
    release.send(()).unwrap();
    // Fairness delivers the queued job before the one coalesced progress event.
    assert_eq!(receive(&observed), 1);
    assert!(owner.client().try_submit(()).is_ok());
    assert_eq!(receive(&observed), 2);
    owner.close().unwrap();
}

#[test]
fn reserved_control_bypasses_full_backlog_and_shares_scope_drain() {
    let mut owner = worker(1);
    let mut scope = owner.scope();
    let control = scope.control_client();
    let ordinary = owner.client();
    let release = block(&ordinary);
    let (events, observed) = mpsc::channel();
    let ordinary_event = events.clone();
    submit(&ordinary, move || ordinary_event.send("ordinary").unwrap());
    assert!(matches!(
        ordinary.try_submit(Box::new(|| {})),
        Err(SubmitError::Full(_))
    ));
    let (continue_control, control_release) = mpsc::channel();
    assert!(control
        .try_submit(Box::new(move || {
            events.send("control").unwrap();
            receive(&control_release);
        }))
        .is_ok());
    // The slot is global, including independently scoped capabilities.
    assert!(matches!(
        owner.control_client().try_submit(Box::new(|| {})),
        Err(SubmitError::Full(_))
    ));
    scope.request_close();
    assert!(matches!(
        control.try_submit(Box::new(|| {})),
        Err(SubmitError::Closed(_))
    ));
    let (drained, drainage) = mpsc::channel();
    let drain_thread = thread::spawn(move || drained.send(scope.close()).unwrap());
    release.send(()).unwrap();
    assert_eq!(receive(&observed), "control");
    assert!(matches!(
        drainage.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    continue_control.send(()).unwrap();
    receive(&drainage).unwrap();
    assert_eq!(receive(&observed), "ordinary");
    drain_thread.join().unwrap();
    owner.close().unwrap();
    assert!(matches!(
        owner.control_client().try_submit(Box::new(|| {})),
        Err(SubmitError::Closed(_))
    ));
}

#[test]
fn continuous_controls_preserve_ordinary_and_progress_fairness() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Event {
        Control(u8),
        Ordinary(u8),
        Progress,
        Closing,
    }
    let (initial, initial_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (control_entered, controls) = mpsc::channel();
    let (continue_control, control_release) = mpsc::channel();
    let history = Arc::new(Mutex::new(Vec::new()));
    let history_write = history.clone();
    let mut owner = Worker::<Event, Infallible>::start(
        "fair-reserved-controls",
        NonZeroUsize::new(2).unwrap(),
        move |inbox| {
            let mut first = true;
            inbox.run_owned(|event, context| {
                let event = match event {
                    OwnerEvent::Job(event) => event,
                    OwnerEvent::Progress => Event::Progress,
                    OwnerEvent::Closing => Event::Closing,
                };
                lock(&history_write).push(event);
                match event {
                    Event::Progress => {
                        context.waker().wake_by_ref();
                        if first {
                            first = false;
                            initial.send(()).unwrap();
                            receive(&release_rx);
                        }
                    }
                    Event::Control(index) => {
                        control_entered.send(index).unwrap();
                        receive(&control_release);
                    }
                    _ => {}
                }
                OwnerProgress::Drained
            });
            Ok(())
        },
    )
    .unwrap();
    receive(&initial_rx);
    let control = owner.control_client();
    owner.client().try_submit(Event::Ordinary(1)).unwrap();
    owner.client().try_submit(Event::Ordinary(2)).unwrap();
    control.try_submit(Event::Control(1)).unwrap();
    release.send(()).unwrap();
    for index in 1..=4 {
        assert_eq!(receive(&controls), index);
        if index < 4 {
            control.try_submit(Event::Control(index + 1)).unwrap();
        }
        continue_control.send(()).unwrap();
    }
    owner.close().unwrap();
    // Replenish the control slot before each control returns. Neither controls
    // nor self-waking progress can reset the other class's fairness bookkeeping.
    assert_eq!(
        &lock(&history)[..8],
        &[
            Event::Progress,
            Event::Control(1),
            Event::Ordinary(1),
            Event::Control(2),
            Event::Progress,
            Event::Control(3),
            Event::Ordinary(2),
            Event::Control(4),
        ]
    );
}

#[test]
fn panic_abandons_reserved_control_outside_locks_and_fails_scope() {
    struct ReenterOnDrop(Client<Task>, mpsc::Sender<bool>);
    impl Drop for ReenterOnDrop {
        fn drop(&mut self) {
            self.1
                .send(matches!(
                    self.0.try_submit(Box::new(|| {})),
                    Err(SubmitError::Reentrant(_))
                ))
                .unwrap();
        }
    }
    let mut owner = worker(1);
    let mut scope = owner.scope();
    let (entered, entered_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    submit(&owner.client(), move || {
        entered.send(()).unwrap();
        receive(&release_rx);
        panic!("executing handler failed");
    });
    receive(&entered_rx);
    let (dropped, drop_observed) = mpsc::channel();
    let probe = ReenterOnDrop(owner.client(), dropped);
    assert!(scope
        .control_client()
        .try_submit(Box::new(move || {
            drop(probe);
            panic!("abandoned control must never execute");
        }))
        .is_ok());
    release.send(()).unwrap();
    assert!(receive(&drop_observed));
    assert!(matches!(
        await_completion(owner.closed()),
        Err(CloseError::Panicked)
    ));
    assert_eq!(scope.close(), Err(DrainError::WorkerFailed));
    assert!(matches!(owner.close(), Err(CloseError::Panicked)));
}

#[test]
fn reserved_control_rejects_same_worker_submission() {
    let mut owner = worker(1);
    let control = owner.control_client();
    let (result, observed) = mpsc::channel();
    submit(&owner.client(), move || {
        result
            .send(matches!(
                control.try_submit(Box::new(|| {})),
                Err(SubmitError::Reentrant(_))
            ))
            .unwrap();
    });
    assert!(receive(&observed));
    owner.close().unwrap();
}

#[test]
fn close_signal_is_synchronous_through_drop_and_does_not_wait_for_handler() {
    let owner = worker(1);
    let signal = owner.close_signal();
    let release = block(&owner.client());
    assert!(!signal.is_closed());
    let completion = owner.closed();
    drop(owner);
    assert!(signal.is_closed());
    assert!(completion.status().is_none());
    release.send(()).unwrap();
}

#[test]
fn inbox_close_signal_survives_owner_and_observes_unexpected_worker_exit() {
    let (sent, received) = mpsc::channel();
    let mut owner = Worker::start(
        "close-signal",
        NonZeroUsize::new(1).unwrap(),
        move |inbox: Inbox<Task>| {
            sent.send(inbox.close_signal()).unwrap();
            inbox.run(|job| job());
            Ok::<(), Infallible>(())
        },
    )
    .unwrap();
    let signal = receive(&received);
    submit(&owner.client(), || panic!("injected worker failure"));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !signal.is_closed() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let _ = owner.close();
    assert!(signal.is_closed());
}

#[test]
fn initializer_client_uses_existing_queue_without_control_or_worker_ownership() {
    let (prepared, observe) = mpsc::channel();
    let mut owner = Worker::<Task, ()>::start(
        "prepared-client",
        NonZeroUsize::new(1).unwrap(),
        move |inbox| {
            let client = inbox.client();
            assert!(matches!(
                client.try_submit(Box::new(|| {})),
                Err(SubmitError::Reentrant(_))
            ));
            prepared.send(client).unwrap();
            inbox.run(|job| job());
            Ok(())
        },
    )
    .unwrap();
    let client = receive(&observe);
    let (finished, result) = mpsc::channel();
    submit(&client, move || {
        finished.send(7).unwrap();
    });
    assert_eq!(receive(&result), 7);
    owner.close().unwrap();
    assert!(matches!(
        client.try_submit(Box::new(|| {})),
        Err(SubmitError::Closed(_))
    ));
}
