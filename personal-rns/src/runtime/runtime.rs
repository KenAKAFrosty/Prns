use heapless::Vec as HeaplessVec;

use super::core::TrafficLedger;
use super::event::PrnsEvent;
use super::host::{CycleStamp, Host, NextWake};
use super::snapshot::{InterfaceView, RuntimeSnapshot};
use crate::engine::proof::IMPLICIT_PROOF_WIRE_LEN;
use crate::engine::{
    AnnounceIngest, AnnounceNowFailure, AnnounceTarget, CommandId, CommandOutcome,
    EngineCycleEntropy, EngineCycleEntropySeed, EngineState, IngestPacketOutcome, InstantMillis,
    IssuedCommand, NextScheduledEngineWork, ProofOwed, RatchetEntropy, SendSingle,
    SendSingleEntropy, SendSingleFailure, Settlement,
};
use crate::interfaces::storage::InterfaceSet;
use crate::interfaces::{
    ConnectionState, InterfaceId, NextScheduledInterfaceWake, RegisteredInterface, SendError,
};
use crate::routing::announce::SelfAnnounceEntropy;
use crate::routing::storage::EngineStorage;
use crate::wire::MTU;

#[derive(Debug)]
pub struct RuntimeStepOutput {
    pub ingested_packet_count: usize,
    pub processed_command_count: usize,
    pub accepted_announce_count: usize,
    pub scheduled_rebroadcast_count: usize,
    pub delivered_count: usize,
    pub emitted_proof_count: usize,
    pub egress_directive_count: usize,
    pub next_poll: NextScheduledInterfaceWake,
}

#[derive(Clone, Copy)]
enum FanoutClass {
    SelfOriginated,
    Transported,
}

enum InboundStep {
    LaneDry,
    NothingOwed,
    OwesProof(ProofOwed),
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
        mut next_command: impl FnMut() -> Option<IssuedCommand>,
    ) -> RuntimeStepOutput {
        cycle_pooled(
            &mut self.engine,
            &mut self.interfaces,
            &mut self.traffic,
            now,
            entropy_seed,
            &mut on_event,
            &mut next_command,
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

    pub async fn run<OnEvent, NextCommand>(
        self,
        mut on_event: OnEvent,
        mut next_command: NextCommand,
    ) -> !
    where
        Ho: Host,
        OnEvent: FnMut(PrnsEvent<'_>),
        NextCommand: FnMut() -> Option<IssuedCommand>,
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
                &mut next_command,
            );

            //Future improvement: Only fire this when actually updated (diffd for changes) instead of every cycle
            on_event(PrnsEvent::SnapshotUpdated(&snapshot_pooled(
                &engine,
                &interfaces,
                &traffic,
            )));
            wake = if output.processed_command_count > 0 {
                NextWake::Immediate
            } else {
                host_wake(engine.next_wakeup(now), output.next_poll)
            };
        }
    }
}

#[allow(clippy::expect_used)]
fn cycle_pooled<I, S, OnEvent, NextCommand>(
    engine: &mut EngineState<S>,
    interfaces: &mut I,
    traffic: &mut TrafficLedger,
    now: InstantMillis,
    entropy_seed: EngineCycleEntropySeed,
    on_event: &mut OnEvent,
    next_command: &mut NextCommand,
) -> RuntimeStepOutput
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
    OnEvent: FnMut(PrnsEvent<'_>),
    NextCommand: FnMut() -> Option<IssuedCommand>,
{
    let EngineCycleEntropy {
        jitter,
        self_announce,
        ratchet,
        send,
    } = EngineCycleEntropy::from_seed(entropy_seed);

    let mut next_poll = NextScheduledInterfaceWake::Idle;
    for started in interfaces.iter_mut() {
        next_poll = next_poll.sooner(started.poll(now));
    }

    let mut processed_command_count = 0;
    let announce_entropy = match next_command() {
        None => Some((self_announce, ratchet)),
        Some(command) => {
            processed_command_count += 1;
            run_command(
                engine,
                interfaces,
                traffic,
                command,
                now,
                (self_announce, ratchet),
                send,
                on_event,
            )
        }
    };

    let mut ingested_packet_count = 0;
    let mut accepted_announce_count = 0;
    let mut scheduled_rebroadcast_count = 0;
    let mut delivered_count = 0;
    let mut emitted_proof_count = 0;
    for interface in interfaces.iter_mut() {
        let descriptor = interface.descriptor();
        let id = descriptor.id;
        let answers_proofs = matches!(
            descriptor.state,
            ConnectionState::Connected | ConnectionState::Degraded
        ) && descriptor.capabilities.allows_transmit();

        // One packet per step: between steps the interface is free again, so a
        // delivery's owed proof is answered on the lane it arrived through
        // before the next packet is taken. This happens per-packet, in arrival order,
        // mirroring the reference's own receive-then-prove rhythm.
        loop {
            let step = match interface.next_inbound(|packet| {
                traffic.add_rx(id, packet.bytes.len() as u64);
                ingested_packet_count += 1;
                match engine.ingest_packet(packet, jitter) {
                    IngestPacketOutcome::Announce(AnnounceIngest::Accepted) => {
                        accepted_announce_count += 1;
                        scheduled_rebroadcast_count += 1;
                        None
                    }
                    IngestPacketOutcome::Announce(
                        AnnounceIngest::HeldForRetry | AnnounceIngest::Ignored,
                    ) => None,
                    IngestPacketOutcome::Delivery {
                        delivery,
                        maybe_owed_proof,
                    } => {
                        delivered_count += 1;
                        on_event(PrnsEvent::Delivered(delivery));
                        maybe_owed_proof
                    }
                    IngestPacketOutcome::Ignored => None,
                }
            }) {
                None => InboundStep::LaneDry,
                Some(None) => InboundStep::NothingOwed,
                Some(Some(owed)) => InboundStep::OwesProof(owed),
            };

            match step {
                InboundStep::LaneDry => break,
                InboundStep::NothingOwed => {}
                InboundStep::OwesProof(_) if !answers_proofs => {}
                InboundStep::OwesProof(owed) => {
                    let mut proof = [0u8; IMPLICIT_PROOF_WIRE_LEN];
                    if let Ok(n) = engine.write_proof(&owed, &mut proof) {
                        let sent = interface.send_with(|buf| {
                            buf[..n].copy_from_slice(&proof[..n]);
                            n
                        });
                        if sent.is_ok() {
                            traffic.add_tx(id, n as u64);
                            emitted_proof_count += 1;
                        }
                    }
                }
            }
        }
    }

    let tick_output = engine.tick(now, jitter);
    let egress_directive_count = tick_output.egress_directive_count();
    for directive in tick_output.egress_directives() {
        // Serializing per interface (instead of once into a staging buffer) is
        // free (`to_wire` is a header write plus a copy of the retained,
        // already-signed payload) and it lets each packet land in its queue
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
            FanoutClass::Transported,
        );
    }
    tick_output.commit();

    if !engine.registered_interfaces().is_empty() {
        if let Some((nonce, ratchet_entropy)) = announce_entropy {
            // The self-announce is staged once — building it signs, and signing
            // per interface would be waste — then the fan copies the staged bytes
            // into each grant.
            let mut emit_buffer = [0u8; MTU];
            if let Ok(Some(n)) =
                engine.write_due_self_announce(now, nonce, ratchet_entropy, &mut emit_buffer)
            {
                fan_to_handles(
                    interfaces,
                    traffic,
                    |buf| {
                        buf[..n].copy_from_slice(&emit_buffer[..n]);
                        n
                    },
                    engine.registered_interfaces(),
                    FanoutClass::SelfOriginated,
                );
            }
        }
    }

    for started in interfaces.iter_mut() {
        started.drain_control_reports();
    }

    RuntimeStepOutput {
        ingested_packet_count,
        processed_command_count,
        accepted_announce_count,
        scheduled_rebroadcast_count,
        delivered_count,
        emitted_proof_count,
        egress_directive_count,
        next_poll,
    }
}

/// Run one app command to completion, spending the cycle's announce entropy if
/// the command mints an announce. Whatever is handed back unspent is the
/// scheduled announce step's to use — entropy moves, so the cycle can never
/// mint twice from it.
#[allow(clippy::too_many_arguments)]
fn run_command<I, S, OnEvent>(
    engine: &mut EngineState<S>,
    interfaces: &mut I,
    traffic: &mut TrafficLedger,
    issued: IssuedCommand,
    now: InstantMillis,
    announce_entropy: (SelfAnnounceEntropy, RatchetEntropy),
    send_entropy: SendSingleEntropy,
    on_event: &mut OnEvent,
) -> Option<(SelfAnnounceEntropy, RatchetEntropy)>
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
    OnEvent: FnMut(PrnsEvent<'_>),
{
    let (id, commanded) = match engine.ingest_command(issued) {
        CommandOutcome::OwesAnnounce { id, announce } => (id, announce),
        CommandOutcome::AnnounceRejected { id, error } => {
            on_event(PrnsEvent::CommandSettled {
                id,
                settlement: Settlement::AnnounceNow(Err(AnnounceNowFailure::Rejected(error))),
            });
            return Some(announce_entropy);
        }
        CommandOutcome::OwesSendSingle { id, send } => {
            run_send_single(
                engine,
                interfaces,
                traffic,
                id,
                &send,
                now,
                send_entropy,
                on_event,
            );
            return Some(announce_entropy);
        }
        CommandOutcome::SendSingleRejected { id, error } => {
            on_event(PrnsEvent::CommandSettled {
                id,
                settlement: Settlement::SendSingle(Err(SendSingleFailure::Rejected(error))),
            });
            return Some(announce_entropy);
        }
    };

    let (nonce, ratchet_entropy) = announce_entropy;
    let mut emit_buffer = [0u8; MTU];
    let settlement = match engine.write_commanded_announce(
        &commanded,
        now,
        nonce,
        ratchet_entropy,
        &mut emit_buffer,
    ) {
        Ok(n) => {
            let fire_on = match &commanded.target {
                AnnounceTarget::AllInterfaces => engine.registered_interfaces(),
                AnnounceTarget::Interface(interface) => core::slice::from_ref(interface),
            };
            fan_to_handles(
                interfaces,
                traffic,
                |buf| {
                    buf[..n].copy_from_slice(&emit_buffer[..n]);
                    n
                },
                fire_on,
                FanoutClass::SelfOriginated,
            );
            Settlement::AnnounceNow(Ok(()))
        }
        Err(error) => Settlement::AnnounceNow(Err(AnnounceNowFailure::WriteFailed(error))),
    };
    on_event(PrnsEvent::CommandSettled { id, settlement });
    None
}

/// A written send fires on the interface its destination's announce arrived on and
/// settles later by proof, timeout, or cull. The only settlements emitted
/// here are immediate failures and the culled command a full table evicted.
#[allow(clippy::too_many_arguments)]
fn run_send_single<I, S, OnEvent>(
    engine: &mut EngineState<S>,
    interfaces: &mut I,
    traffic: &mut TrafficLedger,
    id: CommandId,
    send_single: &SendSingle,
    now: InstantMillis,
    entropy: SendSingleEntropy,
    on_event: &mut OnEvent,
) where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
    OnEvent: FnMut(PrnsEvent<'_>),
{
    let mut emit_buffer = [0u8; MTU];
    match engine.write_commanded_send_single(id, send_single, now, entropy, &mut emit_buffer) {
        Ok(dispatch) => {
            let n = dispatch.wire_len;
            fan_to_handles(
                interfaces,
                traffic,
                |buf| {
                    buf[..n].copy_from_slice(&emit_buffer[..n]);
                    n
                },
                core::slice::from_ref(&dispatch.fire_on),
                FanoutClass::SelfOriginated,
            );
            if let Some(culled) = dispatch.culled {
                on_event(PrnsEvent::CommandSettled {
                    id: culled.command_id,
                    settlement: Settlement::SendSingle(Err(SendSingleFailure::Culled)),
                });
            }
        }
        Err(error) => on_event(PrnsEvent::CommandSettled {
            id,
            settlement: Settlement::SendSingle(Err(SendSingleFailure::WriteFailed(error))),
        }),
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
                FanoutClass::SelfOriginated => descriptor.capabilities.allows_transmit(),
                FanoutClass::Transported => descriptor.capabilities.allows_transport(),
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

    use crate::engine::{
        AnnounceAppData, AnnounceNow, AnnounceNowError, AnnounceNowFailure, AnnounceTarget,
        CommandId, EngineCommand, IssuedCommand, RatchetPolicy, Settlement,
        ENGINE_CYCLE_ENTROPY_LEN,
    };
    use crate::interfaces::storage::FixedInterfaceSet;
    use crate::interfaces::{
        ConnectionState, ControlReport, DriverMode, EgressCapability, InboundPacket,
        IngressCapability, InterfaceCapabilities, InterfaceDescriptor, InterfaceHandle,
        InterfaceMode, MediumKind, StartedInterface, TransportCapability,
    };
    use crate::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::routing::delivery::Delivery;
    use crate::routing::storage::FixedInline;
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::wire::{DestinationHash, PacketType, WirePacketHeader};

    type Cap = FixedInline<64, 64, 4096, 4, 512, 64, 8, 8, 8, 128, 8, 8>;

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
        fn next_inbound<R>(&mut self, f: impl FnOnce(InboundPacket<'_>) -> R) -> Option<R> {
            if self.inbound.is_empty() {
                return None;
            }
            let (arrived_at, mut bytes) = self.inbound.remove(0);
            Some(f(InboundPacket {
                arrived_at,
                source_interface: self.id,
                bytes: &mut bytes,
            }))
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
            EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
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
                PrnsEvent::CommandSettled { id, settlement } => {
                    panic!("no command was queued, yet one settled: {id:?} {settlement:?}")
                }
            },
            || None,
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

        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
        let mut engine = EngineState::<Cap>::new(secret);
        let destination = engine
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
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
                PrnsEvent::CommandSettled { id, settlement } => {
                    panic!("no command was queued, yet one settled: {id:?} {settlement:?}")
                }
            },
            || None,
        );

        assert_eq!(out.delivered_count, 1);
        assert_eq!(out.emitted_proof_count, 0);
        assert_eq!(
            delivered,
            std::vec![(destination, b"hello-through-the-stack".to_vec())],
        );
        assert_eq!(
            runtime.interfaces()[0].handle.sent,
            std::vec::Vec::<std::vec::Vec<u8>>::new(),
        );
    }

    fn single_destination_engine() -> (EngineState<Cap>, DestinationHash) {
        use crate::identity::Zeroizing;

        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        let mut engine = EngineState::<Cap>::new(Zeroizing::new(secret));
        let node = engine.transport_identity().unwrap();
        let destination = engine
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        (engine, destination)
    }

    fn announce_now_command(destination: DestinationHash, target: AnnounceTarget) -> IssuedCommand {
        IssuedCommand {
            id: CommandId(7),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target,
                app_data: AnnounceAppData::Scheduled,
            }),
        }
    }

    fn no_failed_settlements(event: PrnsEvent<'_>) {
        if let PrnsEvent::CommandSettled {
            id,
            settlement: Settlement::AnnounceNow(Err(failure)),
        } = event
        {
            panic!("command {id:?} failed: {failure:?}");
        }
    }

    fn parsed_announce_destination(raw: &[u8]) -> DestinationHash {
        let (header, _) = WirePacketHeader::parse(raw).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        header.destination
    }

    #[test]
    fn a_commanded_announce_fans_only_to_its_targeted_interface() {
        let (engine, destination) = single_destination_engine();
        let target = iface(0xB2);
        let mut runtime = Runtime::new(
            engine,
            interface_set([
                started(iface(0xA1), std::vec::Vec::new()),
                started(target, std::vec::Vec::new()),
            ]),
            (),
        );

        let mut queue = VecDeque::from([announce_now_command(
            destination,
            AnnounceTarget::Interface(target),
        )]);
        let mut settled = std::vec::Vec::new();
        let out = runtime.cycle_once(
            InstantMillis(1_000),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
            |event| {
                if let PrnsEvent::CommandSettled { id, settlement } = event {
                    settled.push((id, settlement));
                }
            },
            || queue.pop_front(),
        );

        assert_eq!(out.processed_command_count, 1);
        assert_eq!(
            settled,
            std::vec![(CommandId(7), Settlement::AnnounceNow(Ok(())))],
        );
        assert_eq!(runtime.interfaces()[0].handle.sent.len(), 0);
        let sent = &runtime.interfaces()[1].handle.sent;
        assert_eq!(sent.len(), 1);
        assert_eq!(parsed_announce_destination(&sent[0]), destination);
    }

    #[test]
    fn a_commanded_announce_to_all_interfaces_fans_everywhere() {
        let (engine, destination) = single_destination_engine();
        let mut runtime = Runtime::new(
            engine,
            interface_set([
                started(iface(0xA1), std::vec::Vec::new()),
                started(iface(0xB2), std::vec::Vec::new()),
            ]),
            (),
        );

        let mut queue = VecDeque::from([announce_now_command(
            destination,
            AnnounceTarget::AllInterfaces,
        )]);
        runtime.cycle_once(
            InstantMillis(1_000),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
            no_failed_settlements,
            || queue.pop_front(),
        );

        let first = &runtime.interfaces()[0].handle.sent;
        let second = &runtime.interfaces()[1].handle.sent;
        assert_eq!(first.len(), 1);
        assert_eq!(first, second);
        assert_eq!(parsed_announce_destination(&first[0]), destination);
    }

    #[test]
    fn a_rejected_command_surfaces_on_the_event_lane() {
        let (engine, _) = single_destination_engine();
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(iface(0xA1), std::vec::Vec::new())]),
            (),
        );

        let mut queue = VecDeque::from([announce_now_command(
            DestinationHash::new([0x99; 16]),
            AnnounceTarget::AllInterfaces,
        )]);
        let mut settled = std::vec::Vec::new();
        let out = runtime.cycle_once(
            InstantMillis(1_000),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
            |event| {
                if let PrnsEvent::CommandSettled { id, settlement } = event {
                    settled.push((id, settlement));
                }
            },
            || queue.pop_front(),
        );

        assert_eq!(out.processed_command_count, 1);
        assert_eq!(
            settled,
            std::vec![(
                CommandId(7),
                Settlement::AnnounceNow(Err(AnnounceNowFailure::Rejected(
                    AnnounceNowError::UnknownDestination
                ))),
            )],
        );
        assert_eq!(runtime.interfaces()[0].handle.sent.len(), 0);
    }

    #[test]
    fn each_queued_command_takes_its_own_cycle_with_fresh_entropy() {
        use crate::routing::announce::Announce;

        let (engine, destination) = single_destination_engine();
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(iface(0xA1), std::vec::Vec::new())]),
            (),
        );
        let issued_as = |id| IssuedCommand {
            id,
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Scheduled,
            }),
        };
        let mut queue = VecDeque::from([issued_as(CommandId(1)), issued_as(CommandId(2))]);

        let mut settled = std::vec::Vec::new();
        let mut collect = |event: PrnsEvent<'_>| {
            if let PrnsEvent::CommandSettled { id, settlement } = event {
                settled.push((id, settlement));
            }
        };
        let first = runtime.cycle_once(
            InstantMillis(1_000),
            EngineCycleEntropySeed::new([0xAA; ENGINE_CYCLE_ENTROPY_LEN]),
            &mut collect,
            || queue.pop_front(),
        );
        let second = runtime.cycle_once(
            InstantMillis(2_000),
            EngineCycleEntropySeed::new([0xBB; ENGINE_CYCLE_ENTROPY_LEN]),
            &mut collect,
            || queue.pop_front(),
        );

        assert_eq!(first.processed_command_count, 1);
        assert_eq!(second.processed_command_count, 1);
        assert_eq!(
            settled,
            std::vec![
                (CommandId(1), Settlement::AnnounceNow(Ok(()))),
                (CommandId(2), Settlement::AnnounceNow(Ok(()))),
            ],
        );
        let sent = &runtime.interfaces()[0].handle.sent;
        assert_eq!(sent.len(), 2);
        let announce_id_of = |raw: &[u8]| {
            let (header, payload) = WirePacketHeader::parse(raw).unwrap();
            Announce::from_wire(&header, payload).unwrap().announce_id
        };
        assert_ne!(announce_id_of(&sent[0]), announce_id_of(&sent[1]));
    }

    #[test]
    fn a_rejected_send_settles_on_the_event_lane() {
        use crate::engine::{SendSingle, SendSingleError, SendSingleFailure, SendSinglePayload};
        use crate::identity::Zeroizing;

        let mut secret = [0u8; 64];
        secret[..32].fill(0x55);
        secret[32..].fill(0x66);
        let engine = EngineState::<Cap>::new(Zeroizing::new(secret));
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(iface(0xA1), std::vec::Vec::new())]),
            (),
        );

        let mut queue = VecDeque::from([IssuedCommand {
            id: CommandId(7),
            command: EngineCommand::SendSingle(SendSingle {
                destination: DestinationHash::new([0x99; 16]),
                payload: SendSinglePayload::from_slice(b"into-the-void").unwrap(),
            }),
        }]);
        let mut settled = std::vec::Vec::new();
        let out = runtime.cycle_once(
            InstantMillis(1_000),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
            |event| {
                if let PrnsEvent::CommandSettled { id, settlement } = event {
                    settled.push((id, settlement));
                }
            },
            || queue.pop_front(),
        );

        assert_eq!(out.processed_command_count, 1);
        assert_eq!(
            settled,
            std::vec![(
                CommandId(7),
                Settlement::SendSingle(Err(SendSingleFailure::Rejected(
                    SendSingleError::NoRouteToDestination
                ))),
            )],
        );
        assert_eq!(runtime.interfaces()[0].handle.sent.len(), 0);
    }

    #[test]
    fn a_written_send_fires_on_the_announce_interface_and_does_not_settle_yet() {
        use crate::engine::test_support::{hx, RATCHETED_SELF_ANNOUNCE_RNS_WIRE};
        use crate::engine::{SendSingle, SendSinglePayload};
        use crate::identity::Zeroizing;

        let mut secret = [0u8; 64];
        secret[..32].fill(0x55);
        secret[32..].fill(0x66);
        let engine = EngineState::<Cap>::new(Zeroizing::new(secret));
        let source = iface(0xA1);
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(
                source,
                std::vec![(InstantMillis(500), hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE))],
            )]),
            (),
        );

        let destination =
            DestinationHash::new(hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap());
        let mut queue = VecDeque::new();
        let mut settled = std::vec::Vec::new();
        let mut collect = |event: PrnsEvent<'_>| {
            if let PrnsEvent::CommandSettled { id, settlement } = event {
                settled.push((id, settlement));
            }
        };

        let first = runtime.cycle_once(
            InstantMillis(600),
            EngineCycleEntropySeed::new([0xAA; ENGINE_CYCLE_ENTROPY_LEN]),
            &mut collect,
            || queue.pop_front(),
        );
        assert_eq!(first.accepted_announce_count, 1);

        queue.push_back(IssuedCommand {
            id: CommandId(7),
            command: EngineCommand::SendSingle(SendSingle {
                destination,
                payload: SendSinglePayload::from_slice(b"over-the-wire").unwrap(),
            }),
        });
        let second = runtime.cycle_once(
            InstantMillis(1_000),
            EngineCycleEntropySeed::new([0xBB; ENGINE_CYCLE_ENTROPY_LEN]),
            &mut collect,
            || queue.pop_front(),
        );

        assert_eq!(second.processed_command_count, 1);
        assert_eq!(
            settled,
            std::vec![],
            "a written send settles by proof or timeout, never in its own cycle"
        );
        let sent = &runtime.interfaces()[0].handle.sent;
        assert_eq!(sent.len(), 1);
        let (header, _) = WirePacketHeader::parse(&sent[0]).unwrap();
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(header.destination, destination);
    }

    #[test]
    fn a_command_defers_the_due_scheduled_announce_one_cycle() {
        use crate::engine::self_announce::AnnounceConfig;
        use crate::engine::ReannounceSchedule;

        let (mut engine, scheduled) = single_destination_engine();
        engine
            .schedule_announce(
                &scheduled,
                AnnounceConfig {
                    app_data: b"hello-personal",
                    schedule: ReannounceSchedule::default(),
                },
            )
            .unwrap();
        let node = engine.transport_identity().unwrap();
        let commanded = engine
            .register_single_destination(
                &node,
                "personal",
                &["other"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(iface(0xA1), std::vec::Vec::new())]),
            (),
        );

        let mut queue = VecDeque::from([announce_now_command(
            commanded,
            AnnounceTarget::AllInterfaces,
        )]);
        runtime.cycle_once(
            InstantMillis(1_000),
            EngineCycleEntropySeed::new([0xAA; ENGINE_CYCLE_ENTROPY_LEN]),
            no_failed_settlements,
            || queue.pop_front(),
        );
        runtime.cycle_once(
            InstantMillis(2_000),
            EngineCycleEntropySeed::new([0xBB; ENGINE_CYCLE_ENTROPY_LEN]),
            no_failed_settlements,
            || queue.pop_front(),
        );

        let sent = &runtime.interfaces()[0].handle.sent;
        assert_eq!(sent.len(), 2);
        assert_eq!(parsed_announce_destination(&sent[0]), commanded);
        assert_eq!(parsed_announce_destination(&sent[1]), scheduled);
    }

    #[test]
    fn a_prove_all_delivery_answers_with_the_proof_on_the_arriving_interface() {
        use crate::crypto::X25519SecretKey;
        use crate::engine::proof::IMPLICIT_PROOF_WIRE_LEN;
        use crate::identity::in_memory::InMemoryNodeIdentity;
        use crate::identity::{IdentitySigner, RemoteIdentity, Zeroizing};
        use crate::routing::dedup::PacketHash;
        use crate::wire::{
            ContextFlag, DestinationType, IfacFlag, PropagationType, WireContext, MTU,
        };

        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        let secret = Zeroizing::new(secret);

        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
        let mut engine = EngineState::<Cap>::new(secret);
        let destination = engine
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
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
                b"prove-through-the-stack",
                &mut wire[header_len..],
            )
            .unwrap();
        let raw = wire[..header_len + sealed].to_vec();
        let packet_hash = PacketHash::of_wire_packet(&raw).unwrap();

        let source = iface(0xA1);
        let arrival = InstantMillis(1_000);
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(source, std::vec![(arrival, raw)])]),
            (),
        );

        let out = runtime.cycle_once(
            InstantMillis(arrival.0 + 1),
            EngineCycleEntropySeed::new([0xCA; ENGINE_CYCLE_ENTROPY_LEN]),
            |_event| {},
            || None,
        );
        assert_eq!(out.delivered_count, 1);
        assert_eq!(out.emitted_proof_count, 1);

        let mut expected = std::vec::Vec::new();
        expected.push(0x03);
        expected.push(0x00);
        expected.extend_from_slice(packet_hash.proof_destination().as_bytes());
        expected.push(0x00);
        expected.extend_from_slice(&identity.sign(packet_hash.as_bytes()).0);
        assert_eq!(expected.len(), IMPLICIT_PROOF_WIRE_LEN);
        assert_eq!(runtime.interfaces()[0].handle.sent, std::vec![expected]);
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
            || None,
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
            || None,
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
            || None,
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
            || None,
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
            FanoutClass::Transported,
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
    fn fanout_distinguishes_transmit_only_from_transport_interfaces() {
        let transmit_only = iface(0x11);
        let transport = iface(0x22);
        let mut interfaces = interface_set([
            started_with_descriptor_and_reports(
                descriptor_with_capabilities(
                    transmit_only,
                    ConnectionState::Connected,
                    EgressCapability::Enabled(TransportCapability::NoTransport),
                ),
                std::vec::Vec::new(),
                [],
            ),
            started_with_descriptor_and_reports(
                descriptor_with_capabilities(
                    transport,
                    ConnectionState::Connected,
                    EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
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
            &[transmit_only, transport],
            FanoutClass::Transported,
        );

        assert!(interfaces.as_slice()[0].handle.sent.is_empty());
        assert_eq!(
            interfaces.as_slice()[1].handle.sent,
            std::vec![bytes.to_vec()]
        );
        assert_eq!(traffic.totals_for(transmit_only), (0, 0));
        assert_eq!(traffic.totals_for(transport), (0, bytes.len() as u64));

        fan_to_handles(
            &mut interfaces,
            &mut traffic,
            write_packet,
            &[transmit_only, transport],
            FanoutClass::SelfOriginated,
        );

        assert_eq!(
            interfaces.as_slice()[0].handle.sent,
            std::vec![bytes.to_vec()]
        );
        assert_eq!(
            interfaces.as_slice()[1].handle.sent,
            std::vec![bytes.to_vec(), bytes.to_vec()]
        );
        assert_eq!(traffic.totals_for(transmit_only), (0, bytes.len() as u64));
        assert_eq!(traffic.totals_for(transport), (0, 2 * bytes.len() as u64));
    }
}
