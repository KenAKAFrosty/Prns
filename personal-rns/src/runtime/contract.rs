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
    NextScheduledEngineWork,
};
use crate::interfaces::storage::InterfaceSet;
use crate::interfaces::{
    ConnectionState, InterfaceId, NextScheduledInterfaceWake, OutboundPacket, RegisteredInterface,
    SendError,
};
use crate::routing::storage::EngineStorage;
use crate::wire::MTU;

/// Summary of one pooled drive cycle.
#[derive(Debug)]
pub struct RuntimeStepOutput {
    pub ingested_packet_count: usize,
    pub accepted_announce_count: usize,
    pub scheduled_rebroadcast_count: usize,
    pub egress_directive_count: usize,
    pub next_poll: NextScheduledInterfaceWake,
}

#[derive(Clone, Copy)]
enum FanoutClass {
    LocalOriginated,
    Transit,
}

/// The engine, its registered interfaces, traffic meter, and [`Host`] that drives
/// them. Generic over the engine-state storage `S`, the interface set `I` (fixed or
/// growable), and the host `Ho`. Each interface is reached only through
/// [`RegisteredInterface`], so no concrete handle or worker type surfaces here.
pub struct Runtime<Ho, I, S>
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
{
    engine: EngineState<S>,
    interfaces: I,
    traffic: TrafficLedger,
    host: Ho,
}

impl<Ho, I, S> Runtime<Ho, I, S>
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
{
    /// Register each interface with the engine and hold the set. The host built the
    /// set, so its backing already decided the bound; this only registers, panicking
    /// if the host supplied a non-routable interface.
    pub fn new(mut engine: EngineState<S>, interfaces: I, host: Ho) -> Self {
        for interface in interfaces.iter() {
            engine
                .register_routable_interface_descriptor(interface.descriptor())
                .expect("interface descriptor is connected and transmits");
        }
        Self {
            engine,
            interfaces,
            traffic: TrafficLedger::new(),
            host,
        }
    }

    /// One pooled drive cycle: poll interfaces, drain inbound into the engine, tick,
    /// fan the due egress, originate a due self-announce. The host supplies the
    /// clock and entropy.
    pub fn cycle_once(
        &mut self,
        now: InstantMillis,
        entropy_seed: EngineCycleEntropySeed,
    ) -> RuntimeStepOutput {
        cycle_pooled(
            &mut self.engine,
            &mut self.interfaces,
            &mut self.traffic,
            now,
            entropy_seed,
        )
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        snapshot_pooled(&self.engine, &self.interfaces, &self.traffic)
    }

    pub fn next_wakeup(&self, now: InstantMillis) -> NextScheduledEngineWork {
        self.engine.next_wakeup(now)
    }

    pub fn interfaces(&self) -> &[I::Item] {
        self.interfaces.as_slice()
    }

    pub fn engine(&self) -> &EngineState<S> {
        &self.engine
    }

    /// Drive this runtime forever: at each wake the host supplies the clock and
    /// entropy, then the loop pools interfaces, hands the cycle's snapshot to
    /// `on_snapshot`, and computes the next engine or interface deadline. The only
    /// `.await` is the host's wake — a sync host blocks there, an async host suspends.
    pub async fn run<OnSnapshot>(self, mut on_snapshot: OnSnapshot) -> !
    where
        Ho: Host,
        OnSnapshot: FnMut(&RuntimeSnapshot),
    {
        let Self {
            mut engine,
            mut interfaces,
            mut traffic,
            mut host,
        } = self;
        let mut wake = NextScheduledEngineWork::Immediate;
        loop {
            let CycleStamp { now, seed } = host.wait(wake).await;
            let output = cycle_pooled(&mut engine, &mut interfaces, &mut traffic, now, seed);
            on_snapshot(&snapshot_pooled(&engine, &interfaces, &traffic));
            wake = host_wake(engine.next_wakeup(now), output.next_poll);
        }
    }
}

/// The pooled cycle over borrowed parts (so [`Runtime::run`] can drive it while
/// the host is borrowed separately). Both [`Runtime::cycle_once`] and
/// [`Runtime::run`] route through here.
fn cycle_pooled<I, S>(
    engine: &mut EngineState<S>,
    interfaces: &mut I,
    traffic: &mut TrafficLedger,
    now: InstantMillis,
    entropy_seed: EngineCycleEntropySeed,
) -> RuntimeStepOutput
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
{
    let entropy = EngineCycleEntropy::from_seed(entropy_seed);

    // Poll every interface; a self-driven one reports Idle, so this folds in each
    // runtime-driven worker's next deadline without the runtime knowing which is which.
    let mut next_poll = NextScheduledInterfaceWake::Idle;
    for started in interfaces.iter_mut() {
        next_poll = next_poll.sooner(started.poll(now));
    }

    // Drain inbound while packets still borrow from their interface ring slots.
    let mut ingested_packet_count = 0;
    let mut accepted_announce_count = 0;
    let mut scheduled_rebroadcast_count = 0;
    for started in interfaces.iter_mut() {
        let id = started.descriptor().id;
        started.drain_inbound(|packet| {
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

    // Apply each interface's pending control reports to its connection state.
    for started in interfaces.iter_mut() {
        started.drain_control_reports();
    }

    RuntimeStepOutput {
        ingested_packet_count,
        accepted_announce_count,
        scheduled_rebroadcast_count,
        egress_directive_count,
        next_poll,
    }
}

/// Fan one outgoing packet to the interfaces the engine named in `fire_on`,
/// metering its bytes to each target.
fn fan_to_handles<I>(
    interfaces: &mut I,
    traffic: &mut TrafficLedger,
    bytes: &[u8],
    fire_on: &[InterfaceId],
    fanout_class: FanoutClass,
) where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
{
    for started in interfaces.iter_mut() {
        // Decide eligibility off the descriptor first, so its borrow ends before
        // the mutable `send`.
        let descriptor = started.descriptor();
        let id = descriptor.id;
        let eligible = fire_on.contains(&id)
            && matches!(
                descriptor.state,
                ConnectionState::Connected | ConnectionState::Degraded
            )
            && match fanout_class {
                FanoutClass::LocalOriginated => descriptor.capabilities.allows_local_egress(),
                FanoutClass::Transit => descriptor.capabilities.allows_transit(),
            };
        if eligible {
            match started.send(OutboundPacket::new(bytes)) {
                Ok(()) => traffic.add_tx(id, bytes.len() as u64),
                Err(SendError::QueueFull | SendError::PacketTooLarge) => {}
            }
        }
    }
}

/// The app-facing [`RuntimeSnapshot`], over borrowed parts.
fn snapshot_pooled<I, S>(
    engine: &EngineState<S>,
    interfaces: &I,
    traffic: &TrafficLedger,
) -> RuntimeSnapshot
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
{
    let mut views = HeaplessVec::new();
    for started in interfaces.iter() {
        let descriptor = started.descriptor();
        let id = descriptor.id;
        let (reticulum_rx_byte_count, reticulum_tx_byte_count) = traffic.totals_for(id);
        // Truncates silently past MAX_REGISTERED_INTERFACES — the snapshot view buffer
        // is still fixed; a growable interface set outgrowing it wants snapshot storage.
        let _ = views.push(InterfaceView {
            id,
            connection_state: descriptor.state,
            reticulum_rx_byte_count,
            reticulum_tx_byte_count,
            tracked_destinations: engine.route_count_via(id) as u32,
        });
    }
    RuntimeSnapshot { interfaces: views }
}

fn host_wake(
    engine: NextScheduledEngineWork,
    interface: NextScheduledInterfaceWake,
) -> NextScheduledEngineWork {
    use NextScheduledEngineWork::{
        At as EngineAt, Idle as EngineIdle, Immediate as EngineImmediate,
    };
    use NextScheduledInterfaceWake::{At as PollAt, Idle as PollIdle, Immediate as PollImmediate};
    match (engine, interface) {
        (EngineImmediate, _) | (_, PollImmediate) => EngineImmediate,
        (EngineIdle, PollIdle) => EngineIdle,
        (EngineAt(deadline), PollIdle) | (EngineIdle, PollAt(deadline)) => EngineAt(deadline),
        (EngineAt(x), PollAt(y)) => EngineAt(InstantMillis(x.0.min(y.0))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use crate::engine::ENGINE_CYCLE_ENTROPY_LEN;
    use crate::interfaces::storage::FixedInterfaceSet;
    use crate::interfaces::{
        ConnectionState, ControlReport, DriverMode, EgressCapability, InboundPacket,
        IngressCapability, InterfaceCapabilities, InterfaceDescriptor, InterfaceHandle,
        InterfaceMode, MediumKind, StartedInterface, TransitCapability,
    };
    use crate::routing::storage::FixedCapacity;
    use crate::routing::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::wire::{PacketType, WirePacketHeader};

    /// Test-only canonical sizing — production has no storage defaults.
    type Cap = FixedCapacity<64, 64, 4096, 4, 512, 64>;

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

    type TestInterface = StartedInterface<TestHandle, core::convert::Infallible>;

    /// Collect started interfaces into a fixed set the runtime can own.
    fn interface_set(
        started: impl IntoIterator<Item = TestInterface>,
    ) -> FixedInterfaceSet<TestInterface, 4> {
        let mut set = FixedInterfaceSet::new();
        for interface in started {
            let _ = set.push(interface);
        }
        set
    }

    #[test]
    fn pools_inbound_into_the_engine_and_fans_the_rebroadcast_to_the_peer() {
        let raw = hx(RAW_ANNOUNCE);
        let source = iface(0xA1);
        let peer = iface(0xB2);
        let arrival = InstantMillis(1_000);

        let engine = EngineState::<Cap>::default();
        // A unit host (`()`): the cycle/snapshot seam needs no real substrate.
        let mut runtime = Runtime::new(
            engine,
            interface_set([
                started(source, std::vec![(arrival, raw.clone())]),
                started(peer, std::vec::Vec::new()),
            ]),
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
        let mut runtime = Runtime::new(
            engine,
            interface_set([
                started(source, std::vec![(arrival, raw.clone())]),
                started(peer, std::vec::Vec::new()),
            ]),
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
        let engine = EngineState::<Cap>::default();
        let mut runtime = Runtime::new(
            engine,
            interface_set([started_with_reports(
                id,
                [ControlReport::ConnectionState(ConnectionState::Degraded)],
            )]),
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
        let engine = EngineState::<Cap>::default();
        let mut runtime = Runtime::new(
            engine,
            interface_set([started_with_reports(id, [ControlReport::Stopped])]),
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
        let mut interfaces = interface_set([
            started_with_state_and_reports(
                connected,
                ConnectionState::Connected,
                std::vec::Vec::new(),
                [],
            ),
            started_with_state_and_reports(
                failed,
                ConnectionState::Failed,
                std::vec::Vec::new(),
                [],
            ),
        ]);
        let mut traffic = TrafficLedger::new();
        let bytes = [0xAA, 0xBB, 0xCC];

        fan_to_handles(
            &mut interfaces,
            &mut traffic,
            &bytes,
            &[connected, failed],
            FanoutClass::Transit,
        );

        assert_eq!(
            interfaces.as_slice()[0].handle.sent,
            std::vec![bytes.to_vec()]
        );
        assert!(interfaces.as_slice()[1].handle.sent.is_empty());
        assert_eq!(traffic.totals_for(connected), (0, bytes.len() as u64));
        assert_eq!(traffic.totals_for(failed), (0, 0));
    }

    #[test]
    fn fanout_distinguishes_local_only_from_transit_interfaces() {
        let local_only = iface(0x11);
        let transit = iface(0x22);
        let mut interfaces = interface_set([
            started_with_descriptor_and_reports(
                descriptor_with_capabilities(
                    local_only,
                    ConnectionState::Connected,
                    EgressCapability::Enabled(TransitCapability::NoTransit),
                ),
                std::vec::Vec::new(),
                [],
            ),
            started_with_descriptor_and_reports(
                descriptor_with_capabilities(
                    transit,
                    ConnectionState::Connected,
                    EgressCapability::Enabled(TransitCapability::CrossInterfaceOnly),
                ),
                std::vec::Vec::new(),
                [],
            ),
        ]);
        let mut traffic = TrafficLedger::new();
        let bytes = [0xAA, 0xBB, 0xCC];

        fan_to_handles(
            &mut interfaces,
            &mut traffic,
            &bytes,
            &[local_only, transit],
            FanoutClass::Transit,
        );

        assert!(interfaces.as_slice()[0].handle.sent.is_empty());
        assert_eq!(
            interfaces.as_slice()[1].handle.sent,
            std::vec![bytes.to_vec()]
        );
        assert_eq!(traffic.totals_for(local_only), (0, 0));
        assert_eq!(traffic.totals_for(transit), (0, bytes.len() as u64));

        fan_to_handles(
            &mut interfaces,
            &mut traffic,
            &bytes,
            &[local_only, transit],
            FanoutClass::LocalOriginated,
        );

        assert_eq!(
            interfaces.as_slice()[0].handle.sent,
            std::vec![bytes.to_vec()]
        );
        assert_eq!(
            interfaces.as_slice()[1].handle.sent,
            std::vec![bytes.to_vec(), bytes.to_vec()]
        );
        assert_eq!(traffic.totals_for(local_only), (0, bytes.len() as u64));
        assert_eq!(traffic.totals_for(transit), (0, 2 * bytes.len() as u64));
    }
}
