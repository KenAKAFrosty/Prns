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
    NextScheduledEngineWork, MAX_REGISTERED_INTERFACES,
};
use crate::interfaces::{
    ConnectionState, ControlReport, DriverMode, InterfaceHandle, InterfaceId, OutboundPacket,
    SendError, StartedInterface,
};
use crate::routing::storage::Storage;
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

#[derive(Clone, Copy)]
enum FanoutClass {
    LocalOriginated,
    Transit,
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
pub struct ContractRuntime<Ho, IH, W, S>
where
    IH: InterfaceHandle,
    S: Storage,
{
    engine: EngineState<S>,
    interfaces: HeaplessVec<StartedInterface<IH, W>, MAX_REGISTERED_INTERFACES>,
    traffic: TrafficLedger,
    host: Ho,
}

impl<Ho, IH, W, S> ContractRuntime<Ho, IH, W, S>
where
    IH: InterfaceHandle,
    S: Storage,
{
    /// Build a runtime and register each started interface with the engine.
    /// Panics if the host supplies a non-routable interface or more than
    /// [`MAX_REGISTERED_INTERFACES`] interfaces.
    pub fn new(
        mut engine: EngineState<S>,
        started: impl IntoIterator<Item = StartedInterface<IH, W>>,
        host: Ho,
    ) -> Self {
        let mut set: HeaplessVec<StartedInterface<IH, W>, MAX_REGISTERED_INTERFACES> =
            HeaplessVec::new();
        for interface in started {
            engine
                .register_routable_interface_descriptor(&interface.descriptor)
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

    pub fn engine(&self) -> &EngineState<S> {
        &self.engine
    }
}

/// Drive `runtime` forever. The host supplies the clock and entropy at each wake;
/// the loop then pools interfaces, emits a snapshot, and computes the next engine
/// or interface deadline.
pub async fn run_contract<Ho, Observe, IH, W, S>(
    runtime: ContractRuntime<Ho, IH, W, S>,
    mut observe: Observe,
) -> !
where
    Ho: Host,
    Observe: FnMut(&RuntimeSnapshot),
    IH: InterfaceHandle,
    S: Storage,
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
fn cycle_pooled<IH, W, S>(
    engine: &mut EngineState<S>,
    interfaces: &mut HeaplessVec<StartedInterface<IH, W>, MAX_REGISTERED_INTERFACES>,
    traffic: &mut TrafficLedger,
    now: InstantMillis,
    entropy_seed: EngineCycleEntropySeed,
) -> ContractStepOutput
where
    IH: InterfaceHandle,
    S: Storage,
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
        fan_to_handles(
            interfaces,
            traffic,
            &emit_buffer[..n],
            directive.fire_on(),
            FanoutClass::Transit,
        );
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
                FanoutClass::LocalOriginated,
            );
        }
    }

    // Drain control reports.
    for started in interfaces.iter_mut() {
        while let Some(report) = started.handle.next_report() {
            match report {
                ControlReport::ConnectionState(connection_state) => {
                    started.descriptor.state = connection_state;
                }
                ControlReport::Stopped => {
                    started.descriptor.state = ConnectionState::Disconnected;
                }
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
    fanout_class: FanoutClass,
) {
    for started in interfaces.iter_mut() {
        if fire_on.contains(&started.descriptor.id)
            && matches!(
                started.descriptor.state,
                ConnectionState::Connected | ConnectionState::Degraded
            )
            && match fanout_class {
                FanoutClass::LocalOriginated => {
                    started.descriptor.capabilities.allows_local_egress()
                }
                FanoutClass::Transit => started.descriptor.capabilities.allows_transit(),
            }
        {
            match started.handle.send(OutboundPacket::new(bytes)) {
                Ok(()) => traffic.add_tx(started.descriptor.id, bytes.len() as u64),
                Err(SendError::QueueFull | SendError::PacketTooLarge) => {}
            }
        }
    }
}

/// The app-facing [`RuntimeSnapshot`], over borrowed parts.
fn snapshot_pooled<IH, W, S>(
    engine: &EngineState<S>,
    interfaces: &HeaplessVec<StartedInterface<IH, W>, MAX_REGISTERED_INTERFACES>,
    traffic: &TrafficLedger,
) -> RuntimeSnapshot
where
    IH: InterfaceHandle,
    S: Storage,
{
    let mut views = HeaplessVec::new();
    for started in interfaces {
        let id = started.descriptor.id;
        let (reticulum_rx_byte_count, reticulum_tx_byte_count) = traffic.totals_for(id);
        // `push` can't overflow: interfaces.len() <= MAX_REGISTERED_INTERFACES.
        let _ = views.push(InterfaceView {
            id,
            connection_state: started.descriptor.state,
            reticulum_rx_byte_count,
            reticulum_tx_byte_count,
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
    use std::collections::VecDeque;

    use crate::engine::ENGINE_CYCLE_ENTROPY_LEN;
    use crate::interfaces::{
        ConnectionState, EgressCapability, InboundPacket, IngressCapability, InterfaceCapabilities,
        InterfaceDescriptor, InterfaceMode, MediumKind, TransitCapability,
    };
    use crate::routing::storage::FixedCapacity;
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
        reports: VecDeque<ControlReport>,
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

        fn send(&mut self, packet: OutboundPacket<'_>) -> Result<(), SendError> {
            self.sent.push(packet.bytes.to_vec());
            Ok(())
        }

        fn request_stop(&mut self) {}

        fn next_report(&mut self) -> Option<ControlReport> {
            self.reports.pop_front()
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

    fn descriptor_with_state(id: InterfaceId, state: ConnectionState) -> InterfaceDescriptor {
        descriptor_with_capabilities(
            id,
            state,
            EgressCapability::Enabled(TransitCapability::CrossInterfaceOnly),
        )
    }

    fn descriptor_with_capabilities(
        id: InterfaceId,
        state: ConnectionState,
        egress: EgressCapability,
    ) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress,
            },
            mode: InterfaceMode::Full,
            medium: MediumKind::Loopback,
            state,
        }
    }

    /// `W = Infallible`: rnsd's all-self-driven shape, so `RuntimeDriven` is
    /// unconstructable and the poll branch is dead-code-eliminated.
    fn started(
        id: InterfaceId,
        inbound: std::vec::Vec<(InstantMillis, std::vec::Vec<u8>)>,
    ) -> StartedInterface<TestHandle, core::convert::Infallible> {
        started_with_state_and_reports(id, ConnectionState::Connected, inbound, [])
    }

    fn started_with_reports(
        id: InterfaceId,
        reports: impl IntoIterator<Item = ControlReport>,
    ) -> StartedInterface<TestHandle, core::convert::Infallible> {
        started_with_state_and_reports(
            id,
            ConnectionState::Connected,
            std::vec::Vec::new(),
            reports,
        )
    }

    fn started_with_state_and_reports(
        id: InterfaceId,
        state: ConnectionState,
        inbound: std::vec::Vec<(InstantMillis, std::vec::Vec<u8>)>,
        reports: impl IntoIterator<Item = ControlReport>,
    ) -> StartedInterface<TestHandle, core::convert::Infallible> {
        started_with_descriptor_and_reports(descriptor_with_state(id, state), inbound, reports)
    }

    fn started_with_descriptor_and_reports(
        descriptor: InterfaceDescriptor,
        inbound: std::vec::Vec<(InstantMillis, std::vec::Vec<u8>)>,
        reports: impl IntoIterator<Item = ControlReport>,
    ) -> StartedInterface<TestHandle, core::convert::Infallible> {
        StartedInterface {
            descriptor,
            handle: TestHandle {
                id: descriptor.id,
                inbound,
                reports: reports.into_iter().collect(),
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

        let engine = EngineState::<FixedCapacity>::default();
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
        assert_eq!(
            snapshot.interfaces[0].connection_state,
            ConnectionState::Connected
        );
        assert_eq!(
            snapshot.interfaces[0].reticulum_rx_byte_count,
            raw.len() as u64
        );
        assert_eq!(snapshot.interfaces[0].tracked_destinations, 1);
        assert_eq!(snapshot.interfaces[1].id, peer);
        assert_eq!(
            snapshot.interfaces[1].connection_state,
            ConnectionState::Connected
        );
        assert_eq!(
            snapshot.interfaces[1].reticulum_tx_byte_count,
            submitted.len() as u64
        );
        assert_eq!(snapshot.interfaces[1].tracked_destinations, 0);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn growable_heap_engine_drives_a_full_cycle() {
        use crate::routing::storage::GrowableHeap;
        let raw = hx(RAW_ANNOUNCE);
        let source = iface(0xA1);
        let peer = iface(0xB2);
        let arrival = InstantMillis(1_000);

        let engine = EngineState::<GrowableHeap>::default();
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

        // Same behavior as the fixed engine, on knob-free heap storage.
        assert_eq!(out.accepted_announce_count, 1);
        assert_eq!(out.egress_directive_count, 1);
        assert!(runtime.interfaces()[0].handle.sent.is_empty());
        assert_eq!(runtime.interfaces()[1].handle.sent.len(), 1);
    }

    #[test]
    fn snapshot_reflects_reported_connection_state() {
        let id = iface(0xC3);
        let engine = EngineState::<FixedCapacity>::default();
        let mut runtime = ContractRuntime::new(
            engine,
            [started_with_reports(
                id,
                [ControlReport::ConnectionState(ConnectionState::Degraded)],
            )],
            (),
        );

        let _ = runtime.cycle_once(
            InstantMillis(1),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
        );

        let snapshot = runtime.snapshot();
        assert_eq!(
            snapshot.interfaces[0].connection_state,
            ConnectionState::Degraded
        );
    }

    #[test]
    fn stopped_reports_disconnect_the_snapshot() {
        let id = iface(0xD4);
        let engine = EngineState::<FixedCapacity>::default();
        let mut runtime = ContractRuntime::new(
            engine,
            [started_with_reports(id, [ControlReport::Stopped])],
            (),
        );

        let _ = runtime.cycle_once(
            InstantMillis(1),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
        );

        let snapshot = runtime.snapshot();
        assert_eq!(
            snapshot.interfaces[0].connection_state,
            ConnectionState::Disconnected
        );
    }

    #[test]
    fn fanout_skips_interfaces_that_are_not_connected() {
        let connected = iface(0xE5);
        let failed = iface(0xF6);
        let mut interfaces = HeaplessVec::new();
        let _ = interfaces.push(started_with_state_and_reports(
            connected,
            ConnectionState::Connected,
            std::vec::Vec::new(),
            [],
        ));
        let _ = interfaces.push(started_with_state_and_reports(
            failed,
            ConnectionState::Failed,
            std::vec::Vec::new(),
            [],
        ));
        let mut traffic = TrafficLedger::new();
        let bytes = [0xAA, 0xBB, 0xCC];

        fan_to_handles(
            &mut interfaces,
            &mut traffic,
            &bytes,
            &[connected, failed],
            FanoutClass::Transit,
        );

        assert_eq!(interfaces[0].handle.sent, std::vec![bytes.to_vec()]);
        assert!(interfaces[1].handle.sent.is_empty());
        assert_eq!(traffic.totals_for(connected), (0, bytes.len() as u64));
        assert_eq!(traffic.totals_for(failed), (0, 0));
    }

    #[test]
    fn fanout_distinguishes_local_only_from_transit_interfaces() {
        let local_only = iface(0x11);
        let transit = iface(0x22);
        let mut interfaces = HeaplessVec::new();
        let _ = interfaces.push(started_with_descriptor_and_reports(
            descriptor_with_capabilities(
                local_only,
                ConnectionState::Connected,
                EgressCapability::Enabled(TransitCapability::NoTransit),
            ),
            std::vec::Vec::new(),
            [],
        ));
        let _ = interfaces.push(started_with_descriptor_and_reports(
            descriptor_with_capabilities(
                transit,
                ConnectionState::Connected,
                EgressCapability::Enabled(TransitCapability::CrossInterfaceOnly),
            ),
            std::vec::Vec::new(),
            [],
        ));
        let mut traffic = TrafficLedger::new();
        let bytes = [0xAA, 0xBB, 0xCC];

        fan_to_handles(
            &mut interfaces,
            &mut traffic,
            &bytes,
            &[local_only, transit],
            FanoutClass::Transit,
        );

        assert!(interfaces[0].handle.sent.is_empty());
        assert_eq!(interfaces[1].handle.sent, std::vec![bytes.to_vec()]);
        assert_eq!(traffic.totals_for(local_only), (0, 0));
        assert_eq!(traffic.totals_for(transit), (0, bytes.len() as u64));

        fan_to_handles(
            &mut interfaces,
            &mut traffic,
            &bytes,
            &[local_only, transit],
            FanoutClass::LocalOriginated,
        );

        assert_eq!(interfaces[0].handle.sent, std::vec![bytes.to_vec()]);
        assert_eq!(
            interfaces[1].handle.sent,
            std::vec![bytes.to_vec(), bytes.to_vec()]
        );
        assert_eq!(traffic.totals_for(local_only), (0, bytes.len() as u64));
        assert_eq!(traffic.totals_for(transit), (0, 2 * bytes.len() as u64));
    }
}
