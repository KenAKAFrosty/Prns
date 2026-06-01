use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use super::super::{Manifold, RuntimeSnapshot};
use crate::engine::{EngineCycleEntropySeed, InboundPacket, InstantMillis, NextScheduledWakeup};
use crate::interfaces::InterfaceWorker;
use crate::routing::storage::{
    AnnounceIdHistory, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
};

/// The clock reading and entropy seed a [`RuntimeHost`] samples at wake, handed
/// to one [`Manifold::cycle_once`]. Owned (not borrowed) so the runner can move
/// the move-only seed into the cycle while the host's inbound batch is borrowed
/// separately through [`RuntimeHost::inbound_packets`].
pub struct CycleStamp {
    pub now: InstantMillis,
    pub seed: EngineCycleEntropySeed,
}

/// A host for the runtime: the one place a platform's outside world is touched.
/// A `RuntimeHost` owns the real clock, the CSPRNG, the inbound mailbox, and the
/// sleep primitive — whatever pieces it needs flow *up* to it rather than into
/// the engine. [`run`] drives a [`Manifold`] forever by asking the host for one
/// cycle's fuel at a time.
///
/// `async` because that is the unifying substrate (see the module docs): an
/// executor-backed host suspends in [`wait`](Self::wait); a sync poll-loop host
/// blocks there and never yields, so [`block_on`] drives it with no executor.
#[allow(async_fn_in_trait)]
pub trait RuntimeHost {
    /// Sleep until `wake` (the engine's next deadline) elapses or inbound
    /// arrives — draining whatever queued into the host's own batch buffer —
    /// then sample the clock and draw a fresh per-cycle seed. This is the only
    /// `.await` in the loop: where an executor-backed host suspends and a sync
    /// host blocks.
    async fn wait(&mut self, wake: NextScheduledWakeup) -> CycleStamp;

    /// Lend the inbound drained by the last [`wait`](Self::wait) as borrowed
    /// packets — a lazy view over the host's batch buffer. Returning an iterator
    /// rather than a `&[InboundPacket]` slice is what keeps this implementable:
    /// the host owns the bytes and yields views on the fly, so nothing stores a
    /// contiguous packet array borrowing its own buffer (which would be
    /// self-referential).
    fn inbound_packets(&self) -> impl Iterator<Item = InboundPacket<'_>>;
}

/// Drive `manifold` forever on `host`: ask the host for one cycle's clock +
/// entropy (which sleeps until there is work), cycle the engine on the host's
/// inbound, surface the fresh snapshot to `observe`, and feed the engine's next
/// deadline back as the next sleep target. The single per-platform loop — every
/// substrate differs only in its [`RuntimeHost`] impl.
///
/// `observe` is the app-facing read seam (a daemon logging route growth, a
/// display's `Watch` sender) — the embryonic read half of the consumer surface,
/// kept off [`RuntimeHost`] because it is app concern, not substrate.
pub async fn run<Host, Observe, W, R, A, H, D, const MAX_HELD: usize>(
    mut manifold: Manifold<W, R, A, H, D, MAX_HELD>,
    mut host: Host,
    mut observe: Observe,
) -> !
where
    Host: RuntimeHost,
    Observe: FnMut(&RuntimeSnapshot),
    W: InterfaceWorker,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    let mut wake = NextScheduledWakeup::Immediate;
    loop {
        let CycleStamp { now, seed } = host.wait(wake).await;
        let _ = manifold.cycle_once(now, seed, host.inbound_packets());
        observe(&manifold.snapshot());
        wake = manifold.next_wakeup(now);
    }
}

/// Drive a runtime future on a substrate with no async executor — a sync
/// poll-loop host. Such a host's [`RuntimeHost::wait`] blocks internally and
/// never yields `Pending`, so the future runs straight through; the noop waker
/// is never actually scheduled. An executor-backed host drives [`run`] with its
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
    use crate::engine::{FixedCapacityEngineState, OutboundPacket, ENGINE_CYCLE_ENTROPY_LEN};
    use crate::interfaces::{InterfaceDescriptor, InterfaceId, InterfaceStats, QueueFull};

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

    /// A host serving exactly one canned inbound packet, to drive one real cycle
    /// through the `wait` → `inbound` → `cycle_once` seam without an executor.
    struct OneShotHost {
        bytes: [u8; 1],
    }
    impl RuntimeHost for OneShotHost {
        async fn wait(&mut self, _wake: NextScheduledWakeup) -> CycleStamp {
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

    /// An uninhabited worker, to build a zero-interface `Manifold` for the cycle
    /// plumbing test without registering a real interface.
    enum NoWorker {}
    impl InterfaceWorker for NoWorker {
        const PACKET_BUFFER_SIZE: usize = 1;
        fn descriptor(&self) -> InterfaceDescriptor {
            match *self {}
        }
        fn health(&self) -> InterfaceStats {
            match *self {}
        }
        fn submit(&mut self, _packet: OutboundPacket) -> Result<(), QueueFull> {
            match *self {}
        }
    }

    #[test]
    fn wait_and_inbound_drive_one_cycle() {
        let no_workers: [NoWorker; 0] = [];
        let state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let mut manifold = Manifold::new(state, no_workers);
        let mut host = OneShotHost { bytes: [0xAA] };

        let processed = block_on(async {
            let CycleStamp { now, seed } = host.wait(NextScheduledWakeup::Immediate).await;
            manifold
                .cycle_once(now, seed, host.inbound_packets())
                .ingest
                .processed_packet_count()
        });

        // The host's one inbound packet flowed wait → inbound → cycle_once into
        // the engine.
        assert_eq!(processed, 1);
        assert_eq!(manifold.engine().ingested_packet_count(), 1);
    }
}
