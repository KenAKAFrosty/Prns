use core::convert::Infallible;

use crate::engine::{
    EngineDriver, EngineState, InboundPacket, InstantMillis, NextScheduledWakeup, StepOutput,
};
use crate::interfaces::{InterfaceId, InterfaceWorker};
use crate::routing::storage::{
    AnnounceIdHistory, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
};

/// The substrate-neutral engine-bolt: it owns the engine and the registered
/// worker, and turns one drive cycle into intake → engine step → exhaust.
///
/// It is the multi-worker generalization of [`EngineDriver`]: given the inbound
/// the runtime aggregated plus a fresh clock and entropy, [`cycle`](Manifold::cycle)
/// ingests, ticks, and routes every egress directive to the worker named in its
/// `fire_on` list via [`InterfaceWorker::submit`]. The over-time loop that
/// decides *when* to cycle, draws entropy, and aggregates inbound is the
/// per-platform runtime, layered above this — never part of it. `now`/`entropy`
/// are passed in so the manifold itself touches no clock or RNG and stays
/// neutral across std and embassy hosts.
///
/// Generic over the engine-state storage so a constrained host can pick a small
/// preset (a desk node needs far fewer tracked destinations / history slots than
/// the desktop default). Single-worker today; the worker field grows to a
/// registry when a host runs more than one — the routing (`fire_on` id → that
/// worker's `submit`) is already the shape a multi-worker manifold uses.
pub struct Manifold<W, R, A, H, D, const MAX_HELD: usize>
where
    W: InterfaceWorker,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    engine: EngineState<R, A, H, D, MAX_HELD>,
    worker: W,
}

impl<W, R, A, H, D, const MAX_HELD: usize> Manifold<W, R, A, H, D, MAX_HELD>
where
    W: InterfaceWorker,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    /// Bolt `engine` to `worker`, registering the worker (by its descriptor) so
    /// the engine routes to it. Panics if the worker is not routable
    /// (`Connected`/`Degraded` + transmits) — a host wires a live worker.
    pub fn new(mut engine: EngineState<R, A, H, D, MAX_HELD>, worker: W) -> Self {
        engine
            .register_routable_descriptor(&worker.descriptor())
            .expect("worker descriptor is connected and transmits");
        Self { engine, worker }
    }

    /// One drive cycle: ingest `inbound`, tick at `now` with `entropy`, and
    /// route the resulting egress to the worker. Returns the step summary.
    pub fn cycle(
        &mut self,
        now: InstantMillis,
        entropy: u64,
        inbound: &[InboundPacket<'_>],
    ) -> StepOutput {
        let Self { engine, worker } = self;
        let mut driver = ManifoldDriver {
            now,
            entropy,
            inbound,
            worker,
        };
        driver
            .step(engine)
            .expect("manifold driver ops are infallible")
    }

    /// When the engine next needs a cycle — the runtime mins this with its own
    /// inbound readiness to size its sleep.
    pub fn next_wakeup(&self, now: InstantMillis) -> NextScheduledWakeup {
        self.engine.next_wakeup(now)
    }

    pub fn worker(&self) -> &W {
        &self.worker
    }

    pub fn engine(&self) -> &EngineState<R, A, H, D, MAX_HELD> {
        &self.engine
    }
}

/// Per-cycle [`EngineDriver`] the manifold builds over borrowed pieces: the
/// runtime's `now`/`entropy`, the aggregated inbound batch, and the worker to
/// exhaust to. Infallible — every op is a copy or a non-blocking submit.
struct ManifoldDriver<'a, W: InterfaceWorker> {
    now: InstantMillis,
    entropy: u64,
    inbound: &'a [InboundPacket<'a>],
    worker: &'a mut W,
}

impl<W: InterfaceWorker> EngineDriver for ManifoldDriver<'_, W> {
    type Error = Infallible;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(self.now)
    }

    fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
        // The runtime drew CSPRNG-grade bytes this cycle; spread them across the
        // engine's request (it asks for 8).
        let bytes = self.entropy.to_le_bytes();
        for (dst, src) in buf.iter_mut().zip(bytes.iter().cycle()) {
            *dst = *src;
        }
        Ok(())
    }

    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(self.inbound)
    }

    fn handle_egress(&mut self, bytes: &[u8], fire_on: &[InterfaceId]) -> Result<(), Self::Error> {
        if fire_on.contains(&self.worker.descriptor().id) {
            // Non-blocking; a full worker queue drops this packet — the engine
            // re-emits announces on its own cadence, so a drop self-heals.
            let _ = self.worker.submit(bytes);
        }
        Ok(())
    }
}
