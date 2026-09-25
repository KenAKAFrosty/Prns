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
