use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::super::{Host, NextWake};
use crate::engine::InstantMillis;
use crate::interfaces::substrate::{TokioHostSubstrate, TokioInterfaceHandle, TokioInterfaceSeam};
use crate::interfaces::{Interface, InterfaceId, StartedInterface};

/// Upper bound on one wait, so a far-off deadline or an idle engine still
/// loops back periodically. A daemon host is mains-powered; the cap is free.
const MAX_WAIT: Duration = Duration::from_secs(1);

pub struct TokioHost {
    wake: Arc<Notify>,
    clock_base: Instant,
}

impl TokioHost {
    pub fn new() -> Self {
        Self {
            wake: Arc::new(Notify::new()),
            clock_base: Instant::now(),
        }
    }

    pub fn wake_handle(&self) -> TokioWakeHandle {
        TokioWakeHandle(self.wake.clone())
    }

    pub fn glue_seam<const MTU: usize>(
        &self,
        id: InterfaceId,
        max_buffered_packets: usize,
    ) -> TokioInterfaceSeam<MTU> {
        TokioInterfaceSeam::new(id, self.clock_base, max_buffered_packets, self.wake.clone())
    }

    pub fn attach<I, const MTU: usize>(
        &self,
        interface: I,
        max_buffered_packets: usize,
    ) -> StartedInterface<TokioInterfaceHandle<MTU>, I::Worker>
    where
        I: Interface<TokioHostSubstrate<MTU>>,
    {
        let id = interface.descriptor().id;
        self.glue_seam(id, max_buffered_packets)
            .start_interface(interface)
    }

    fn now(&self) -> InstantMillis {
        InstantMillis(self.clock_base.elapsed().as_millis() as u64)
    }
}

impl Default for TokioHost {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct TokioWakeHandle(Arc<Notify>);

impl TokioWakeHandle {
    pub fn poke(&self) {
        self.0.notify_one();
    }
}

fn wait_for(next: NextWake, now: InstantMillis, max: Duration) -> Duration {
    match next {
        NextWake::Immediate => Duration::ZERO,
        NextWake::Idle => max,
        NextWake::At(deadline) => Duration::from_millis(deadline.0.saturating_sub(now.0)).min(max),
    }
}

impl Host for TokioHost {
    async fn wait(&mut self, wake: NextWake) -> InstantMillis {
        // Suspend until the engine's next deadline or an interface pokes the
        // wake — whichever first — yielding the executor to the worker tasks
        // meanwhile. The wake is coalesced (`Notify` holds one permit); the
        // runtime drains every interface's ring each cycle regardless, so a
        // missed poke only costs the next deadline, never a packet.
        let timeout = wait_for(wake, self.now(), MAX_WAIT);
        if !timeout.is_zero() {
            let _ = tokio::time::timeout(timeout, self.wake.notified()).await;
        }
        self.now()
    }

    #[allow(clippy::expect_used)]
    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        getrandom::getrandom(bytes).expect("OS CSPRNG must provide cycle entropy");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::InboundSink;

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use tokio::sync::oneshot;

    use crate::engine::{CommandId, IssuedCommand, Settleable, Settlement};

    struct Commander {
        next_id: AtomicU64,
        pending: Mutex<HashMap<CommandId, oneshot::Sender<Settlement>>>,
        command_tx: tokio::sync::mpsc::UnboundedSender<IssuedCommand>,
        wake: TokioWakeHandle,
    }

    impl Commander {
        async fn issue<C: Settleable>(&self, command: C) -> Result<C::Success, C::Failure> {
            let id = CommandId(self.next_id.fetch_add(1, Ordering::Relaxed));
            let (settle_tx, settle_rx) = oneshot::channel();
            self.pending.lock().unwrap().insert(id, settle_tx);
            self.command_tx
                .send(IssuedCommand {
                    id,
                    command: command.into_command(),
                })
                .expect("the runtime task holds the receiver");
            self.wake.poke();
            let settlement = settle_rx
                .await
                .expect("the runtime settles every issued command");
            C::from_settlement(settlement).expect("the settlement variant matches the issued verb")
        }

        fn settle(&self, id: CommandId, settlement: Settlement) {
            let waiting = self.pending.lock().unwrap().remove(&id);
            if let Some(waiting) = waiting {
                let _ = waiting.send(settlement);
            }
        }
    }

    #[tokio::test]
    async fn wait_returns_promptly_when_an_interface_pokes_the_wake() {
        let mut host = TokioHost::new();
        let mut seam = host.glue_seam::<8>(InterfaceId::new([0; 16]), 1);

        let poker = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            seam.worker_context
                .inbound
                .submit(|buf| {
                    buf[0] = 1;
                    1
                })
                .unwrap();
        });

        let _stamp = tokio::time::timeout(Duration::from_secs(5), host.wait(NextWake::Idle))
            .await
            .expect("the poke must cut the idle wait short, not run out the 1s cap");
        poker.await.unwrap();
    }

    #[tokio::test]
    async fn an_immediate_wake_does_not_wait() {
        let mut host = TokioHost::new();
        let now = host.wait(NextWake::Immediate).await;
        assert!(now.0 < 100);
    }

    #[tokio::test]
    async fn an_awaited_announce_now_settles_through_a_live_runtime() {
        use crate::engine::test_support::{fixed_secret_key, Cap};
        use crate::engine::{
            AnnounceAppData, AnnounceNow, AnnounceNowError, AnnounceNowFailure, AnnounceTarget,
            EngineState, RatchetPolicy,
        };
        use crate::interfaces::storage::FixedInterfaceSet;
        use crate::interfaces::{ControlReport, InboundPacket, InterfaceHandle, SendError};
        use crate::routing::upstream_app_destinations::ProofStrategy;
        use crate::runtime::{PrnsEvent, Runtime};
        use crate::wire::DestinationHash;

        struct NullHandle;
        impl InterfaceHandle for NullHandle {
            fn next_inbound<R>(&mut self, _f: impl FnOnce(InboundPacket<'_>) -> R) -> Option<R> {
                None
            }
            fn acquire_send_grant(
                &mut self,
                _fill: impl FnOnce(&mut [u8]) -> usize,
            ) -> Result<usize, SendError> {
                Ok(0)
            }
            fn request_stop(&mut self) {}
            fn next_report(&mut self) -> Option<ControlReport> {
                None
            }
        }
        type NullInterface =
            crate::interfaces::StartedInterface<NullHandle, core::convert::Infallible>;

        let mut engine: EngineState<Cap> = EngineState::new(fixed_secret_key());
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

        let host = TokioHost::new();
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        let commander = Arc::new(Commander {
            next_id: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            command_tx,
            wake: host.wake_handle(),
        });

        let runtime = Runtime::new(engine, FixedInterfaceSet::<NullInterface, 1>::new(), host);
        let settler = commander.clone();
        tokio::spawn(async move {
            runtime
                .run(
                    move |event| {
                        if let PrnsEvent::CommandSettled { id, settlement } = event {
                            settler.settle(id, settlement);
                        }
                    },
                    move || command_rx.try_recv().ok(),
                )
                .await
        });

        let accepted = tokio::time::timeout(
            Duration::from_secs(5),
            commander.issue(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Scheduled,
            }),
        )
        .await
        .expect("an issued announce settles well inside the timeout");
        assert_eq!(accepted, Ok(()));

        let rejected = tokio::time::timeout(
            Duration::from_secs(5),
            commander.issue(AnnounceNow {
                destination: DestinationHash::new([0x99; 16]),
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Scheduled,
            }),
        )
        .await
        .expect("a rejected announce settles well inside the timeout");
        assert_eq!(
            rejected,
            Err(AnnounceNowFailure::Rejected(
                AnnounceNowError::UnknownDestination
            )),
        );
    }

    #[tokio::test]
    async fn an_awaited_send_single_settles_delivered_across_two_live_runtimes() {
        use std::sync::mpsc::{channel, Receiver, Sender};

        use crate::engine::self_announce::AnnounceConfig;
        use crate::engine::test_support::{fixed_secret_key, second_secret_key, Cap};
        use crate::engine::{
            Delivered, EngineState, RatchetPolicy, ReannounceSchedule, SendSingle, SendSingleError,
            SendSingleFailure, SendSinglePayload,
        };
        use crate::interfaces::storage::{FixedInterfaceSet, InterfaceSet};
        use crate::interfaces::{
            ConnectionState, ControlReport, DriverMode, EgressCapability, InboundPacket,
            IngressCapability, InterfaceCapabilities, InterfaceDescriptor, InterfaceHandle,
            InterfaceMode, MediumKind, SendError, StartedInterface, TransportCapability,
        };
        use crate::routing::upstream_app_destinations::ProofStrategy;
        use crate::runtime::{PrnsEvent, Runtime};
        use crate::wire::MTU;

        struct ChannelHandle {
            id: InterfaceId,
            clock_base: Instant,
            rx: Receiver<std::vec::Vec<u8>>,
            tx: Sender<std::vec::Vec<u8>>,
            peer_wake: TokioWakeHandle,
        }

        impl InterfaceHandle for ChannelHandle {
            fn next_inbound<R>(&mut self, f: impl FnOnce(InboundPacket<'_>) -> R) -> Option<R> {
                let mut bytes = self.rx.try_recv().ok()?;
                Some(f(InboundPacket {
                    arrived_at: InstantMillis(self.clock_base.elapsed().as_millis() as u64),
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
                let _ = self.tx.send(buf[..written].to_vec());
                self.peer_wake.poke();
                Ok(written)
            }
            fn request_stop(&mut self) {}
            fn next_report(&mut self) -> Option<ControlReport> {
                None
            }
        }

        fn linked_descriptor(id: InterfaceId) -> InterfaceDescriptor {
            InterfaceDescriptor {
                id,
                capabilities: InterfaceCapabilities {
                    ingress: IngressCapability::Enabled,
                    egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
                },
                mode: InterfaceMode::Full,
                medium: MediumKind::Loopback,
                state: ConnectionState::Connected,
            }
        }

        type LinkedInterface = StartedInterface<ChannelHandle, core::convert::Infallible>;

        let mut prover: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = prover.held_identity_hashes()[0];
        let destination = prover
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        prover
            .schedule_announce(
                &destination,
                AnnounceConfig {
                    app_data: b"capstone",
                    schedule: ReannounceSchedule::default(),
                },
            )
            .unwrap();
        let sender: EngineState<Cap> = EngineState::new(second_secret_key());

        let sender_host = TokioHost::new();
        let prover_host = TokioHost::new();
        let (to_prover_tx, to_prover_rx) = channel();
        let (to_sender_tx, to_sender_rx) = channel();

        let mut sender_set = FixedInterfaceSet::<LinkedInterface, 1>::new();
        let _ = sender_set.push(StartedInterface {
            descriptor: linked_descriptor(InterfaceId::new([0xA1; 16])),
            handle: ChannelHandle {
                id: InterfaceId::new([0xA1; 16]),
                clock_base: Instant::now(),
                rx: to_sender_rx,
                tx: to_prover_tx,
                peer_wake: prover_host.wake_handle(),
            },
            drive: DriverMode::SelfDriven,
        });
        let mut prover_set = FixedInterfaceSet::<LinkedInterface, 1>::new();
        let _ = prover_set.push(StartedInterface {
            descriptor: linked_descriptor(InterfaceId::new([0xB2; 16])),
            handle: ChannelHandle {
                id: InterfaceId::new([0xB2; 16]),
                clock_base: Instant::now(),
                rx: to_prover_rx,
                tx: to_sender_tx,
                peer_wake: sender_host.wake_handle(),
            },
            drive: DriverMode::SelfDriven,
        });

        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        let commander = Arc::new(Commander {
            next_id: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            command_tx,
            wake: sender_host.wake_handle(),
        });

        let sender_runtime = Runtime::new(sender, sender_set, sender_host);
        let prover_runtime = Runtime::new(prover, prover_set, prover_host);
        let settler = commander.clone();
        tokio::spawn(async move {
            sender_runtime
                .run(
                    move |event| {
                        if let PrnsEvent::CommandSettled { id, settlement } = event {
                            settler.settle(id, settlement);
                        }
                    },
                    move || command_rx.try_recv().ok(),
                )
                .await
        });
        tokio::spawn(async move { prover_runtime.run(|_event| {}, || None).await });

        let payload = SendSinglePayload::from_slice(b"capstone-delivery").unwrap();
        let delivered = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let settled = commander
                    .issue(SendSingle {
                        destination,
                        payload: payload.clone(),
                    })
                    .await;
                match settled {
                    Err(SendSingleFailure::Rejected(SendSingleError::NoRouteToDestination)) => {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    settled => break settled,
                }
            }
        })
        .await
        .expect("the awaited send settles well inside the timeout");

        let rtt_ms = match delivered {
            Ok(Delivered { rtt_ms }) => rtt_ms,
            other => panic!("the capstone send must settle Delivered, got {other:?}"),
        };
        assert!(
            rtt_ms < 5_000,
            "a live loopback proof returns fast, measured rtt_ms = {rtt_ms}",
        );
    }

    #[tokio::test]
    async fn a_send_settles_delivered_through_a_keyless_relay_across_three_live_runtimes() {
        use std::sync::mpsc::{channel, Receiver, Sender};

        use crate::engine::self_announce::AnnounceConfig;
        use crate::engine::test_support::{fixed_secret_key, second_secret_key};
        use crate::engine::{
            Delivered, EngineState, RatchetPolicy, ReannounceSchedule, SendSingle, SendSingleError,
            SendSingleFailure, SendSinglePayload,
        };
        use crate::interfaces::storage::{FixedInterfaceSet, InterfaceSet};
        use crate::interfaces::{
            ConnectionState, ControlReport, DriverMode, EgressCapability, InboundPacket,
            IngressCapability, InterfaceCapabilities, InterfaceDescriptor, InterfaceHandle,
            InterfaceMode, MediumKind, SendError, StartedInterface, TransportCapability,
        };
        use crate::routing::storage::FixedInline;
        use crate::routing::upstream_app_destinations::ProofStrategy;
        use crate::runtime::{PrnsEvent, Runtime};
        use crate::wire::{TransportId, MTU};

        // Three engines live on this test thread at once; a desk-sized storage
        // spell keeps their combined footprint far from the thread's stack.
        type SmallCap = FixedInline<8, 8, 512, 4, 64, 4, 4, 4, 4, 32, 4, 4, 4, 4>;

        struct ChannelHandle {
            id: InterfaceId,
            clock_base: Instant,
            rx: Receiver<std::vec::Vec<u8>>,
            tx: Sender<std::vec::Vec<u8>>,
            peer_wake: TokioWakeHandle,
        }

        impl InterfaceHandle for ChannelHandle {
            fn next_inbound<R>(&mut self, f: impl FnOnce(InboundPacket<'_>) -> R) -> Option<R> {
                let mut bytes = self.rx.try_recv().ok()?;
                Some(f(InboundPacket {
                    arrived_at: InstantMillis(self.clock_base.elapsed().as_millis() as u64),
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
                let _ = self.tx.send(buf[..written].to_vec());
                self.peer_wake.poke();
                Ok(written)
            }
            fn request_stop(&mut self) {}
            fn next_report(&mut self) -> Option<ControlReport> {
                None
            }
        }

        fn linked_descriptor(id: InterfaceId) -> InterfaceDescriptor {
            InterfaceDescriptor {
                id,
                capabilities: InterfaceCapabilities {
                    ingress: IngressCapability::Enabled,
                    egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
                },
                mode: InterfaceMode::Full,
                medium: MediumKind::Loopback,
                state: ConnectionState::Connected,
            }
        }

        type LinkedInterface = StartedInterface<ChannelHandle, core::convert::Infallible>;

        let mut prover: EngineState<SmallCap> = EngineState::new(fixed_secret_key());
        let node = prover.held_identity_hashes()[0];
        let destination = prover
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        prover
            .schedule_announce(
                &destination,
                AnnounceConfig {
                    app_data: b"three-node-capstone",
                    schedule: ReannounceSchedule::default(),
                },
            )
            .unwrap();
        let sender: EngineState<SmallCap> = EngineState::new(second_secret_key());

        // The relay holds no identity and registers nothing: a bare transport id
        // is its entire cryptographic existence.
        let mut relay: EngineState<SmallCap> = EngineState::<SmallCap>::default();
        relay.set_transport_id(TransportId::new([0x7A; 16]));

        let sender_host = TokioHost::new();
        let relay_host = TokioHost::new();
        let prover_host = TokioHost::new();

        let (sender_to_relay_tx, sender_to_relay_rx) = channel();
        let (relay_to_sender_tx, relay_to_sender_rx) = channel();
        let (relay_to_prover_tx, relay_to_prover_rx) = channel();
        let (prover_to_relay_tx, prover_to_relay_rx) = channel();

        let mut sender_set = FixedInterfaceSet::<LinkedInterface, 1>::new();
        let _ = sender_set.push(StartedInterface {
            descriptor: linked_descriptor(InterfaceId::new([0xA1; 16])),
            handle: ChannelHandle {
                id: InterfaceId::new([0xA1; 16]),
                clock_base: Instant::now(),
                rx: relay_to_sender_rx,
                tx: sender_to_relay_tx,
                peer_wake: relay_host.wake_handle(),
            },
            drive: DriverMode::SelfDriven,
        });

        let mut relay_set = FixedInterfaceSet::<LinkedInterface, 2>::new();
        let _ = relay_set.push(StartedInterface {
            descriptor: linked_descriptor(InterfaceId::new([0xB2; 16])),
            handle: ChannelHandle {
                id: InterfaceId::new([0xB2; 16]),
                clock_base: Instant::now(),
                rx: sender_to_relay_rx,
                tx: relay_to_sender_tx,
                peer_wake: sender_host.wake_handle(),
            },
            drive: DriverMode::SelfDriven,
        });
        let _ = relay_set.push(StartedInterface {
            descriptor: linked_descriptor(InterfaceId::new([0xC3; 16])),
            handle: ChannelHandle {
                id: InterfaceId::new([0xC3; 16]),
                clock_base: Instant::now(),
                rx: prover_to_relay_rx,
                tx: relay_to_prover_tx,
                peer_wake: prover_host.wake_handle(),
            },
            drive: DriverMode::SelfDriven,
        });

        let mut prover_set = FixedInterfaceSet::<LinkedInterface, 1>::new();
        let _ = prover_set.push(StartedInterface {
            descriptor: linked_descriptor(InterfaceId::new([0xD4; 16])),
            handle: ChannelHandle {
                id: InterfaceId::new([0xD4; 16]),
                clock_base: Instant::now(),
                rx: relay_to_prover_rx,
                tx: prover_to_relay_tx,
                peer_wake: relay_host.wake_handle(),
            },
            drive: DriverMode::SelfDriven,
        });

        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        let commander = Arc::new(Commander {
            next_id: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            command_tx,
            wake: sender_host.wake_handle(),
        });

        let sender_runtime = Runtime::new(sender, sender_set, sender_host);
        let relay_runtime = Runtime::new(relay, relay_set, relay_host);
        let prover_runtime = Runtime::new(prover, prover_set, prover_host);
        let settler = commander.clone();
        tokio::spawn(async move {
            sender_runtime
                .run(
                    move |event| {
                        if let PrnsEvent::CommandSettled { id, settlement } = event {
                            settler.settle(id, settlement);
                        }
                    },
                    move || command_rx.try_recv().ok(),
                )
                .await
        });
        tokio::spawn(async move { relay_runtime.run(|_event| {}, || None).await });
        tokio::spawn(async move { prover_runtime.run(|_event| {}, || None).await });

        let payload = SendSinglePayload::from_slice(b"across-the-relay").unwrap();
        let delivered = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let settled = commander
                    .issue(SendSingle {
                        destination,
                        payload: payload.clone(),
                    })
                    .await;
                match settled {
                    Err(SendSingleFailure::Rejected(SendSingleError::NoRouteToDestination)) => {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    settled => break settled,
                }
            }
        })
        .await
        .expect("the relayed send settles well inside the timeout");

        let rtt_ms = match delivered {
            Ok(Delivered { rtt_ms }) => rtt_ms,
            other => panic!("the relayed send must settle Delivered, got {other:?}"),
        };
        assert!(
            rtt_ms < 5_000,
            "a proof crossing two hops still returns fast, measured rtt_ms = {rtt_ms}",
        );
    }
}
