use heapless::Vec as HeaplessVec;

use super::snapshot::{InterfaceView, RuntimeSnapshot};
use crate::engine::{
    ingest_packets, tick, EngineCycleEntropy, EngineCycleEntropySeed, EngineState, InboundPacket,
    InstantMillis, NextScheduledWakeup, OutboundPacket, StepOutput, TickSummary,
    MAX_REGISTERED_INTERFACES,
};
use crate::interfaces::{InterfaceId, InterfaceWorker};
use crate::routing::storage::{
    AnnounceIdHistory, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
};
use crate::wire::MTU;

/// The substrate-neutral engine-bolt: it owns the engine and the registered
/// workers, and turns one drive cycle into intake → engine step → exhaust.
///
/// Given the inbound the runtime aggregated plus a fresh clock and entropy,
/// [`cycle_once`](Manifold::cycle_once) applies the engine primitives directly —
/// ingesting, ticking, and routing every egress directive to the worker(s) named in
/// its `fire_on` list via [`InterfaceWorker::submit`]. The over-time loop that
/// decides *when* to cycle, draws entropy, and aggregates inbound is the
/// per-platform runtime, layered above this — never part of it. `now`/`entropy`
/// are passed in so the manifold itself touches no clock or RNG and stays
/// neutral across std and embassy hosts.
///
/// Holds a set of workers (capacity [`MAX_REGISTERED_INTERFACES`], the same cap
/// the engine routes within). A host that runs more than one interface composes
/// `W` as its own worker enum so the set stays a single concrete type — no
/// `dyn`, no alloc, and the inbound mailbox stays sized to one compile-time
/// [`InterfaceWorker::PACKET_BUFFER_SIZE`]. Generic over the engine-state
/// storage so a constrained host can pick a small preset (a desk node needs far
/// fewer tracked destinations / history slots than the desktop default).
pub struct Manifold<W, R, A, H, D, const MAX_HELD: usize>
where
    W: InterfaceWorker,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    engine: EngineState<R, A, H, D, MAX_HELD>,
    workers: HeaplessVec<W, MAX_REGISTERED_INTERFACES>,
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
    /// Bolt `engine` to `workers`, registering each (by its descriptor) so the
    /// engine routes to it. Panics if a worker is not routable
    /// (`Connected`/`Degraded` + transmits) or if more than
    /// [`MAX_REGISTERED_INTERFACES`] are supplied — a host wires a known-good
    /// set at boot. Pass an array, e.g. `Manifold::new(state, [wifi])` or
    /// `Manifold::new(state, [HostWorker::Wifi(w), HostWorker::Usb(u)])`.
    pub fn new(
        mut engine: EngineState<R, A, H, D, MAX_HELD>,
        workers: impl IntoIterator<Item = W>,
    ) -> Self {
        let mut set: HeaplessVec<W, MAX_REGISTERED_INTERFACES> = HeaplessVec::new();
        for worker in workers {
            let descriptor = worker.descriptor();
            engine
                .register_routable_descriptor(&descriptor)
                .expect("worker descriptor is connected and transmits");
            if set.push(worker).is_err() {
                panic!("more workers than MAX_REGISTERED_INTERFACES");
            }
        }
        Self {
            engine,
            workers: set,
            traffic: TrafficLedger::new(),
        }
    }

    /// The whole per-cycle orchestration, applied directly
    /// against the manifold's own engine, workers, and traffic meter: carve the
    /// step `seed`, ingest `inbound` (metering each packet to the interface that
    /// stamped it), tick at `now`, fan every due egress directive to the workers
    /// it names, and originate a due self-announce. The substrate — clock,
    /// entropy, the inbound stream — is the caller's; the per-interface I/O is
    /// the workers'. Returns the step summary.
    pub fn cycle_once<'p>(
        &mut self,
        now: InstantMillis,
        entropy_seed: EngineCycleEntropySeed,
        inbound: impl IntoIterator<Item = InboundPacket<'p>>,
    ) -> StepOutput {
        let Self {
            engine,
            workers,
            traffic,
        } = self;
        let entropy = EngineCycleEntropy::from_seed(entropy_seed);

        // Ingest the inbound batch, metering each packet to the interface that
        // stamped it (a worker stamps only real Reticulum traffic; its own
        // discovery chatter never reaches here).
        let traffic_meter = &mut *traffic;
        let ingest_output = ingest_packets(
            engine,
            inbound.into_iter().inspect(move |packet| {
                traffic_meter.add_rx(packet.source_interface, packet.bytes.len() as u64);
            }),
            entropy.jitter,
        );

        // Tick, then fan each due egress directive out to the workers it names.
        let tick_output = tick(engine, now, entropy.jitter);
        let tick_summary = TickSummary {
            egress_directive_count: tick_output.egress_directive_count(),
            recovered_from_held_count: tick_output.recovered_from_held_count(),
        };
        let mut emit_buffer = [0u8; MTU];
        for directive in tick_output.egress_directives() {
            let n = directive
                .to_wire(&mut emit_buffer)
                .expect("MTU-sized buf fits any valid wire packet");
            fan_out(
                workers.as_mut_slice(),
                traffic,
                OutboundPacket::new(&emit_buffer[..n]),
                directive.fire_on(),
            );
        }
        tick_output.commit();

        // Originate our own announce when one is due — fanned to every registered
        // interface (no source to exclude), only when there is one to carry it.
        let mut self_announce_minted = false;
        if !engine.registered_interfaces().is_empty() {
            if let Some(n) =
                engine.write_due_self_announce(now, entropy.self_announce, &mut emit_buffer)
            {
                fan_out(
                    workers.as_mut_slice(),
                    traffic,
                    OutboundPacket::new(&emit_buffer[..n]),
                    engine.registered_interfaces(),
                );
                self_announce_minted = true;
            }
        }

        let entropy_consumed = self_announce_minted
            || ingest_output.scheduled_rebroadcast_count() > 0
            || tick_summary.recovered_from_held_count > 0;

        StepOutput {
            ingest: ingest_output,
            tick: tick_summary,
            entropy_consumed,
        }
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
        for worker in &self.workers {
            let descriptor = worker.descriptor();
            let (reticulum_rx_bytes, reticulum_tx_bytes) = self.traffic.totals_for(descriptor.id);
            // `push` can't overflow: workers.len() <= MAX_REGISTERED_INTERFACES,
            // the same cap RuntimeSnapshot.interfaces is sized to.
            let _ = interfaces.push(InterfaceView {
                id: descriptor.id,
                online: worker.health().link.is_up(),
                reticulum_rx_bytes,
                reticulum_tx_bytes,
                // Destinations whose route was learned on this interface — the
                // routing table's per-interface tally, not the global total.
                tracked_destinations: self.engine.route_count_via(descriptor.id) as u32,
            });
        }
        RuntimeSnapshot { interfaces }
    }

    /// When the engine next needs a cycle — the runtime mins this with its own
    /// inbound readiness to size its sleep.
    pub fn next_wakeup(&self, now: InstantMillis) -> NextScheduledWakeup {
        self.engine.next_wakeup(now)
    }

    pub fn workers(&self) -> &[W] {
        &self.workers
    }

    pub fn engine(&self) -> &EngineState<R, A, H, D, MAX_HELD> {
        &self.engine
    }
}

/// Fan one outgoing packet to the workers the engine named in `fire_on`,
/// metering its bytes to each target. Non-blocking: a full worker queue drops
/// the packet — the engine re-emits announces on its own cadence, so a drop
/// self-heals.
fn fan_out<W: InterfaceWorker>(
    workers: &mut [W],
    traffic: &mut TrafficLedger,
    packet: OutboundPacket,
    fire_on: &[InterfaceId],
) {
    for id in fire_on {
        traffic.add_tx(*id, packet.bytes.len() as u64);
    }
    for worker in workers.iter_mut() {
        if fire_on.contains(&worker.descriptor().id) {
            let _ = worker.submit(packet);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::FixedCapacityEngineState;
    use crate::interfaces::{
        Capabilities, ConnectionState, InterfaceDescriptor, InterfaceMode, InterfaceStats,
        LinkState, MediumKind,
    };
    use crate::routing::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::wire::{PacketType, WirePacketHeader};

    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    #[derive(Debug)]
    struct TestWorker {
        descriptor: InterfaceDescriptor,
        health: InterfaceStats,
        submitted: std::vec::Vec<std::vec::Vec<u8>>,
    }

    impl InterfaceWorker for TestWorker {
        const PACKET_BUFFER_SIZE: usize = 500;

        fn descriptor(&self) -> InterfaceDescriptor {
            self.descriptor
        }

        fn health(&self) -> InterfaceStats {
            self.health
        }

        fn submit(&mut self, packet: OutboundPacket) -> Result<(), crate::interfaces::QueueFull> {
            self.submitted.push(packet.bytes.to_vec());
            Ok(())
        }
    }

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    fn worker(id: InterfaceId, online: bool) -> TestWorker {
        TestWorker {
            descriptor: InterfaceDescriptor {
                id,
                capabilities: Capabilities {
                    receives: true,
                    transmits: true,
                    forwards: true,
                    repeats: false,
                },
                mode: InterfaceMode::Full,
                medium: MediumKind::Loopback,
                state: ConnectionState::Connected,
            },
            health: InterfaceStats {
                link: LinkState::from_up(online),
                rx_packet_count: 0,
                tx_packet_count: 0,
            },
            submitted: std::vec::Vec::new(),
        }
    }

    #[test]
    fn new_registers_workers_and_snapshot_reflects_worker_health() {
        let first = iface(0xA1);
        let second = iface(0xB2);
        let engine: FixedCapacityEngineState = FixedCapacityEngineState::default();

        let manifold = Manifold::new(engine, [worker(first, true), worker(second, false)]);

        assert_eq!(manifold.engine().registered_interfaces(), &[first, second]);
        assert_eq!(manifold.workers().len(), 2);
        let snapshot = manifold.snapshot();
        assert_eq!(snapshot.interfaces.len(), 2);
        assert_eq!(
            snapshot.interfaces[0],
            InterfaceView {
                id: first,
                online: true,
                reticulum_rx_bytes: 0,
                reticulum_tx_bytes: 0,
                tracked_destinations: 0,
            }
        );
        assert_eq!(
            snapshot.interfaces[1],
            InterfaceView {
                id: second,
                online: false,
                reticulum_rx_bytes: 0,
                reticulum_tx_bytes: 0,
                tracked_destinations: 0,
            }
        );
    }

    #[test]
    fn cycle_meters_traffic_and_submits_due_rebroadcast_to_fire_on_worker() {
        let raw = hx(RAW_ANNOUNCE);
        let source = iface(0xA1);
        let peer = iface(0xB2);
        let arrival = InstantMillis(1_000);
        let inbound = [InboundPacket {
            arrived_at: arrival,
            source_interface: source,
            bytes: &raw,
        }];
        let engine: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let mut manifold = Manifold::new(engine, [worker(source, true), worker(peer, true)]);

        let out = manifold.cycle_once(
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            EngineCycleEntropySeed::new([0xCA; crate::engine::ENGINE_CYCLE_ENTROPY_LEN]),
            inbound.iter().copied(),
        );

        assert_eq!(out.ingest.accepted_announce_count(), 1);
        assert_eq!(out.ingest.scheduled_rebroadcast_count(), 1);
        assert_eq!(out.tick.egress_directive_count, 1);
        assert!(manifold.workers()[0].submitted.is_empty());
        assert_eq!(manifold.workers()[1].submitted.len(), 1);

        let (original_header, original_payload) = WirePacketHeader::parse(&raw).unwrap();
        let submitted = &manifold.workers()[1].submitted[0];
        let (submitted_header, submitted_payload) = WirePacketHeader::parse(submitted).unwrap();
        assert_eq!(submitted_header.packet_type, PacketType::Announce);
        assert_eq!(submitted_header.hops, original_header.hops + 1);
        assert_eq!(submitted_header.destination, original_header.destination);
        assert_eq!(submitted_payload, original_payload);

        let snapshot = manifold.snapshot();
        assert_eq!(snapshot.interfaces.len(), 2);
        assert_eq!(snapshot.interfaces[0].id, source);
        assert_eq!(snapshot.interfaces[0].reticulum_rx_bytes, raw.len() as u64);
        assert_eq!(snapshot.interfaces[0].reticulum_tx_bytes, 0);
        // The destination was learned on `source`, so only it tallies the route.
        assert_eq!(snapshot.interfaces[0].tracked_destinations, 1);
        assert_eq!(snapshot.interfaces[1].id, peer);
        assert_eq!(snapshot.interfaces[1].reticulum_rx_bytes, 0);
        assert_eq!(
            snapshot.interfaces[1].reticulum_tx_bytes,
            submitted.len() as u64
        );
        // `peer` only re-broadcast the announce; it didn't learn the route, so
        // its per-interface tally stays zero — the global total would say 1.
        assert_eq!(snapshot.interfaces[1].tracked_destinations, 0);
    }
}
