use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::super::{CycleStamp, Host, NextWake};
use crate::engine::{EngineCycleEntropySeed, InstantMillis, ENGINE_CYCLE_ENTROPY_LEN};
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
    #[allow(clippy::expect_used)]
    async fn wait(&mut self, wake: NextWake) -> CycleStamp {
        // Suspend until the engine's next deadline or an interface pokes the
        // wake — whichever first — yielding the executor to the worker tasks
        // meanwhile. The wake is coalesced (`Notify` holds one permit); the
        // runtime drains every interface's ring each cycle regardless, so a
        // missed poke only costs the next deadline, never a packet.
        let timeout = wait_for(wake, self.now(), MAX_WAIT);
        if !timeout.is_zero() {
            let _ = tokio::time::timeout(timeout, self.wake.notified()).await;
        }

        let mut seed = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        getrandom::getrandom(&mut seed).expect("OS CSPRNG must provide cycle entropy");
        CycleStamp {
            now: self.now(),
            seed: EngineCycleEntropySeed::new(seed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::InboundSink;

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
        let stamp = host.wait(NextWake::Immediate).await;
        assert!(stamp.now.0 < 100);
    }

    #[tokio::test]
    async fn an_awaited_announce_now_settles_through_a_live_runtime() {
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::{Arc, Mutex};
        use tokio::sync::oneshot;

        use crate::engine::test_support::{fixed_secret_key, Cap};
        use crate::engine::{
            AnnounceAppData, AnnounceNow, AnnounceNowError, AnnounceNowFailure, AnnounceTarget,
            CommandId, EngineState, IssuedCommand, RatchetPolicy, Settleable, Settlement,
        };
        use crate::interfaces::storage::FixedInterfaceSet;
        use crate::interfaces::{ControlReport, InboundPacket, InterfaceHandle, SendError};
        use crate::routing::upstream_app_destinations::ProofStrategy;
        use crate::runtime::{PrnsEvent, Runtime};
        use crate::wire::DestinationHash;

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
                C::from_settlement(settlement)
                    .expect("the settlement variant matches the issued verb")
            }

            fn settle(&self, id: CommandId, settlement: Settlement) {
                let waiting = self.pending.lock().unwrap().remove(&id);
                if let Some(waiting) = waiting {
                    let _ = waiting.send(settlement);
                }
            }
        }

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
}
