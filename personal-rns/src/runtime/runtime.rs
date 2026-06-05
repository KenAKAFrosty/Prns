use heapless::Vec as HeaplessVec;

use super::core::TrafficLedger;
use super::event::PrnsEvent;
use super::host::{CycleStamp, Host, NextWake};
use super::snapshot::{InterfaceView, RuntimeSnapshot};
use crate::engine::{
    AnnounceIngest, EngineCycleEntropy, EngineCycleEntropySeed, EngineState, IngestPacketOutcome,
    InstantMillis, NextScheduledEngineWork,
};
use crate::interfaces::storage::InterfaceSet;
use crate::interfaces::{
    ConnectionState, InterfaceId, NextScheduledInterfaceWake, RegisteredInterface, SendError,
};
use crate::routing::storage::EngineStorage;
use crate::wire::MTU;

#[derive(Debug)]
pub struct RuntimeStepOutput {
    pub ingested_packet_count: usize,
    pub accepted_announce_count: usize,
    pub scheduled_rebroadcast_count: usize,
    pub delivered_count: usize,
    pub egress_directive_count: usize,
    pub next_poll: NextScheduledInterfaceWake,
}

#[derive(Clone, Copy)]
enum FanoutClass {
    LocalOriginated,
    Transit,
}

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
    pub fn new(mut engine: EngineState<S>, interfaces: I, host: Ho) -> Self {
        for interface in interfaces.iter() {
            let _ = engine.register_interface_descriptor(interface.descriptor());
        }
        Self {
            engine,
            interfaces,
            traffic: TrafficLedger::new(),
            host,
        }
    }

    pub fn cycle_once(
        &mut self,
        now: InstantMillis,
        entropy_seed: EngineCycleEntropySeed,
        mut on_event: impl FnMut(PrnsEvent<'_>),
    ) -> RuntimeStepOutput {
        cycle_pooled(
            &mut self.engine,
            &mut self.interfaces,
            &mut self.traffic,
            now,
            entropy_seed,
            &mut on_event,
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

    pub async fn run<OnEvent>(self, mut on_event: OnEvent) -> !
    where
        Ho: Host,
        OnEvent: FnMut(PrnsEvent<'_>),
    {
        let Self {
            mut engine,
            mut interfaces,
            mut traffic,
            mut host,
        } = self;
        let mut wake = NextWake::Immediate;
        loop {
            let CycleStamp { now, seed } = host.wait(wake).await;
            let output = cycle_pooled(
                &mut engine,
                &mut interfaces,
                &mut traffic,
                now,
                seed,
                &mut on_event,
            );

            //Future improvement: Only fire this when actually updated (diffd for changes) instead of every cycle
            on_event(PrnsEvent::SnapshotUpdated(&snapshot_pooled(
                &engine,
                &interfaces,
                &traffic,
            )));
            wake = host_wake(engine.next_wakeup(now), output.next_poll);
        }
    }
}

#[allow(clippy::expect_used)]
fn cycle_pooled<I, S, OnEvent>(
    engine: &mut EngineState<S>,
    interfaces: &mut I,
    traffic: &mut TrafficLedger,
    now: InstantMillis,
    entropy_seed: EngineCycleEntropySeed,
    on_event: &mut OnEvent,
) -> RuntimeStepOutput
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
    OnEvent: FnMut(PrnsEvent<'_>),
{
    let entropy = EngineCycleEntropy::from_seed(entropy_seed);

    let mut next_poll = NextScheduledInterfaceWake::Idle;
    for started in interfaces.iter_mut() {
        next_poll = next_poll.sooner(started.poll(now));
    }

    let mut ingested_packet_count = 0;
    let mut accepted_announce_count = 0;
    let mut scheduled_rebroadcast_count = 0;
    let mut delivered_count = 0;
    for interface in interfaces.iter_mut() {
        let id = interface.descriptor().id;
        interface.drain_inbound(|packet| {
            traffic.add_rx(id, packet.bytes.len() as u64);
            ingested_packet_count += 1;
            match engine.ingest_packet(packet, entropy.jitter) {
                IngestPacketOutcome::Announce(AnnounceIngest::Accepted) => {
                    accepted_announce_count += 1;
                    scheduled_rebroadcast_count += 1;
                }
                IngestPacketOutcome::Announce(
                    AnnounceIngest::HeldForRetry | AnnounceIngest::Ignored,
                ) => {}
                IngestPacketOutcome::Delivery(delivery) => {
                    delivered_count += 1;
                    on_event(PrnsEvent::Delivered(delivery));
                }
                IngestPacketOutcome::Ignored => {}
            }
        });
    }

    let tick_output = engine.tick(now, entropy.jitter);
    let egress_directive_count = tick_output.egress_directive_count();
    for directive in tick_output.egress_directives() {
        // Serializing per interface (instead of once into a staging buffer) is
        // free — `to_wire` is a header write plus a copy of the retained,
        // already-signed payload — and it lets each packet land in its queue
        // slot grant directly. With IFAC it stops being an optimization and
        // becomes required: each interface masks the whole packet differently.
        fan_to_handles(
            interfaces,
            traffic,
            |buf| {
                directive
                    .to_wire(buf)
                    .expect("MTU-sized buf fits any valid wire packet")
            },
            directive.fire_on(),
            FanoutClass::Transit,
        );
    }
    tick_output.commit();

    if !engine.registered_interfaces().is_empty() {
        // The self-announce is staged once — building it signs, and signing
        // per interface would be waste — then the fan copies the staged bytes
        // into each grant.
        let mut emit_buffer = [0u8; MTU];
        if let Some(n) =
            engine.write_due_self_announce(now, entropy.self_announce, &mut emit_buffer)
        {
            fan_to_handles(
                interfaces,
                traffic,
                |buf| {
                    buf[..n].copy_from_slice(&emit_buffer[..n]);
                    n
                },
                engine.registered_interfaces(),
                FanoutClass::LocalOriginated,
            );
        }
    }

    for started in interfaces.iter_mut() {
        started.drain_control_reports();
    }

    RuntimeStepOutput {
        ingested_packet_count,
        accepted_announce_count,
        scheduled_rebroadcast_count,
        delivered_count,
        egress_directive_count,
        next_poll,
    }
}

fn fan_to_handles<I>(
    interfaces: &mut I,
    traffic: &mut TrafficLedger,
    write_packet: impl Fn(&mut [u8]) -> usize,
    fire_on: &[InterfaceId],
    fanout_class: FanoutClass,
) where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
{
    for started in interfaces.iter_mut() {
        // Decide eligibility off the descriptor first, so its borrow ends before
        // the mutable `send_with`.
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
            match started.send_with(&write_packet) {
                Ok(written) => traffic.add_tx(id, written as u64),
                Err(SendError::QueueFull) => {}
            }
        }
    }
}

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

fn host_wake(engine: NextScheduledEngineWork, interface: NextScheduledInterfaceWake) -> NextWake {
    use NextScheduledEngineWork::{
        At as EngineAt, Idle as EngineIdle, Immediate as EngineImmediate,
    };
    use NextScheduledInterfaceWake::{At as PollAt, Idle as PollIdle, Immediate as PollImmediate};
    match (engine, interface) {
        (EngineImmediate, _) | (_, PollImmediate) => NextWake::Immediate,
        (EngineIdle, PollIdle) => NextWake::Idle,
        (EngineAt(deadline), PollIdle) | (EngineIdle, PollAt(deadline)) => NextWake::At(deadline),
        (EngineAt(x), PollAt(y)) => NextWake::At(InstantMillis(x.0.min(y.0))),
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
    use crate::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::routing::delivery::Delivery;
    use crate::routing::storage::FixedCapacity;
    use crate::wire::{DestinationHash, PacketType, WirePacketHeader};

    type Cap = FixedCapacity<64, 64, 4096, 4, 512, 64, 8, 8, 128>;

    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

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
            for (arrived_at, mut bytes) in self.inbound.drain(..) {
                f(InboundPacket {
                    arrived_at,
                    source_interface: id,
                    bytes: &mut bytes,
                });
                drained += 1;
            }
            drained
        }

        fn acquire_send_grant(
            &mut self,
            fill: impl FnOnce(&mut [u8]) -> usize,
        ) -> Result<usize, SendError> {
            let mut buf = [0u8; MTU];
            let written = fill(&mut buf);
            self.sent.push(buf[..written].to_vec());
            Ok(written)
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
    fn a_delivered_plain_packet_reaches_the_event_handler_with_its_payload() {
        const RAW_PLAIN_DATA: &str = "080012f815e3e65add6ceb2fda0e7be338680068656c6c6f2d706c61696e";
        let raw = hx(RAW_PLAIN_DATA);
        let source = iface(0xA1);
        let arrival = InstantMillis(1_000);

        let mut engine = EngineState::<Cap>::default();
        let destination = engine
            .register_plain_destination("personal", &["node"])
            .unwrap();
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(source, std::vec![(arrival, raw.clone())])]),
            (),
        );

        let mut delivered: std::vec::Vec<(DestinationHash, std::vec::Vec<u8>, InterfaceId)> =
            std::vec::Vec::new();
        let out = runtime.cycle_once(
            InstantMillis(arrival.0 + 1),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
            |event| match event {
                PrnsEvent::Delivered(Delivery::Plain(delivery)) => delivered.push((
                    delivery.destination,
                    delivery.payload.to_vec(),
                    delivery.source_interface,
                )),
                PrnsEvent::Delivered(Delivery::Single(_)) => {
                    panic!("a plain packet must never surface as a single delivery")
                }
                PrnsEvent::SnapshotUpdated(_) => {}
            },
        );

        assert_eq!(out.ingested_packet_count, 1);
        assert_eq!(out.delivered_count, 1);
        assert_eq!(out.accepted_announce_count, 0);
        assert_eq!(
            delivered,
            std::vec![(destination, b"hello-plain".to_vec(), source)],
        );
    }

    #[test]
    fn a_delivered_single_packet_reaches_the_event_handler_decrypted() {
        use crate::crypto::X25519SecretKey;
        use crate::identity::in_memory::InMemoryNodeIdentity;
        use crate::identity::{IdentitySigner, RemoteIdentity, Zeroizing};
        use crate::wire::{
            ContextFlag, DestinationType, IfacFlag, PropagationType, WireContext, MTU,
        };

        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        let secret = Zeroizing::new(secret);

        let mut engine = EngineState::<Cap>::new(&secret);
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
        let destination = engine
            .register_single_destination(&identity.identity_hash(), "personal", &["node"])
            .unwrap();

        let remote = RemoteIdentity::from_public_keys(
            identity.encryption_public_key(),
            identity.signing_public_key(),
        );
        let header = crate::wire::WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination,
            context: WireContext::None,
        };
        let mut wire = [0u8; MTU];
        let header_len = header.write(&mut wire).unwrap();
        let sealed = remote
            .encrypt(
                &X25519SecretKey::new([0x77; 32]),
                &[0x88; 16],
                b"hello-through-the-stack",
                &mut wire[header_len..],
            )
            .unwrap();
        let raw = wire[..header_len + sealed].to_vec();

        let source = iface(0xA1);
        let arrival = InstantMillis(1_000);
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(source, std::vec![(arrival, raw)])]),
            (),
        );

        let mut delivered: std::vec::Vec<(DestinationHash, std::vec::Vec<u8>)> =
            std::vec::Vec::new();
        let out = runtime.cycle_once(
            InstantMillis(arrival.0 + 1),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
            |event| match event {
                PrnsEvent::Delivered(Delivery::Single(delivery)) => {
                    delivered.push((delivery.destination, delivery.plaintext.to_vec()))
                }
                PrnsEvent::Delivered(Delivery::Plain(_)) => {
                    panic!("a sealed single packet must never surface as plain")
                }
                PrnsEvent::SnapshotUpdated(_) => {}
            },
        );

        assert_eq!(out.delivered_count, 1);
        assert_eq!(
            delivered,
            std::vec![(destination, b"hello-through-the-stack".to_vec())],
        );
    }

    #[test]
    fn pools_inbound_into_the_engine_and_fans_the_rebroadcast_to_the_peer() {
        let raw = hx(RAW_ANNOUNCE);
        let source = iface(0xA1);
        let peer = iface(0xB2);
        let arrival = InstantMillis(1_000);

        let engine = EngineState::<Cap>::default();
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
            |_event| {},
        );

        assert_eq!(out.ingested_packet_count, 1);
        assert_eq!(out.accepted_announce_count, 1);
        assert_eq!(out.scheduled_rebroadcast_count, 1);
        assert_eq!(out.egress_directive_count, 1);

        assert!(runtime.interfaces()[0].handle.sent.is_empty());
        assert_eq!(runtime.interfaces()[1].handle.sent.len(), 1);

        let submitted = &runtime.interfaces()[1].handle.sent[0];
        let (original_header, original_payload) = WirePacketHeader::parse(&raw).unwrap();
        let (submitted_header, submitted_payload) = WirePacketHeader::parse(submitted).unwrap();
        assert_eq!(submitted_header.packet_type, PacketType::Announce);
        assert_eq!(submitted_header.hops, original_header.hops + 1);
        assert_eq!(submitted_header.destination, original_header.destination);
        assert_eq!(submitted_payload, original_payload);

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
            |_event| {},
        );

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
            |_event| {},
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
            |_event| {},
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
            |buf: &mut [u8]| {
                buf[..bytes.len()].copy_from_slice(&bytes);
                bytes.len()
            },
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
        let write_packet = |buf: &mut [u8]| {
            buf[..bytes.len()].copy_from_slice(&bytes);
            bytes.len()
        };

        fan_to_handles(
            &mut interfaces,
            &mut traffic,
            write_packet,
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
            write_packet,
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
