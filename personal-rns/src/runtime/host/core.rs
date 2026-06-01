use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use crate::engine::{
    EngineCycleEntropySeed, InboundPacket, InstantMillis, NextScheduledEngineWork,
};

/// The clock reading and entropy seed a [`Host`] samples at wake, handed to one
/// cycle of the runtime. Owned (not borrowed) so the runner can move the
/// move-only seed into the cycle while the host's inbound batch is borrowed
/// separately through [`Host::inbound_packets`].
pub struct CycleStamp {
    pub now: InstantMillis,
    pub seed: EngineCycleEntropySeed,
}

/// A host: the few substrate methods a platform implements so the runtime can
/// drive it. The host owns the real clock, the CSPRNG, the inbound mailbox, and
/// the sleep primitive — whatever the runtime needs from the outside world flows
/// *up* to it. The runtime (which owns the engine + workers) asks the host for
/// one cycle's fuel at a time; see [`run`](super::super::run).
///
/// `async` because that is the unifying substrate: an executor-backed host
/// suspends in [`wait`](Self::wait); a sync poll-loop host blocks there and never
/// yields, so [`block_on`] drives it with no executor.
#[allow(async_fn_in_trait)]
pub trait Host {
    /// Sleep until the engine's next scheduled work (`wake`) is due or inbound
    /// arrives — draining whatever queued into the host's own batch buffer —
    /// then sample the clock and draw a fresh per-cycle seed. The only `.await`
    /// in the loop: where an executor-backed host suspends and a sync host blocks.
    async fn wait(&mut self, wake: NextScheduledEngineWork) -> CycleStamp;

    /// Lend the inbound drained by the last [`wait`](Self::wait) as borrowed
    /// packets — a lazy view over the host's batch buffer. Returning an iterator
    /// rather than a `&[InboundPacket]` slice is what keeps this implementable:
    /// the host owns the bytes and yields views on the fly, so nothing stores a
    /// contiguous packet array borrowing its own buffer (which would be
    /// self-referential).
    fn inbound_packets(&self) -> impl Iterator<Item = InboundPacket<'_>>;
}

/// Drive a runtime future on a substrate with no async executor — a sync
/// poll-loop host. Such a host's [`Host::wait`] blocks internally and never
/// yields `Pending`, so the future runs straight through; the noop waker is never
/// actually scheduled. An executor-backed host drives [`run`](super::super::run)
/// with its own executor and never calls this.
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
    use crate::engine::ENGINE_CYCLE_ENTROPY_LEN;
    use crate::interfaces::InterfaceId;

    #[test]
    fn block_on_runs_a_ready_future_to_completion() {
        assert_eq!(block_on(async { 40 + 2 }), 42);
    }

    #[test]
    fn block_on_repolls_past_a_pending() {
        // Yields `Pending` once (without scheduling a waker), then `Ready` —
        // proves `block_on` re-polls rather than parking forever.
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

    /// A host serving exactly one canned inbound packet — exercises the `Host`
    /// contract (wait → stamp, inbound_packets → the batch) without an executor.
    struct OneShotHost {
        bytes: [u8; 1],
    }
    impl Host for OneShotHost {
        async fn wait(&mut self, _wake: NextScheduledEngineWork) -> CycleStamp {
            CycleStamp {
                now: InstantMillis(10),
                seed: EngineCycleEntropySeed::new([0u8; ENGINE_CYCLE_ENTROPY_LEN]),
            }
        }
        fn inbound_packets(&self) -> impl Iterator<Item = InboundPacket<'_>> {
            core::iter::once(InboundPacket {
                arrived_at: InstantMillis(5),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &self.bytes,
            })
        }
    }

    #[test]
    fn host_wait_yields_a_stamp_and_lends_its_inbound() {
        let mut host = OneShotHost { bytes: [0xAA] };
        let stamp = block_on(host.wait(NextScheduledEngineWork::Immediate));
        assert_eq!(stamp.now, InstantMillis(10));

        let packets: std::vec::Vec<InboundPacket<'_>> = host.inbound_packets().collect();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].source_interface, InterfaceId::new([0u8; 16]));
        assert_eq!(packets[0].bytes, &[0xAA][..]);
    }
}
