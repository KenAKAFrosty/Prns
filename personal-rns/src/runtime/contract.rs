//! Runtime for pooling started interfaces into the pure engine.
//!
//! Each cycle drains inbound packets from every [`InterfaceHandle`], ticks the
//! engine, and fans resulting egress back through the handles. Interface workers
//! wake the host when their queues need service; the runtime sleeps until an
//! interface wakes it or the engine has scheduled work due.

use heapless::Vec as HeaplessVec;

use super::core::TrafficLedger;
use super::host::{CycleStamp, Host};
use super::snapshot::{InterfaceView, RuntimeSnapshot};
use crate::engine::{
    ingest_packets, tick, EngineCycleEntropy, EngineCycleEntropySeed, EngineState, InstantMillis,
    NextScheduledEngineWork, OutboundPacket, MAX_REGISTERED_INTERFACES,
};
use crate::interfaces::{
    ControlReport, DriverMode, InterfaceHandle, InterfaceId, StartedInterface,
};
use crate::routing::storage::{
    AnnounceIdHistory, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
};
use crate::wire::MTU;

/// Summary of one pooled drive cycle.
#[derive(Debug)]
pub struct ContractStepOutput {
    pub ingested_packet_count: usize,
    pub accepted_announce_count: usize,
    pub scheduled_rebroadcast_count: usize,
    pub egress_directive_count: usize,
    pub next_poll: NextScheduledEngineWork,
}

/// The engine, started interfaces, traffic meter, and [`Host`] that drives them.
///
/// Generic over the engine-state storage (a constrained host picks a small preset)
/// and over the handle type `IH` + the runtime-driven worker type `W` — a host
/// composes both as its own concrete types (or [`core::convert::Infallible`] for
/// `W` when every interface is self-driven, as rnsd's are) so the set stays a
/// single concrete type: no `dyn`, no alloc. `Ho` is unbounded on the struct so
/// the cycle can be tested with a unit host; [`run_contract`] is what requires
/// [`Host`].
pub struct ContractRuntime<Ho, IH, W, R, A, H, D, const MAX_HELD: usize>
where
    IH: InterfaceHandle,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    engine: EngineState<R, A, H, D, MAX_HELD>,
    interfaces: HeaplessVec<StartedInterface<IH, W>, MAX_REGISTERED_INTERFACES>,
    traffic: TrafficLedger,
    host: Ho,
}

impl<Ho, IH, W, R, A, H, D, const MAX_HELD: usize> ContractRuntime<Ho, IH, W, R, A, H, D, MAX_HELD>
where
    IH: InterfaceHandle,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    /// Build a runtime and register each started interface with the engine.
    /// Panics if the host supplies a non-routable interface or more than
    /// [`MAX_REGISTERED_INTERFACES`] interfaces.
    pub fn new(
        mut engine: EngineState<R, A, H, D, MAX_HELD>,
        started: impl IntoIterator<Item = StartedInterface<IH, W>>,
        host: Ho,
    ) -> Self {
        let mut set: HeaplessVec<StartedInterface<IH, W>, MAX_REGISTERED_INTERFACES> =
            HeaplessVec::new();
        for interface in started {
            engine
                .register_routable_descriptor(&interface.descriptor)
                .expect("interface descriptor is connected and transmits");
            if set.push(interface).is_err() {
                panic!("more interfaces than MAX_REGISTERED_INTERFACES");
            }
        }
        Self {
            engine,
            interfaces: set,
            traffic: TrafficLedger::new(),
            host,
        }
    }

    /// One pooled drive cycle over the runtime's own engine + interfaces + meter:
    /// poll the runtime-driven interfaces, drain every handle's inbound into the
    /// engine, tick, fan the due egress, and originate a due self-announce. The
    /// host's substrate (clock, entropy) is supplied by the caller.
    pub fn cycle_once(
        &mut self,
        now: InstantMillis,
        entropy_seed: EngineCycleEntropySeed,
    ) -> ContractStepOutput {
        cycle_pooled(
            &mut self.engine,
            &mut self.interfaces,
            &mut self.traffic,
            now,
            entropy_seed,
        )
    }

    /// The app-facing [`RuntimeSnapshot`] for this cycle.
    pub fn snapshot(&self) -> RuntimeSnapshot {
        snapshot_pooled(&self.engine, &self.interfaces, &self.traffic)
    }

    /// When the engine next needs a cycle — mins with each cycle's `next_poll` in
    /// [`run_contract`] to size the sleep.
    pub fn next_wakeup(&self, now: InstantMillis) -> NextScheduledEngineWork {
        self.engine.next_wakeup(now)
    }

    pub fn interfaces(&self) -> &[StartedInterface<IH, W>] {
        &self.interfaces
    }

    pub fn engine(&self) -> &EngineState<R, A, H, D, MAX_HELD> {
        &self.engine
    }
}

/// Drive `runtime` forever. The host supplies the clock and entropy at each wake;
/// the loop then pools interfaces, emits a snapshot, and computes the next engine
/// or interface deadline.
pub async fn run_contract<Ho, Observe, IH, W, R, A, H, D, const MAX_HELD: usize>(
    runtime: ContractRuntime<Ho, IH, W, R, A, H, D, MAX_HELD>,
    mut observe: Observe,
) -> !
where
    Ho: Host,
    Observe: FnMut(&RuntimeSnapshot),
    IH: InterfaceHandle,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    let ContractRuntime {
        mut engine,
        mut interfaces,
        mut traffic,
        mut host,
    } = runtime;
    let mut wake = NextScheduledEngineWork::Immediate;
    loop {
        let CycleStamp { now, seed } = host.wait(wake).await;
        let output = cycle_pooled(&mut engine, &mut interfaces, &mut traffic, now, seed);
        observe(&snapshot_pooled(&engine, &interfaces, &traffic));
        wake = sooner(engine.next_wakeup(now), output.next_poll);
    }
}

/// The pooled cycle over borrowed parts (so [`run_contract`] can drive it while
/// the host is borrowed separately). Both [`ContractRuntime::cycle_once`] and
/// [`run_contract`] route through here.
fn cycle_pooled<IH, W, R, A, H, D, const MAX_HELD: usize>(
    engine: &mut EngineState<R, A, H, D, MAX_HELD>,
    interfaces: &mut HeaplessVec<StartedInterface<IH, W>, MAX_REGISTERED_INTERFACES>,
    traffic: &mut TrafficLedger,
    now: InstantMillis,
    entropy_seed: EngineCycleEntropySeed,
) -> ContractStepOutput
where
    IH: InterfaceHandle,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    let entropy = EngineCycleEntropy::from_seed(entropy_seed);

    // Poll runtime-driven interfaces and fold their next deadlines into sleep.
    let mut next_poll = NextScheduledEngineWork::Idle;
    for started in interfaces.iter_mut() {
        if let DriverMode::RuntimeDriven { worker, poll } = &mut started.drive {
            next_poll = sooner(next_poll, poll(worker, now));
        }
    }

    // Drain inbound while packets still borrow from their interface ring slots.
    let mut ingested_packet_count = 0;
    let mut accepted_announce_count = 0;
    let mut scheduled_rebroadcast_count = 0;
    for started in interfaces.iter_mut() {
        let id = started.descriptor.id;
        started.handle.drain_inbound(|packet| {
            traffic.add_rx(id, packet.bytes.len() as u64);
            let out = ingest_packets(engine, core::iter::once(packet), entropy.jitter);
            ingested_packet_count += out.processed_packet_count();
            accepted_announce_count += out.accepted_announce_count();
            scheduled_rebroadcast_count += out.scheduled_rebroadcast_count();
        });
    }

    // Tick, then fan each due egress directive to the named interfaces.
    let tick_output = tick(engine, now, entropy.jitter);
    let egress_directive_count = tick_output.egress_directive_count();
    let mut emit_buffer = [0u8; MTU];
    for directive in tick_output.egress_directives() {
        let n = directive
            .to_wire(&mut emit_buffer)
            .expect("MTU-sized buf fits any valid wire packet");
        fan_to_handles(interfaces, traffic, &emit_buffer[..n], directive.fire_on());
    }
    tick_output.commit();

    // Originate our own announce when due.
    if !engine.registered_interfaces().is_empty() {
        if let Some(n) =
            engine.write_due_self_announce(now, entropy.self_announce, &mut emit_buffer)
        {
            fan_to_handles(
                interfaces,
                traffic,
                &emit_buffer[..n],
                engine.registered_interfaces(),
            );
        }
    }

    // Drain control reports.
    for started in interfaces.iter_mut() {
        while let Some(report) = started.handle.next_report() {
            match report {
                // Link-state reports are not surfaced yet.
                ControlReport::Stopped => {}
            }
        }
    }

    ContractStepOutput {
        ingested_packet_count,
        accepted_announce_count,
        scheduled_rebroadcast_count,
        egress_directive_count,
        next_poll,
    }
}

/// Fan one outgoing packet to the interfaces the engine named in `fire_on`,
/// metering its bytes to each target.
fn fan_to_handles<IH: InterfaceHandle, W>(
    interfaces: &mut HeaplessVec<StartedInterface<IH, W>, MAX_REGISTERED_INTERFACES>,
    traffic: &mut TrafficLedger,
    bytes: &[u8],
    fire_on: &[InterfaceId],
) {
    for id in fire_on {
        traffic.add_tx(*id, bytes.len() as u64);
    }
    for started in interfaces.iter_mut() {
        if fire_on.contains(&started.descriptor.id) {
            let _ = started.handle.send(OutboundPacket::new(bytes));
        }
    }
}

/// The app-facing [`RuntimeSnapshot`], over borrowed parts.
fn snapshot_pooled<IH, W, R, A, H, D, const MAX_HELD: usize>(
    engine: &EngineState<R, A, H, D, MAX_HELD>,
    interfaces: &HeaplessVec<StartedInterface<IH, W>, MAX_REGISTERED_INTERFACES>,
    traffic: &TrafficLedger,
) -> RuntimeSnapshot
where
    IH: InterfaceHandle,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    let mut views = HeaplessVec::new();
    for started in interfaces {
        let id = started.descriptor.id;
        let (reticulum_rx_bytes, reticulum_tx_bytes) = traffic.totals_for(id);
        // `push` can't overflow: interfaces.len() <= MAX_REGISTERED_INTERFACES.
        let _ = views.push(InterfaceView {
            id,
            // Link-state reports are not surfaced yet.
            online: true,
            reticulum_rx_bytes,
            reticulum_tx_bytes,
            tracked_destinations: engine.route_count_via(id) as u32,
        });
    }
    RuntimeSnapshot { interfaces: views }
}

/// The sooner of two scheduled-work instants: `Immediate` beats everything, `Idle`
/// loses to everything, and two `At`s take the earlier deadline.
fn sooner(a: NextScheduledEngineWork, b: NextScheduledEngineWork) -> NextScheduledEngineWork {
    use NextScheduledEngineWork::{At, Idle, Immediate};
    match (a, b) {
        (Immediate, _) | (_, Immediate) => Immediate,
        (Idle, other) | (other, Idle) => other,
        (At(x), At(y)) => At(InstantMillis(x.0.min(y.0))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{FixedCapacityEngineState, InboundPacket, ENGINE_CYCLE_ENTROPY_LEN};
    use crate::interfaces::{
        Capabilities, ConnectionState, InterfaceDescriptor, InterfaceMode, MediumKind,
    };
    use crate::routing::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::wire::{PacketType, WirePacketHeader};

    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    /// A handle whose worker end is canned: it drains a preloaded inbound packet on
    /// the first `drain_inbound`, and captures whatever the runtime sends it.
    struct TestHandle {
        id: InterfaceId,
        inbound: std::vec::Vec<(InstantMillis, std::vec::Vec<u8>)>,
        sent: std::vec::Vec<std::vec::Vec<u8>>,
    }

    impl InterfaceHandle for TestHandle {
        fn drain_inbound(&mut self, mut f: impl FnMut(InboundPacket<'_>)) -> usize {
            let id = self.id;
            let mut drained = 0;
            for (arrived_at, bytes) in self.inbound.drain(..) {
                f(InboundPacket {
                    arrived_at,
                    source_interface: id,
                    bytes: &bytes,
                });
                drained += 1;
            }
            drained
        }

        fn send(&mut self, packet: OutboundPacket<'_>) -> bool {
            self.sent.push(packet.bytes.to_vec());
            true
        }

        fn request_stop(&mut self) {}

        fn next_report(&mut self) -> Option<ControlReport> {
            None
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

    fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
        InterfaceDescriptor {
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
        }
    }

    /// `W = Infallible`: rnsd's all-self-driven shape, so `RuntimeDriven` is
    /// unconstructable and the poll branch is dead-code-eliminated.
    fn started(
        id: InterfaceId,
        inbound: std::vec::Vec<(InstantMillis, std::vec::Vec<u8>)>,
    ) -> StartedInterface<TestHandle, core::convert::Infallible> {
        StartedInterface {
            descriptor: descriptor(id),
            handle: TestHandle {
                id,
                inbound,
                sent: std::vec::Vec::new(),
            },
            drive: DriverMode::SelfDriven,
        }
    }

    #[test]
    fn pools_inbound_into_the_engine_and_fans_the_rebroadcast_to_the_peer() {
        let raw = hx(RAW_ANNOUNCE);
        let source = iface(0xA1);
        let peer = iface(0xB2);
        let arrival = InstantMillis(1_000);

        let engine: FixedCapacityEngineState = FixedCapacityEngineState::default();
        // A unit host (`()`): the cycle/snapshot seam needs no real substrate.
        let mut runtime = ContractRuntime::new(
            engine,
            [
                started(source, std::vec![(arrival, raw.clone())]),
                started(peer, std::vec::Vec::new()),
            ],
            (),
        );

        let out = runtime.cycle_once(
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
        );

        // The announce was pooled in from `source` and a rebroadcast scheduled.
        assert_eq!(out.ingested_packet_count, 1);
        assert_eq!(out.accepted_announce_count, 1);
        assert_eq!(out.scheduled_rebroadcast_count, 1);
        assert_eq!(out.egress_directive_count, 1);

        // The rebroadcast was fanned to the peer's handle (not back to source).
        assert!(runtime.interfaces()[0].handle.sent.is_empty());
        assert_eq!(runtime.interfaces()[1].handle.sent.len(), 1);

        let submitted = &runtime.interfaces()[1].handle.sent[0];
        let (original_header, original_payload) = WirePacketHeader::parse(&raw).unwrap();
        let (submitted_header, submitted_payload) = WirePacketHeader::parse(submitted).unwrap();
        assert_eq!(submitted_header.packet_type, PacketType::Announce);
        assert_eq!(submitted_header.hops, original_header.hops + 1);
        assert_eq!(submitted_header.destination, original_header.destination);
        assert_eq!(submitted_payload, original_payload);

        // The snapshot meters rx to `source` and tx to `peer`, and the route was
        // learned on `source`.
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.interfaces[0].id, source);
        assert_eq!(snapshot.interfaces[0].reticulum_rx_bytes, raw.len() as u64);
        assert_eq!(snapshot.interfaces[0].tracked_destinations, 1);
        assert_eq!(snapshot.interfaces[1].id, peer);
        assert_eq!(
            snapshot.interfaces[1].reticulum_tx_bytes,
            submitted.len() as u64
        );
        assert_eq!(snapshot.interfaces[1].tracked_destinations, 0);
    }
}
