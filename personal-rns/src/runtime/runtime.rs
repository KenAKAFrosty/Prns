use heapless::Vec as HeaplessVec;

use super::core::TrafficLedger;
use super::entropy::UnspentEntropyPool;
use super::event::PrnsEvent;
use super::host::{Host, NextWake};
use super::snapshot::{InterfaceView, RuntimeSnapshot};
use crate::engine::proof::IMPLICIT_PROOF_WIRE_LEN;
use crate::engine::{
    AnnounceIngest, AnnounceNowFailure, AnnounceTarget, CommandId, CommandOutcome,
    CommandedAnnounceWriteOutcome, DueSelfAnnounceWriteOutcome, EngineState, IngestPacketOutcome,
    InstantMillis, IssuedCommand, NextScheduledEngineWork, PathFound, PathRequestWriteOutcome,
    PathResponseWriteOutcome, ProofIngest, ProofOwed, RatchetEntropy, RebroadcastDecision,
    RequestPath, RequestPathFailure, SendSingle, SendSingleEntropy, SendSingleFailure,
    SendSingleWriteOutcome, Settlement, WriteSendSingleError,
};
use crate::interfaces::storage::InterfaceSet;
use crate::interfaces::{
    ConnectionState, InterfaceDescriptor, InterfaceId, NextScheduledInterfaceWake,
    RegisteredInterface, SendError, MAX_REGISTERED_INTERFACES,
};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::SelfAnnounceEntropy;
use crate::routing::storage::EngineStorage;
use crate::wire::{DestinationHash, MTU};

#[derive(Debug)]
pub struct RuntimeStepOutput {
    pub ingested_packet_count: usize,
    pub processed_command_count: usize,
    pub accepted_announce_count: usize,
    pub scheduled_rebroadcast_count: usize,
    pub delivered_count: usize,
    pub emitted_proof_count: usize,
    pub forwarded_count: usize,
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
    ForwardsPacket {
        wire_len: usize,
        fire_on: InterfaceId,
    },
    AnswersPathRequest {
        destination: DestinationHash,
    },
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
    pool: UnspentEntropyPool,
    host: Ho,
}

impl<Ho, I, S> Runtime<Ho, I, S>
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
{
    pub fn new(engine: EngineState<S>, interfaces: I, host: Ho) -> Self {
        Self {
            engine,
            interfaces,
            traffic: TrafficLedger::new(),
            pool: UnspentEntropyPool::empty(),
            host,
        }
    }

    pub fn cycle_once(
        &mut self,
        now: InstantMillis,
        mut fill_entropy: impl FnMut(&mut [u8]),
        mut on_event: impl FnMut(PrnsEvent<'_>),
        mut next_command: impl FnMut() -> Option<IssuedCommand>,
    ) -> RuntimeStepOutput {
        cycle_pooled(
            &mut self.engine,
            &mut self.interfaces,
            &mut self.traffic,
            &mut self.pool,
            now,
            &mut fill_entropy,
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
            mut pool,
            mut host,
        } = self;
        let mut wake = NextWake::Immediate;
        loop {
            let now = host.wait(wake).await;
            let output = cycle_pooled(
                &mut engine,
                &mut interfaces,
                &mut traffic,
                &mut pool,
                now,
                &mut |bytes: &mut [u8]| host.fill_entropy(bytes),
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
#[allow(clippy::too_many_arguments)]
fn cycle_pooled<I, S, OnEvent, NextCommand, FillEntropy>(
    engine: &mut EngineState<S>,
    interfaces: &mut I,
    traffic: &mut TrafficLedger,
    pool: &mut UnspentEntropyPool,
    now: InstantMillis,
    fill_entropy: &mut FillEntropy,
    on_event: &mut OnEvent,
    next_command: &mut NextCommand,
) -> RuntimeStepOutput
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
    OnEvent: FnMut(PrnsEvent<'_>),
    NextCommand: FnMut() -> Option<IssuedCommand>,
    FillEntropy: FnMut(&mut [u8]),
{
    let mut jitter_bytes = [0u8; core::mem::size_of::<u64>()];
    fill_entropy(&mut jitter_bytes);
    let jitter = JitterSeed(u64::from_le_bytes(jitter_bytes));
    let self_announce = pool.checkout_self_announce(&mut *fill_entropy);
    let ratchet = pool.checkout_ratchet(&mut *fill_entropy);
    let send = pool.checkout_send_single(&mut *fill_entropy);

    let mut next_poll = NextScheduledInterfaceWake::Idle;
    for started in interfaces.iter_mut() {
        next_poll = next_poll.sooner(started.poll(now));
    }

    let mut interface_view: HeaplessVec<InterfaceDescriptor, MAX_REGISTERED_INTERFACES> =
        HeaplessVec::new();
    for started in interfaces.iter() {
        let _ = interface_view.push(*started.descriptor());
    }

    let mut processed_command_count = 0;
    let unspent = match next_command() {
        None => UnspentCycleEntropy {
            self_announce: Some(self_announce),
            ratchet: Some(ratchet),
            send_single: Some(send),
        },
        Some(command) => {
            processed_command_count += 1;
            run_command(
                engine,
                interfaces,
                &interface_view,
                traffic,
                command,
                now,
                self_announce,
                ratchet,
                send,
                on_event,
            )
        }
    };
    let UnspentCycleEntropy {
        self_announce: mut unspent_self_announce_entropy,
        ratchet: mut unspent_ratchet_entropy,
        send_single: unspent_send_single_entropy,
    } = unspent;
    if let Some(survivor) = unspent_send_single_entropy {
        pool.restore_send_single(survivor);
    }

    let mut ingested_packet_count = 0;
    let mut accepted_announce_count = 0;
    let mut scheduled_rebroadcast_count = 0;
    let mut delivered_count = 0;
    let mut emitted_proof_count = 0;
    let mut forwarded_count = 0;
    let mut forward_buffer = [0u8; MTU];
    for index in 0..interfaces.len() {
        let descriptor = interfaces.as_mut_slice()[index].descriptor();
        let id = descriptor.id;
        let answers_proofs = matches!(
            descriptor.state,
            ConnectionState::Connected | ConnectionState::Degraded
        ) && descriptor.capabilities.allows_transmit();

        // One packet per step: between steps every interface is free again, so a
        // delivery's owed proof is answered on the lane it arrived through (and
        // a packet in transport fans onward, wherever its path leads) before the
        // next packet is taken. This happens per-packet, in arrival order,
        // mirroring the reference's own receive-then-act rhythm.
        loop {
            let interface = &mut interfaces.as_mut_slice()[index];
            let step = match interface.next_inbound(|packet| {
                traffic.add_rx(id, packet.bytes.len() as u64);
                ingested_packet_count += 1;
                match engine.ingest_packet(packet, jitter, &interface_view) {
                    IngestPacketOutcome::Announce(AnnounceIngest::Accepted(accepted)) => {
                        accepted_announce_count += 1;
                        if accepted.rebroadcast == RebroadcastDecision::Scheduled {
                            scheduled_rebroadcast_count += 1;
                        }
                        on_event(PrnsEvent::AnnounceHeard {
                            destination: accepted.destination,
                            hops: accepted.hops,
                            source_interface: id,
                        });
                        // A learned route answers every path request waiting on
                        // this destination — each settles found, at this hop count.
                        while let Some(settled) =
                            engine.pop_settled_path_request(&accepted.destination)
                        {
                            on_event(PrnsEvent::CommandSettled {
                                id: settled.command_id,
                                settlement: Settlement::RequestPath(Ok(PathFound {
                                    hops: accepted.hops,
                                })),
                            });
                        }
                        InboundStep::NothingOwed
                    }
                    IngestPacketOutcome::Announce(
                        AnnounceIngest::HeldForRetry | AnnounceIngest::Ignored,
                    ) => InboundStep::NothingOwed,
                    IngestPacketOutcome::Delivery {
                        delivery,
                        maybe_owed_proof,
                    } => {
                        delivered_count += 1;
                        on_event(PrnsEvent::Delivered(delivery));
                        match maybe_owed_proof {
                            Some(owed) => InboundStep::OwesProof(owed),
                            None => InboundStep::NothingOwed,
                        }
                    }
                    IngestPacketOutcome::Proof(ProofIngest::SendSingleDelivered {
                        id,
                        delivered,
                    }) => {
                        on_event(PrnsEvent::CommandSettled {
                            id,
                            settlement: Settlement::SendSingle(Ok(delivered)),
                        });
                        InboundStep::NothingOwed
                    }
                    IngestPacketOutcome::Proof(ProofIngest::Ignored) => InboundStep::NothingOwed,
                    IngestPacketOutcome::Forward(forward) => {
                        match forward.to_wire(&mut forward_buffer) {
                            Ok(wire_len) => InboundStep::ForwardsPacket {
                                wire_len,
                                fire_on: forward.fire_on,
                            },
                            Err(_) => InboundStep::NothingOwed,
                        }
                    }
                    IngestPacketOutcome::AnswerPathRequest { destination } => {
                        InboundStep::AnswersPathRequest { destination }
                    }
                    IngestPacketOutcome::Ignored => InboundStep::NothingOwed,
                }
            }) {
                None => InboundStep::LaneDry,
                Some(step) => step,
            };

            match step {
                InboundStep::LaneDry => break,
                InboundStep::NothingOwed => {}
                InboundStep::OwesProof(_) if !answers_proofs => {}
                InboundStep::OwesProof(owed) => {
                    let mut proof = [0u8; IMPLICIT_PROOF_WIRE_LEN];
                    if let Ok(n) = engine.write_proof(&owed, &mut proof) {
                        let interface = &mut interfaces.as_mut_slice()[index];
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
                InboundStep::ForwardsPacket { wire_len, fire_on } => {
                    fan_to_handles(
                        interfaces,
                        traffic,
                        |buf| {
                            buf[..wire_len].copy_from_slice(&forward_buffer[..wire_len]);
                            wire_len
                        },
                        FanTargets::Listed(core::slice::from_ref(&fire_on)),
                        FanoutClass::Transported,
                    );
                    forwarded_count += 1;
                }
                InboundStep::AnswersPathRequest { destination } => {
                    let self_announce = pool.checkout_self_announce(&mut *fill_entropy);
                    let mut response = [0u8; MTU];
                    if let PathResponseWriteOutcome::Written { wire_len } = engine
                        .write_path_response_announce(
                            &destination,
                            now,
                            self_announce,
                            &mut response,
                        )
                    {
                        fan_to_handles(
                            interfaces,
                            traffic,
                            |buf| {
                                buf[..wire_len].copy_from_slice(&response[..wire_len]);
                                wire_len
                            },
                            FanTargets::Listed(core::slice::from_ref(&id)),
                            FanoutClass::SelfOriginated,
                        );
                    }
                }
            }
        }
    }

    while let Some(expired) = engine.pop_timed_out_send_single(now) {
        on_event(PrnsEvent::CommandSettled {
            id: expired.command_id,
            settlement: Settlement::SendSingle(Err(SendSingleFailure::Timeout)),
        });
    }

    while let Some(expired) = engine.pop_timed_out_path_request(now) {
        on_event(PrnsEvent::CommandSettled {
            id: expired.command_id,
            settlement: Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
        });
    }

    let tick_output = engine.tick(now, jitter, &interface_view);
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
            FanTargets::Listed(directive.fire_on()),
            FanoutClass::Transported,
        );
    }
    tick_output.commit();

    if !interface_view.is_empty() {
        if let (Some(self_announce_entropy), Some(ratchet_entropy)) = (
            unspent_self_announce_entropy.take(),
            unspent_ratchet_entropy.take(),
        ) {
            use DueSelfAnnounceWriteOutcome::{Failed, NothingDue, Rejected, Written};

            // The self-announce is staged once — building it signs, and signing
            // per interface would be waste — then the fan copies the staged bytes
            // into each grant.
            let mut emit_buffer = [0u8; MTU];
            match engine.write_due_self_announce(
                now,
                self_announce_entropy,
                ratchet_entropy,
                &mut emit_buffer,
            ) {
                Written { len: n, rotation } => {
                    unspent_ratchet_entropy = rotation.into_unspent();
                    fan_to_handles(
                        interfaces,
                        traffic,
                        |buf| {
                            buf[..n].copy_from_slice(&emit_buffer[..n]);
                            n
                        },
                        FanTargets::EveryInterface,
                        FanoutClass::SelfOriginated,
                    );
                }
                NothingDue {
                    unspent_self_announce: self_announce_home,
                    unspent_ratchet: ratchet_home,
                } => {
                    unspent_self_announce_entropy = Some(self_announce_home);
                    unspent_ratchet_entropy = Some(ratchet_home);
                }
                Rejected {
                    rejection: _,
                    unspent_self_announce: self_announce_home,
                    unspent_ratchet: ratchet_home,
                } => {
                    unspent_self_announce_entropy = Some(self_announce_home);
                    unspent_ratchet_entropy = Some(ratchet_home);
                }
                Failed {
                    failure: _,
                    rotation,
                } => {
                    unspent_ratchet_entropy = rotation.into_unspent();
                }
            }
        }
    }
    if let Some(survivor) = unspent_self_announce_entropy {
        pool.restore_self_announce(survivor);
    }
    if let Some(survivor) = unspent_ratchet_entropy {
        pool.restore_ratchet(survivor);
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
        forwarded_count,
        egress_directive_count,
        next_poll,
    }
}

/// Run one app command to completion, spending the cycle's announce entropy if
/// the command mints an announce and the send entropy if it seals a send.
/// Whatever is handed back unspent is the rest of the cycle's to use — entropy
/// moves, so the cycle can never mint twice from it.
#[must_use]
struct UnspentCycleEntropy {
    self_announce: Option<SelfAnnounceEntropy>,
    ratchet: Option<RatchetEntropy>,
    send_single: Option<SendSingleEntropy>,
}

#[allow(clippy::too_many_arguments)]
fn run_command<I, S, OnEvent>(
    engine: &mut EngineState<S>,
    interfaces: &mut I,
    interface_view: &[InterfaceDescriptor],
    traffic: &mut TrafficLedger,
    issued: IssuedCommand,
    now: InstantMillis,
    self_announce_entropy: SelfAnnounceEntropy,
    ratchet: RatchetEntropy,
    send_single_entropy: SendSingleEntropy,
    on_event: &mut OnEvent,
) -> UnspentCycleEntropy
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
    OnEvent: FnMut(PrnsEvent<'_>),
{
    let (id, commanded) = match engine.ingest_command(issued, interface_view) {
        CommandOutcome::OwesAnnounce { id, announce } => (id, announce),
        CommandOutcome::AnnounceRejected { id, error } => {
            on_event(PrnsEvent::CommandSettled {
                id,
                settlement: Settlement::AnnounceNow(Err(AnnounceNowFailure::Rejected(error))),
            });
            return UnspentCycleEntropy {
                self_announce: Some(self_announce_entropy),
                ratchet: Some(ratchet),
                send_single: Some(send_single_entropy),
            };
        }
        CommandOutcome::OwesSendSingle { id, send } => {
            let unspent_send = run_send_single(
                engine,
                interfaces,
                traffic,
                id,
                &send,
                now,
                send_single_entropy,
                on_event,
            );
            return UnspentCycleEntropy {
                self_announce: Some(self_announce_entropy),
                ratchet: Some(ratchet),
                send_single: unspent_send,
            };
        }
        CommandOutcome::SendSingleRejected { id, error } => {
            on_event(PrnsEvent::CommandSettled {
                id,
                settlement: Settlement::SendSingle(Err(SendSingleFailure::Rejected(error))),
            });
            return UnspentCycleEntropy {
                self_announce: Some(self_announce_entropy),
                ratchet: Some(ratchet),
                send_single: Some(send_single_entropy),
            };
        }
        CommandOutcome::OwesPathRequest { id, request } => {
            run_request_path(engine, interfaces, traffic, id, &request, now, on_event);
            return UnspentCycleEntropy {
                self_announce: Some(self_announce_entropy),
                ratchet: Some(ratchet),
                send_single: Some(send_single_entropy),
            };
        }
    };

    use CommandedAnnounceWriteOutcome::{Failed, Rejected, Written};

    let mut emit_buffer = [0u8; MTU];
    let (settlement, maybe_self_announce_entropy, maybe_ratchet_entropy) = match engine
        .write_commanded_announce(
            &commanded,
            now,
            self_announce_entropy,
            ratchet,
            &mut emit_buffer,
        ) {
        Written { len: n, rotation } => {
            let fire_on = match &commanded.target {
                AnnounceTarget::AllInterfaces => FanTargets::EveryInterface,
                AnnounceTarget::Interface(interface) => {
                    FanTargets::Listed(core::slice::from_ref(interface))
                }
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
            (
                Settlement::AnnounceNow(Ok(())),
                None,
                rotation.into_unspent(),
            )
        }
        Rejected {
            rejection,
            unspent_self_announce,
            unspent_ratchet,
        } => (
            Settlement::AnnounceNow(Err(AnnounceNowFailure::WriteFailed(rejection.into()))),
            Some(unspent_self_announce),
            Some(unspent_ratchet),
        ),
        Failed { failure, rotation } => (
            Settlement::AnnounceNow(Err(AnnounceNowFailure::WriteFailed(failure.into()))),
            None,
            rotation.into_unspent(),
        ),
    };
    on_event(PrnsEvent::CommandSettled { id, settlement });
    UnspentCycleEntropy {
        self_announce: maybe_self_announce_entropy,
        ratchet: maybe_ratchet_entropy,
        send_single: Some(send_single_entropy),
    }
}

/// A written send fires on the interface its destination's announce arrived on and
/// settles later by proof, timeout, or cull. The only settlements emitted
/// here are immediate failures and the culled command a full table evicted.
#[allow(clippy::too_many_arguments)]
fn run_send_single<I, S, OnEvent>(
    engine: &mut EngineState<S>,
    interfaces: &mut I,
    traffic_ledger: &mut TrafficLedger,
    id: CommandId,
    send_single: &SendSingle,
    now: InstantMillis,
    entropy: SendSingleEntropy,
    on_event: &mut OnEvent,
) -> Option<SendSingleEntropy>
where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
    OnEvent: FnMut(PrnsEvent<'_>),
{
    use SendSingleWriteOutcome::{Failed, Rejected, Written};

    let mut emit_buffer = [0u8; MTU];
    match engine.write_commanded_send_single(id, send_single, now, entropy, &mut emit_buffer) {
        Written(dispatch) => {
            let n = dispatch.wire_len;
            fan_to_handles(
                interfaces,
                traffic_ledger,
                |buf| {
                    buf[..n].copy_from_slice(&emit_buffer[..n]);
                    n
                },
                FanTargets::Listed(core::slice::from_ref(&dispatch.fire_on)),
                FanoutClass::SelfOriginated,
            );
            if let Some(culled) = dispatch.culled {
                on_event(PrnsEvent::CommandSettled {
                    id: culled.command_id,
                    settlement: Settlement::SendSingle(Err(SendSingleFailure::Culled)),
                });
            }
            None
        }
        Rejected {
            rejection,
            unspent_entropy,
        } => {
            on_event(PrnsEvent::CommandSettled {
                id,
                settlement: Settlement::SendSingle(Err(SendSingleFailure::WriteFailed(
                    rejection.into(),
                ))),
            });
            Some(unspent_entropy)
        }
        Failed { failure } => {
            on_event(PrnsEvent::CommandSettled {
                id,
                settlement: Settlement::SendSingle(Err(SendSingleFailure::WriteFailed(
                    WriteSendSingleError::Seal(failure),
                ))),
            });
            None
        }
    }
}

/// A path request broadcasts to every interface and then waits: it settles
/// later, when an announce learns the destination's route (found) or its
/// timeout passes. The cases that resolve at emission are handled here — an
/// already-known route (found at once, nothing sent), a full pending table
/// culling an older request, or a serialize failure.
fn run_request_path<I, S, OnEvent>(
    engine: &mut EngineState<S>,
    interfaces: &mut I,
    traffic_ledger: &mut TrafficLedger,
    id: CommandId,
    request: &RequestPath,
    now: InstantMillis,
    on_event: &mut OnEvent,
) where
    I: InterfaceSet,
    I::Item: RegisteredInterface,
    S: EngineStorage,
    OnEvent: FnMut(PrnsEvent<'_>),
{
    let mut emit_buffer = [0u8; MTU];
    match engine.write_commanded_path_request(id, request, now, &mut emit_buffer) {
        PathRequestWriteOutcome::AlreadyReachable { hops } => {
            on_event(PrnsEvent::CommandSettled {
                id,
                settlement: Settlement::RequestPath(Ok(PathFound { hops })),
            });
        }
        PathRequestWriteOutcome::Written {
            wire_len: n,
            culled,
        } => {
            fan_to_handles(
                interfaces,
                traffic_ledger,
                |buf| {
                    buf[..n].copy_from_slice(&emit_buffer[..n]);
                    n
                },
                FanTargets::EveryInterface,
                FanoutClass::SelfOriginated,
            );
            if let Some(culled) = culled {
                on_event(PrnsEvent::CommandSettled {
                    id: culled.command_id,
                    settlement: Settlement::RequestPath(Err(RequestPathFailure::Culled)),
                });
            }
        }
        PathRequestWriteOutcome::SerializeFailed(error) => {
            on_event(PrnsEvent::CommandSettled {
                id,
                settlement: Settlement::RequestPath(Err(RequestPathFailure::WriteFailed(error))),
            });
        }
    }
}

enum FanTargets<'a> {
    EveryInterface,
    Listed(&'a [InterfaceId]),
}

impl FanTargets<'_> {
    fn includes(&self, id: InterfaceId) -> bool {
        match self {
            Self::EveryInterface => true,
            Self::Listed(ids) => ids.contains(&id),
        }
    }
}

fn fan_to_handles<I>(
    interfaces: &mut I,
    traffic_ledger: &mut TrafficLedger,
    write_packet: impl Fn(&mut [u8]) -> usize,
    fire_on: FanTargets<'_>,
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
        let eligible = fire_on.includes(id)
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
                Ok(written) => traffic_ledger.add_tx(id, written as u64),
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
    use crate::engine::test_support::{transporting_view, TEST_TRANSPORT_ID};
    use std::collections::VecDeque;

    use crate::engine::{
        write_path_request_wire_packet, AnnounceAppData, AnnounceNow, AnnounceNowError,
        AnnounceNowFailure, AnnounceTarget, CommandId, EngineCommand, IssuedCommand, PathFound,
        PathRequestId, RatchetPolicy, RequestPath, RequestPathFailure, Settlement,
        PATH_REQUEST_DESTINATION,
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
    use crate::wire::{
        DestinationHash, DestinationType, PacketType, PropagationType, WireContext,
        WirePacketHeader, HEADER_MAX_LEN,
    };

    type Cap = FixedInline<64, 64, 4096, 4, 512, 64, 8, 8, 8, 128, 8, 8, 8, 8>;

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
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |event| match event {
                PrnsEvent::Delivered(Delivery::Plain(delivery)) => delivered.push((
                    delivery.destination,
                    delivery.payload.to_vec(),
                    delivery.source_interface,
                )),
                PrnsEvent::Delivered(Delivery::Single(_)) => {
                    panic!("a plain packet must never surface as a single delivery")
                }
                PrnsEvent::SnapshotUpdated(_) | PrnsEvent::AnnounceHeard { .. } => {}
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |event| match event {
                PrnsEvent::Delivered(Delivery::Single(delivery)) => {
                    delivered.push((delivery.destination, delivery.plaintext.to_vec()))
                }
                PrnsEvent::Delivered(Delivery::Plain(_)) => {
                    panic!("a sealed single packet must never surface as plain")
                }
                PrnsEvent::SnapshotUpdated(_) | PrnsEvent::AnnounceHeard { .. } => {}
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
        let node = engine.held_identity_hashes()[0];
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
            no_failed_settlements,
            || queue.pop_front(),
        );

        let first = &runtime.interfaces()[0].handle.sent;
        let second = &runtime.interfaces()[1].handle.sent;
        assert_eq!(first.len(), 1);
        assert_eq!(first, second);
        assert_eq!(parsed_announce_destination(&first[0]), destination);
    }

    fn request_path_command(destination: DestinationHash) -> IssuedCommand {
        IssuedCommand {
            id: CommandId(7),
            command: EngineCommand::RequestPath(RequestPath {
                destination,
                id: PathRequestId::new([0x55; 16]),
            }),
        }
    }

    #[test]
    fn a_request_path_with_no_route_fans_everywhere_and_waits() {
        let (engine, _) = single_destination_engine();
        let mut runtime = Runtime::new(
            engine,
            interface_set([
                started(iface(0xA1), std::vec::Vec::new()),
                started(iface(0xB2), std::vec::Vec::new()),
            ]),
            (),
        );

        let wanted = DestinationHash::new([0x44; 16]);
        let mut queue = VecDeque::from([request_path_command(wanted)]);
        let mut settled = std::vec::Vec::new();
        let out = runtime.cycle_once(
            InstantMillis(1_000),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |event| {
                if let PrnsEvent::CommandSettled { id, settlement } = event {
                    settled.push((id, settlement));
                }
            },
            || queue.pop_front(),
        );

        assert_eq!(out.processed_command_count, 1);
        assert!(
            settled.is_empty(),
            "with no route yet, the request waits — it does not settle on emission",
        );

        let first = &runtime.interfaces()[0].handle.sent;
        let second = &runtime.interfaces()[1].handle.sent;
        assert_eq!(first.len(), 1);
        assert_eq!(
            first, second,
            "a path request broadcasts identically on every interface",
        );

        let (header, payload) = WirePacketHeader::parse(&first[0]).unwrap();
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(header.destination_type, DestinationType::Plain);
        assert_eq!(header.propagation, PropagationType::Broadcast);
        assert_eq!(header.destination, PATH_REQUEST_DESTINATION);
        assert_eq!(&payload[..16], wanted.as_bytes());
    }

    #[test]
    fn a_request_path_settles_found_when_its_route_arrives() {
        let raw = hx(RAW_ANNOUNCE);
        let wanted = parsed_announce_destination(&raw);
        let mut runtime = Runtime::new(
            EngineState::<Cap>::default(),
            interface_set([started(iface(0xA1), std::vec![(InstantMillis(900), raw)])]),
            (),
        );

        // The command drains before the inbound loop, so within one cycle the
        // request goes out unanswered, then the announce arrives and answers it.
        let mut queue = VecDeque::from([request_path_command(wanted)]);
        let mut settled = std::vec::Vec::new();
        runtime.cycle_once(
            InstantMillis(1_000),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |event| {
                if let PrnsEvent::CommandSettled { id, settlement } = event {
                    settled.push((id, settlement));
                }
            },
            || queue.pop_front(),
        );

        assert_eq!(
            settled,
            std::vec![(
                CommandId(7),
                Settlement::RequestPath(Ok(PathFound { hops: 1 })),
            )],
            "the learned route settles the request, found one hop away",
        );
    }

    #[test]
    fn a_request_path_settles_timeout_when_no_route_arrives() {
        let (engine, _) = single_destination_engine();
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(iface(0xA1), std::vec::Vec::new())]),
            (),
        );

        let mut queue = VecDeque::from([request_path_command(DestinationHash::new([0x44; 16]))]);
        let mut settled = std::vec::Vec::new();
        runtime.cycle_once(
            InstantMillis(1_000),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |event| {
                if let PrnsEvent::CommandSettled { id, settlement } = event {
                    settled.push((id, settlement));
                }
            },
            || queue.pop_front(),
        );
        assert!(settled.is_empty());

        runtime.cycle_once(
            InstantMillis(1_000 + crate::engine::PATH_REQUEST_TIMEOUT_MS),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |event| {
                if let PrnsEvent::CommandSettled { id, settlement } = event {
                    settled.push((id, settlement));
                }
            },
            || None,
        );

        assert_eq!(
            settled,
            std::vec![(
                CommandId(7),
                Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
            )],
        );
    }

    #[test]
    fn a_request_path_for_a_known_route_settles_found_at_once_without_emitting() {
        let raw = hx(RAW_ANNOUNCE);
        let wanted = parsed_announce_destination(&raw);
        let mut runtime = Runtime::new(
            EngineState::<Cap>::default(),
            interface_set([started(iface(0xA1), std::vec![(InstantMillis(900), raw)])]),
            (),
        );

        // Cycle one only hears the announce — a lone cross-interface-only pipe
        // with no transport id sends nothing back.
        runtime.cycle_once(
            InstantMillis(1_000),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |_| {},
            || None,
        );
        assert!(runtime.interfaces()[0].handle.sent.is_empty());

        let mut queue = VecDeque::from([request_path_command(wanted)]);
        let mut settled = std::vec::Vec::new();
        runtime.cycle_once(
            InstantMillis(2_000),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |event| {
                if let PrnsEvent::CommandSettled { id, settlement } = event {
                    settled.push((id, settlement));
                }
            },
            || queue.pop_front(),
        );

        assert_eq!(
            settled,
            std::vec![(
                CommandId(7),
                Settlement::RequestPath(Ok(PathFound { hops: 1 })),
            )],
            "the route is already known — found at once",
        );
        assert!(
            runtime.interfaces()[0].handle.sent.is_empty(),
            "nothing to ask for, so no path request leaves",
        );
    }

    #[test]
    fn a_path_request_for_a_local_destination_is_answered_on_its_interface() {
        let (engine, local) = single_destination_engine();
        let mut request = [0u8; 500];
        let n = write_path_request_wire_packet(local, &[0x55; 16], &mut request).unwrap();
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(
                iface(0xA1),
                std::vec![(InstantMillis(900), request[..n].to_vec())],
            )]),
            (),
        );

        runtime.cycle_once(
            InstantMillis(1_000),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |_| {},
            || None,
        );

        let sent = &runtime.interfaces()[0].handle.sent;
        assert_eq!(
            sent.len(),
            1,
            "the request is answered with exactly one packet"
        );
        let (header, _) = WirePacketHeader::parse(&sent[0]).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.context, WireContext::PathResponse);
        assert_eq!(header.destination, local);
    }

    #[test]
    fn an_announce_heard_on_a_repeating_interface_fires_back_out_that_interface() {
        let raw = hx(RAW_ANNOUNCE);
        let mut runtime = Runtime::new(
            {
                let mut engine: EngineState<Cap> = EngineState::<Cap>::default();
                engine.set_transport_id(TEST_TRANSPORT_ID);
                engine
            },
            interface_set([started_with_descriptor_and_reports(
                descriptor_with_capabilities(
                    iface(0xA1),
                    ConnectionState::Connected,
                    EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
                ),
                std::vec![(InstantMillis(500), raw.clone())],
                [],
            )]),
            (),
        );

        let out = runtime.cycle_once(
            InstantMillis(1_000),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |_| {},
            || None,
        );
        assert_eq!(out.accepted_announce_count, 1);
        assert_eq!(out.scheduled_rebroadcast_count, 1);

        let _ = runtime.cycle_once(
            InstantMillis(500 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |_| {},
            || None,
        );
        let (original_header, original_payload) = WirePacketHeader::parse(&raw).unwrap();
        let stamped_header = WirePacketHeader {
            transport_id: Some(TEST_TRANSPORT_ID),
            propagation: PropagationType::Transport,
            hops: original_header.hops + 1,
            ..original_header
        };
        let mut expected = std::vec![0u8; HEADER_MAX_LEN + original_payload.len()];
        let header_len = stamped_header.write(&mut expected).unwrap();
        expected[header_len..].copy_from_slice(original_payload);
        assert_eq!(
            runtime.interfaces()[0].handle.sent,
            std::vec![expected],
            "the announce must re-emit on its arrival interface, one hop further, stamped with our transport id, exactly once",
        );
    }

    #[test]
    fn a_lone_cross_interface_only_interface_never_repeats_what_it_heard() {
        let raw = hx(RAW_ANNOUNCE);
        let mut runtime = Runtime::new(
            {
                let mut engine: EngineState<Cap> = EngineState::<Cap>::default();
                engine.set_transport_id(TEST_TRANSPORT_ID);
                engine
            },
            interface_set([started_with_descriptor_and_reports(
                descriptor_with_capabilities(
                    iface(0xA1),
                    ConnectionState::Connected,
                    EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
                ),
                std::vec![(InstantMillis(500), raw)],
                [],
            )]),
            (),
        );

        let out = runtime.cycle_once(
            InstantMillis(1_000),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |_| {},
            || None,
        );
        assert_eq!(out.accepted_announce_count, 1);

        for now in [
            500 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1,
            500 + 2 * DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1,
        ] {
            let _ = runtime.cycle_once(
                InstantMillis(now),
                |bytes: &mut [u8]| bytes.fill(0xCA),
                |_| {},
                || None,
            );
        }
        assert!(
            runtime.interfaces()[0].handle.sent.is_empty(),
            "a point-to-point pipe must never replay an announce at its only peer",
        );
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
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
            |bytes: &mut [u8]| bytes.fill(0xAA),
            &mut collect,
            || queue.pop_front(),
        );
        let second = runtime.cycle_once(
            InstantMillis(2_000),
            |bytes: &mut [u8]| bytes.fill(0xBB),
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
    fn an_idle_cycles_announce_entropy_is_pooled_and_seals_the_next_due_announce() {
        use crate::engine::test_support::personal_node_announcer;
        use crate::engine::ReannounceSchedule;

        let interval = ReannounceSchedule::default().interval_millis();
        let mut runtime = Runtime::new(
            personal_node_announcer(),
            interface_set([started(iface(0xA1), std::vec::Vec::new())]),
            (),
        );

        let _boot = runtime.cycle_once(
            InstantMillis(600),
            |bytes: &mut [u8]| bytes.fill(0xAA),
            |_| {},
            || None,
        );
        let _idle = runtime.cycle_once(
            InstantMillis(700),
            |bytes: &mut [u8]| bytes.fill(0xBB),
            |_| {},
            || None,
        );
        let _due = runtime.cycle_once(
            InstantMillis(600 + interval),
            |bytes: &mut [u8]| bytes.fill(0xCC),
            |_| {},
            || None,
        );

        let sent = &runtime.interfaces()[0].handle.sent;
        assert_eq!(sent.len(), 2);

        let mut twin = personal_node_announcer();
        let mut boot_expected = [0u8; MTU];
        let boot_len = twin
            .write_due_self_announce(
                InstantMillis(600),
                SelfAnnounceEntropy::new([0xAA; SelfAnnounceEntropy::LEN]),
                RatchetEntropy::new([0xAA; RatchetEntropy::LEN]),
                &mut boot_expected,
            )
            .written_len();
        assert_eq!(&sent[0][..], &boot_expected[..boot_len]);

        let mut due_expected = [0u8; MTU];
        let due_len = twin
            .write_due_self_announce(
                InstantMillis(600 + interval),
                SelfAnnounceEntropy::new([0xBB; SelfAnnounceEntropy::LEN]),
                RatchetEntropy::new([0xBB; RatchetEntropy::LEN]),
                &mut due_expected,
            )
            .written_len();
        assert_eq!(
            &sent[1][..],
            &due_expected[..due_len],
            "the idle cycle's pooled units seal the next due announce, not the fresh seed",
        );
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
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
            |bytes: &mut [u8]| bytes.fill(0xAA),
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
            |bytes: &mut [u8]| bytes.fill(0xBB),
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
    fn a_rejected_send_restores_its_entropy_and_the_pooled_unit_seals_byte_identically() {
        use crate::engine::test_support::{
            personal_node_announcer, second_secret_key, TEST_ENTROPY, TEST_RATCHET_ENTROPY,
            TEST_SELF_ANNOUNCE_ENTROPY,
        };
        use crate::engine::{SendSingle, SendSinglePayload};
        use SendSingleWriteOutcome::Written;

        fn send_entropy_of(key: u8, iv: u8) -> SendSingleEntropy {
            let mut bytes = [key; SendSingleEntropy::LEN];
            bytes[32..].fill(iv);
            SendSingleEntropy::new(bytes)
        }

        fn hearer_with_route(announce_wire: &[u8]) -> EngineState<Cap> {
            let mut state = EngineState::new(second_secret_key());
            let mut raw = announce_wire.to_vec();
            let _ = state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xA1),
                    bytes: &mut raw,
                },
                TEST_ENTROPY,
                &transporting_view(),
            );
            state
        }

        let mut announcer = personal_node_announcer();
        let mut announce_buf = [0u8; MTU];
        let announce_len = announcer
            .write_due_self_announce(
                InstantMillis(100),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();
        let destination = announcer.self_announced_destinations()[0];
        let send = SendSingle {
            destination,
            payload: SendSinglePayload::from_slice(b"pooled-retry").unwrap(),
        };

        let mut twin = hearer_with_route(&announce_buf[..announce_len]);
        let mut expected_buf = [0u8; MTU];
        let expected_len = match twin.write_commanded_send_single(
            CommandId(1),
            &send,
            InstantMillis(1_000),
            send_entropy_of(0x33, 0x44),
            &mut expected_buf,
        ) {
            Written(dispatch) => dispatch.wire_len,
            _ => panic!("the routed twin writes the expected wire"),
        };

        let mut no_route: EngineState<Cap> = EngineState::new(second_secret_key());
        let mut interfaces = interface_set(Option::<TestInterface>::None);
        let mut traffic = TrafficLedger::new();
        let mut settled = std::vec::Vec::new();
        let came_home = run_send_single(
            &mut no_route,
            &mut interfaces,
            &mut traffic,
            CommandId(2),
            &send,
            InstantMillis(900),
            send_entropy_of(0x33, 0x44),
            &mut |event| {
                if let PrnsEvent::CommandSettled { id, settlement } = event {
                    settled.push((id, settlement));
                }
            },
        );
        assert_eq!(
            settled,
            std::vec![(
                CommandId(2),
                Settlement::SendSingle(Err(SendSingleFailure::WriteFailed(
                    WriteSendSingleError::RouteVanished
                ))),
            )],
        );

        let mut pool = UnspentEntropyPool::empty();
        pool.restore_send_single(came_home.expect("a rejected send hands its entropy back up"));
        let pooled = pool.checkout_send_single(|bytes: &mut [u8]| {
            bytes[..32].fill(0xDE);
            bytes[32..].fill(0xAD);
        });
        let mut routed = hearer_with_route(&announce_buf[..announce_len]);
        let mut reused_buf = [0u8; MTU];
        let reused_len = match routed.write_commanded_send_single(
            CommandId(3),
            &send,
            InstantMillis(1_000),
            pooled,
            &mut reused_buf,
        ) {
            Written(dispatch) => dispatch.wire_len,
            _ => panic!("the pooled unit seals on the routed state"),
        };
        assert_eq!(
            &reused_buf[..reused_len],
            &expected_buf[..expected_len],
            "the unit that came home from the rejected send seals byte-identical wire",
        );
    }

    #[test]
    fn an_arriving_proof_settles_the_send_through_the_event_lane() {
        use crate::engine::test_support::{
            hx, personal_node_announcer, RAW_SEALED_FOR_PROOF, RNS_1_3_1_IMPLICIT_PROOF,
            TEST_RATCHET_ENTROPY, TEST_SELF_ANNOUNCE_ENTROPY,
        };
        use crate::engine::{Delivered, SendSingle, SendSinglePayload};
        use crate::identity::Zeroizing;

        let mut announcer = personal_node_announcer();
        let mut announce_buf = [0u8; MTU];
        let announce_len = announcer
            .write_due_self_announce(
                InstantMillis(100),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();

        let mut secret = [0u8; 64];
        secret[..32].fill(0x55);
        secret[32..].fill(0x66);
        let mut engine = EngineState::<Cap>::new(Zeroizing::new(secret));
        let mut interfaces = interface_set([started(
            iface(0xA1),
            std::vec![(InstantMillis(500), announce_buf[..announce_len].to_vec())],
        )]);
        let mut traffic = TrafficLedger::new();

        let destination =
            DestinationHash::new(hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap());
        let mut queue = VecDeque::new();
        let mut pool = UnspentEntropyPool::empty();
        let mut settled = std::vec::Vec::new();
        let mut collect = |event: PrnsEvent<'_>| {
            if let PrnsEvent::CommandSettled { id, settlement } = event {
                settled.push((id, settlement));
            }
        };

        let mut send_vector_fill = |bytes: &mut [u8]| {
            if bytes.len() == SendSingleEntropy::LEN {
                bytes[..32].fill(0x33);
                bytes[32..].fill(0x44);
            } else {
                bytes.fill(0xAA);
            }
        };
        let first = cycle_pooled(
            &mut engine,
            &mut interfaces,
            &mut traffic,
            &mut pool,
            InstantMillis(600),
            &mut send_vector_fill,
            &mut collect,
            &mut || queue.pop_front(),
        );
        assert_eq!(first.accepted_announce_count, 1);

        queue.push_back(IssuedCommand {
            id: CommandId(7),
            command: EngineCommand::SendSingle(SendSingle {
                destination,
                payload: SendSinglePayload::from_slice(b"proof-parity").unwrap(),
            }),
        });
        let second = cycle_pooled(
            &mut engine,
            &mut interfaces,
            &mut traffic,
            &mut pool,
            InstantMillis(1_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xBB),
            &mut collect,
            &mut || queue.pop_front(),
        );
        assert_eq!(second.processed_command_count, 1);
        assert_eq!(interfaces.as_slice()[0].handle.sent.len(), 1);
        assert_eq!(
            interfaces.as_slice()[0].handle.sent[0],
            hx(RAW_SEALED_FOR_PROOF),
        );

        for interface in interfaces.iter_mut() {
            interface
                .handle
                .inbound
                .push((InstantMillis(1_250), hx(RNS_1_3_1_IMPLICIT_PROOF)));
        }
        let third = cycle_pooled(
            &mut engine,
            &mut interfaces,
            &mut traffic,
            &mut pool,
            InstantMillis(1_300),
            &mut |bytes: &mut [u8]| bytes.fill(0xCC),
            &mut collect,
            &mut || queue.pop_front(),
        );
        assert_eq!(third.ingested_packet_count, 1);

        assert_eq!(
            settled,
            std::vec![(
                CommandId(7),
                Settlement::SendSingle(Ok(Delivered { rtt_ms: 250 })),
            )],
            "the RNS 1.3.1 proof settles exactly the sent command, with its measured rtt",
        );
    }

    #[test]
    fn an_unproven_send_settles_timeout_through_the_event_lane() {
        use crate::engine::test_support::{hx, RATCHETED_SELF_ANNOUNCE_RNS_WIRE};
        use crate::engine::{SendSingle, SendSinglePayload};
        use crate::identity::Zeroizing;

        let mut secret = [0u8; 64];
        secret[..32].fill(0x55);
        secret[32..].fill(0x66);
        let engine = EngineState::<Cap>::new(Zeroizing::new(secret));
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(
                iface(0xA1),
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
            |bytes: &mut [u8]| bytes.fill(0xAA),
            &mut collect,
            || queue.pop_front(),
        );
        assert_eq!(first.accepted_announce_count, 1);

        queue.push_back(IssuedCommand {
            id: CommandId(9),
            command: EngineCommand::SendSingle(SendSingle {
                destination,
                payload: SendSinglePayload::from_slice(b"into-silence").unwrap(),
            }),
        });
        runtime.cycle_once(
            InstantMillis(1_000),
            |bytes: &mut [u8]| bytes.fill(0xBB),
            &mut collect,
            || queue.pop_front(),
        );

        runtime.cycle_once(
            InstantMillis(13_000),
            |bytes: &mut [u8]| bytes.fill(0xCC),
            &mut collect,
            || queue.pop_front(),
        );

        assert_eq!(
            settled,
            std::vec![(
                CommandId(9),
                Settlement::SendSingle(Err(SendSingleFailure::Timeout)),
            )],
            "no proof by timeout_at settles the send as Timeout, exactly once",
        );
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
        let node = engine.held_identity_hashes()[0];
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
            |bytes: &mut [u8]| bytes.fill(0xAA),
            no_failed_settlements,
            || queue.pop_front(),
        );
        runtime.cycle_once(
            InstantMillis(2_000),
            |bytes: &mut [u8]| bytes.fill(0xBB),
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
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
    fn an_accepted_announce_surfaces_as_announce_heard() {
        let raw = hx(RAW_ANNOUNCE);
        let source = iface(0xA1);
        let engine = EngineState::<Cap>::default();
        let mut runtime = Runtime::new(
            engine,
            interface_set([started(
                source,
                std::vec![(InstantMillis(1_000), raw.clone())],
            )]),
            (),
        );

        let mut heard = std::vec::Vec::new();
        let out = runtime.cycle_once(
            InstantMillis(1_100),
            |bytes: &mut [u8]| bytes.fill(0xCA),
            |event| {
                if let PrnsEvent::AnnounceHeard {
                    destination,
                    hops,
                    source_interface,
                } = event
                {
                    heard.push((destination, hops, source_interface));
                }
            },
            || None,
        );

        assert_eq!(out.accepted_announce_count, 1);
        assert_eq!(
            heard,
            std::vec![(
                DestinationHash::new(hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap()),
                1,
                source,
            )],
            "discovery: the accepted announce names its destination, hop count, and arrival lane",
        );
    }

    #[test]
    fn pools_inbound_into_the_engine_and_fans_the_rebroadcast_to_the_peer() {
        let raw = hx(RAW_ANNOUNCE);
        let source = iface(0xA1);
        let peer = iface(0xB2);
        let arrival = InstantMillis(1_000);

        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
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

        let mut engine = EngineState::<GrowableHeap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
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
            |bytes: &mut [u8]| bytes.fill(0xCA),
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
            FanTargets::Listed(&[connected, failed]),
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
            FanTargets::Listed(&[transmit_only, transport]),
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
            FanTargets::Listed(&[transmit_only, transport]),
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
