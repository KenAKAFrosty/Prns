use core::cell::Cell;
use core::task::{Context, Waker};
use std::collections::VecDeque;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Closed;

struct Source(VecDeque<Result<usize, Closed>>);

impl BleSource for Source {
    type Error = Closed;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Closed> {
        out.fill(7);
        match self.0.pop_front() {
            Some(result) => result,
            None => core::future::pending().await,
        }
    }
}

struct Sink<'a> {
    ready: &'a Cell<bool>,
    starts: &'a Cell<usize>,
    result: Result<(), Closed>,
}

impl BleSink for Sink<'_> {
    type Error = Closed;

    async fn send_frame(&mut self, _: &[u8]) -> Result<(), Closed> {
        self.starts.set(self.starts.get() + 1);
        poll_fn(|_| match self.ready.get() {
            true => Poll::Ready(self.result),
            false => Poll::Pending,
        })
        .await
    }
}

#[test]
fn send_settles_during_forwarding_backpressure_without_losing_the_received_frame() {
    for result in [Ok(()), Err(Closed)] {
        let send_ready = Cell::new(false);
        let forward_ready = Cell::new(false);
        let starts = Cell::new(0);
        let mut source = Source(VecDeque::from([Ok(2)]));
        let mut sink = Sink {
            ready: &send_ready,
            starts: &starts,
            result,
        };
        let mut inbound = [0; 2];
        let mut forwarded = Vec::new();
        {
            let mut running = pin!(send_frame_duplex(
                &mut source,
                &mut sink,
                &[1],
                &mut inbound,
                Forwarder {
                    ready: &forward_ready,
                    frames: &mut forwarded
                },
            ));
            let mut context = Context::from_waker(Waker::noop());
            assert!(running.as_mut().poll(&mut context).is_pending());
            send_ready.set(true);
            assert!(running.as_mut().poll(&mut context).is_pending());
            send_ready.set(false);
            forward_ready.set(true);
            assert_eq!(
                running.as_mut().poll(&mut context),
                Poll::Ready(BleDuplexOutcome::Finished(result))
            );
        }
        assert_eq!((starts.get(), forwarded), (1, vec![vec![7, 7]]));
    }
}

#[test]
fn invalid_receives_abort_the_pending_send_without_forwarding_a_prefix() {
    for (received, expected) in [
        (Ok(3), BleDuplexOutcome::InvalidReceiveLength(3)),
        (
            Ok(usize::MAX),
            BleDuplexOutcome::InvalidReceiveLength(usize::MAX),
        ),
        (Err(Closed), BleDuplexOutcome::ReceiveFailed(Closed)),
    ] {
        let ready = Cell::new(false);
        let starts = Cell::new(0);
        let mut source = Source(VecDeque::from([received]));
        let mut sink = Sink {
            ready: &ready,
            starts: &starts,
            result: Ok(()),
        };
        let mut inbound = [0; 2];
        let mut frames = Vec::new();
        let mut running = pin!(send_frame_duplex(
            &mut source,
            &mut sink,
            &[1],
            &mut inbound,
            Forwarder {
                ready: &ready,
                frames: &mut frames
            },
        ));
        assert_eq!(
            running
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(expected)
        );
        assert_eq!(starts.get(), 1);
    }
}

struct Forwarder<'a> {
    ready: &'a Cell<bool>,
    frames: &'a mut Vec<Vec<u8>>,
}

struct RefusingForwarder<'a> {
    ready: &'a Cell<bool>,
}

impl BleFrameForwarder for RefusingForwarder<'_> {
    type Error = Closed;

    async fn forward(&mut self, _: &[u8]) -> Result<(), Self::Error> {
        poll_fn(|_| {
            if self.ready.get() {
                Poll::Ready(Err(Closed))
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

#[test]
fn forwarding_failure_is_preserved_before_or_after_send_settlement() {
    for finish_send in [false, true] {
        let send_ready = Cell::new(false);
        let forward_ready = Cell::new(false);
        let starts = Cell::new(0);
        let mut source = Source(VecDeque::from([Ok(2)]));
        let mut sink = Sink {
            ready: &send_ready,
            starts: &starts,
            result: Ok(()),
        };
        let mut inbound = [0; 2];
        let mut running = pin!(send_frame_duplex(
            &mut source,
            &mut sink,
            &[1],
            &mut inbound,
            RefusingForwarder {
                ready: &forward_ready
            },
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert!(running.as_mut().poll(&mut context).is_pending());
        send_ready.set(finish_send);
        assert!(running.as_mut().poll(&mut context).is_pending());
        forward_ready.set(true);
        assert_eq!(
            running.as_mut().poll(&mut context),
            Poll::Ready(BleDuplexOutcome::ForwardFailed(Closed))
        );
        assert_eq!(starts.get(), 1);
    }
}

impl BleFrameForwarder for Forwarder<'_> {
    type Error = core::convert::Infallible;

    async fn forward(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        poll_fn(|_| match self.ready.get() {
            true => Poll::Ready(()),
            false => Poll::Pending,
        })
        .await;
        self.frames.push(frame.to_vec());
        Ok(())
    }
}

#[test]
fn receive_only_work_ignores_empty_frames_and_settles_current_forwarding_exactly_once() {
    let work_ready = Cell::new(false);
    let forward_ready = Cell::new(false);
    let completions = Cell::new(0);
    let mut source = Source(VecDeque::from([Ok(0), Ok(2), Ok(1)]));
    let mut inbound = [0; 2];
    let mut frames = Vec::new();
    {
        let work = poll_fn(|_| {
            assert_eq!(
                completions.get(),
                0,
                "completed work must not be polled again"
            );
            if work_ready.get() {
                completions.set(1);
                Poll::Ready(Ok::<_, Closed>(()))
            } else {
                Poll::Pending
            }
        });
        let work = pin!(work);
        let mut running = pin!(receive_frames_during(
            work,
            &mut source,
            &mut inbound,
            Forwarder {
                ready: &forward_ready,
                frames: &mut frames
            },
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert!(running.as_mut().poll(&mut context).is_pending());
        work_ready.set(true);
        assert!(running.as_mut().poll(&mut context).is_pending());
        assert_eq!(completions.get(), 1);
        forward_ready.set(true);
        assert_eq!(
            running.as_mut().poll(&mut context),
            Poll::Ready(BleDuplexOutcome::Finished(Ok(())))
        );
    }
    assert_eq!(
        (frames, source.0, completions.get()),
        (vec![vec![7, 7]], VecDeque::from([Ok(1)]), 1)
    );
}

#[test]
fn ready_work_wins_without_consuming_another_frame() {
    for result in [Ok(()), Err(Closed)] {
        let mut source = Source(VecDeque::from([Ok(2)]));
        let mut inbound = [0; 2];
        let mut frames = Vec::new();
        let forward_ready = Cell::new(true);
        {
            let work = pin!(core::future::ready(result));
            let mut running = pin!(receive_frames_during(
                work,
                &mut source,
                &mut inbound,
                Forwarder {
                    ready: &forward_ready,
                    frames: &mut frames
                },
            ));
            assert_eq!(
                running
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(BleDuplexOutcome::Finished(result))
            );
        }
        assert_eq!(
            (source.0, inbound, frames),
            (VecDeque::from([Ok(2)]), [0; 2], vec![])
        );
    }
}

#[test]
fn receive_driver_storage_is_independent_of_borrowed_work_size() {
    struct WorkState<const BYTES: usize>([u8; BYTES]);

    impl<const BYTES: usize> Future for WorkState<BYTES> {
        type Output = Result<(), Closed>;

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            if self.0.iter().all(|byte| *byte == 0) {
                Poll::Pending
            } else {
                Poll::Ready(Ok(()))
            }
        }
    }

    fn driver_size<const BYTES: usize>() -> usize {
        let work = pin!(WorkState([0; BYTES]));
        let mut source = Source(VecDeque::new());
        let mut inbound = [0; 2];
        let forward_ready = Cell::new(true);
        let mut frames = Vec::new();
        let driver = receive_frames_during(
            work,
            &mut source,
            &mut inbound,
            Forwarder {
                ready: &forward_ready,
                frames: &mut frames,
            },
        );
        core::mem::size_of_val(&driver)
    }

    assert_eq!(driver_size::<1>(), driver_size::<4096>());
}
