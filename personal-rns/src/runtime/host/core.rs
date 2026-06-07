use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use crate::engine::InstantMillis;

#[derive(Debug, Clone, Copy)]
pub enum NextWake {
    Immediate,
    At(InstantMillis),
    Idle,
}

/// `async` because that is the unifying substrate: an executor-backed host suspends in
/// [`wait`](Self::wait); a sync poll-loop host blocks there and never yields, so
/// [`block_on`] drives it with no executor.
#[allow(async_fn_in_trait)]
pub trait Host {
    async fn wait(&mut self, wake: NextWake) -> InstantMillis;
    fn fill_entropy(&mut self, bytes: &mut [u8]);
}

/// Drive a runtime future on a substrate with no async executor — a sync poll-loop
/// host. Such a host's [`Host::wait`] blocks internally and never yields `Pending`, so
/// the future runs straight through; the noop waker is never actually scheduled. An
/// executor-backed host drives [`Runtime::run`](crate::runtime::Runtime::run) with its
/// own executor and never calls this.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_runs_a_ready_future_to_completion() {
        assert_eq!(block_on(async { 40 + 2 }), 42);
    }

    #[test]
    fn block_on_repolls_past_a_pending() {
        struct YieldOnce(bool);
        impl Future for YieldOnce {
            type Output = u8;
            fn poll(mut self: core::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u8> {
                if self.0 {
                    Poll::Ready(7)
                } else {
                    self.0 = true;
                    Poll::Pending
                }
            }
        }
        assert_eq!(block_on(YieldOnce(false)), 7);
    }
}
