use core::convert::Infallible;

use heapless::Vec as HeaplessVec;

use super::snapshot::{InterfaceView, RuntimeSnapshot};
use crate::engine::{
    EngineDriver, EngineState, InboundPacket, InstantMillis, NextScheduledWakeup, StepOutput,
    MAX_REGISTERED_INTERFACES,
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
    traffic: TrafficLedger,
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
        Self {
            engine,
            worker,
            traffic: TrafficLedger::new(),
        }
    }

    /// One drive cycle: ingest `inbound`, tick at `now` with `entropy`, and
    /// route the resulting egress to the worker. Returns the step summary.
    pub fn cycle(
        &mut self,
        now: InstantMillis,
        entropy: u64,
        inbound: &[InboundPacket<'_>],
    ) -> StepOutput {
        let Self {
            engine,
            worker,
            traffic,
        } = self;
        let mut driver = ManifoldDriver {
            now,
            entropy,
            inbound,
            worker,
            traffic,
        };
        driver
            .step(engine)
            .expect("manifold driver ops are infallible")
    }

    /// The app-facing [`RuntimeSnapshot`] for this cycle: per registered
    /// interface, its liveness, the cumulative Reticulum bytes it has carried,
    /// and the destinations the routing table tracks behind it.
    ///
    /// This is the one seam an app reads the engine through — it never touches
    /// engine internals — so the manifold decides here what is visible. Cheap
    /// and non-blocking; the per-platform runtime publishes it post-cycle (e.g.
    /// into a `Watch` a display subscribes to), so the app reacts to engine
    /// changes without polling.
    pub fn snapshot(&self) -> RuntimeSnapshot {
        let mut interfaces = HeaplessVec::new();
        let descriptor = self.worker.descriptor();
        let (reticulum_rx_bytes, reticulum_tx_bytes) = self.traffic.totals_for(descriptor.id);
        let _ = interfaces.push(InterfaceView {
            id: descriptor.id,
            online: self.worker.health().online,
            reticulum_rx_bytes,
            reticulum_tx_bytes,
            // Single worker today; once the routing table records the learned-on
            // interface, this reads its per-interface share instead of the whole
            // table. Correct as-is while a node runs one interface.
            tracked_destinations: self.engine.route_count() as u32,
        });
        RuntimeSnapshot { interfaces }
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
    traffic: &'a mut TrafficLedger,
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
        // Every packet here is real Reticulum traffic the worker stamped — its
        // own discovery/beacon chatter never reaches the mailbox — so this is
        // the exact point to meter inbound fabric bytes, attributed to the
        // interface that heard each one.
        let inbound = self.inbound;
        for packet in inbound {
            self.traffic
                .add_rx(packet.source_interface, packet.bytes.len() as u64);
        }
        Ok(inbound)
    }

    fn handle_egress(&mut self, bytes: &[u8], fire_on: &[InterfaceId]) -> Result<(), Self::Error> {
        // Mirror of the inbound meter: the engine emits a real Reticulum packet
        // on each `fire_on` interface, so count its bytes once per target.
        for id in fire_on {
            self.traffic.add_tx(*id, bytes.len() as u64);
        }
        if fire_on.contains(&self.worker.descriptor().id) {
            // Non-blocking; a full worker queue drops this packet — the engine
            // re-emits announces on its own cadence, so a drop self-heals.
            let _ = self.worker.submit(bytes);
        }
        Ok(())
    }
}

/// Cumulative Reticulum byte totals per interface, the manifold's own meter of
/// the fabric traffic crossing its seam. Capacity is [`MAX_REGISTERED_INTERFACES`]
/// — the same cap the engine's fanout uses — so an id from any registered
/// interface always has a slot.
struct TrafficLedger {
    entries: HeaplessVec<InterfaceTraffic, MAX_REGISTERED_INTERFACES>,
}

struct InterfaceTraffic {
    id: InterfaceId,
    reticulum_rx_bytes: u64,
    reticulum_tx_bytes: u64,
}

impl TrafficLedger {
    const fn new() -> Self {
        Self {
            entries: HeaplessVec::new(),
        }
    }

    /// The interface's row, inserting a zeroed one on first sight. `None` only
    /// if more than [`MAX_REGISTERED_INTERFACES`] distinct ids appear — which
    /// the engine's own registration cap forbids — so callers treat it as a
    /// benign no-op.
    fn row_mut(&mut self, id: InterfaceId) -> Option<&mut InterfaceTraffic> {
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            return Some(&mut self.entries[i]);
        }
        self.entries
            .push(InterfaceTraffic {
                id,
                reticulum_rx_bytes: 0,
                reticulum_tx_bytes: 0,
            })
            .ok()?;
        self.entries.last_mut()
    }

    fn add_rx(&mut self, id: InterfaceId, bytes: u64) {
        if let Some(row) = self.row_mut(id) {
            row.reticulum_rx_bytes = row.reticulum_rx_bytes.wrapping_add(bytes);
        }
    }

    fn add_tx(&mut self, id: InterfaceId, bytes: u64) {
        if let Some(row) = self.row_mut(id) {
            row.reticulum_tx_bytes = row.reticulum_tx_bytes.wrapping_add(bytes);
        }
    }

    /// `(rx, tx)` totals for `id`; `(0, 0)` for an interface that hasn't yet
    /// carried any Reticulum traffic.
    fn totals_for(&self, id: InterfaceId) -> (u64, u64) {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| (e.reticulum_rx_bytes, e.reticulum_tx_bytes))
            .unwrap_or((0, 0))
    }
}
