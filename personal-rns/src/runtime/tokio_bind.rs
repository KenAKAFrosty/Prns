//! The tokio platform binding — the first concrete [`Bind`], distilled from the hand-roll in the
//! benchmark `scenario_node`. It owns the interface descriptors, their grant lanes, the
//! inbound-notify and command channels, the [`TokioHost`], and the reactor call; the runtime
//! hands it a fully assembled engine and it drives the tokio reactor forever.
//!
//! The command channel's sender comes out of [`TokioBind::new`] as a [`TokioCommands`] handle —
//! the dual-side surface: keep it to drive the node inline before the final `Prns::run`, or move
//! it into other tasks when you taskify the run.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::engine::{
    CommandId, Delivered, EngineCommand, EngineState, IssuedCommand, Journaled, SendSingle,
    SendSingleFailure, SendSinglePayload, Settlement,
};
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::reactor::impls::tokio_reactor::{
    self, tokio_grant_lane, Egress, HostCommand, TokioGrantConsumer, TokioGrantProducer, TokioHost,
    TokioInterfaceSeam,
};
use crate::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::{Bind, PrnsEvent};

const LANE_DEPTH: usize = 64;

/// The awaiters for issued commands a caller is parked on: a settlement tagged with one of these
/// [`CommandId`]s is handed to its oneshot and consumed, never surfacing as an ambient event.
/// Shared between the [`TokioCommands`] handle (which registers) and the drive loop (which
/// resolves), so the async surface bridges the engine's fire-and-forget lane for the consumer.
type PendingSettlements = Mutex<HashMap<CommandId, oneshot::Sender<Settlement>>>;

/// Why an awaited send never reached `Delivered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError<F> {
    PayloadTooLarge,
    NodeStopped,
    Failed(F),
}

/// Recover the guard across a poisoned lock instead of panicking: the map holds only oneshot
/// senders, so a panic elsewhere leaves it structurally sound — a stale awaiter at worst.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Peel an awaited settlement off the event stream: a [`Journaled::CommandSettled`] whose id a
/// caller is parked on goes to that caller's oneshot and the event is dropped; everything else
/// (including a settlement nobody awaited) is returned for the app's `on_event`.
fn intercept_settlement<'a>(
    pending: &PendingSettlements,
    journaled: Journaled<'a>,
) -> Option<Journaled<'a>> {
    if let Journaled::CommandSettled { id, settlement } = &journaled {
        let awaiter = lock(pending).remove(id);
        if let Some(awaiter) = awaiter {
            let _ = awaiter.send(*settlement);
            return None;
        }
    }
    Some(journaled)
}

/// A cloneable handle to the running node — bind it as `prns` and drive the node through it.
/// Obtained from [`TokioBind::new`]; usable inline (before the final `Prns::run`) or moved into
/// other tasks.
///
/// Fire-and-forget commands go through [`issue`](Self::issue); a command whose outcome you want
/// to await — like [`send_single`](Self::send_single) — returns a future that resolves when the
/// engine settles it.
#[derive(Clone)]
pub struct TokioCommands {
    commands: UnboundedSender<HostCommand>,
    pending: Arc<PendingSettlements>,
    next_id: Arc<AtomicU64>,
}

impl TokioCommands {
    /// Queue an engine command (announce, send single, establish link, …) for the next reactor
    /// cycle. Returns `false` once the node has stopped and the channel is closed. The caller
    /// mints the [`CommandId`] and watches `on_event` for the matching settlement; to await the
    /// outcome instead, prefer the typed methods.
    pub fn issue(&self, command: IssuedCommand) -> bool {
        self.commands.send(HostCommand::Engine(command)).is_ok()
    }

    /// Send one Single data packet to `destination` and await its delivery proof. The future
    /// resolves when the engine settles the send — `Ok(Delivered)` with the round trip it
    /// measured, or the typed reason it did not deliver.
    pub async fn send_single(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<Delivered, SendError<SendSingleFailure>> {
        let payload =
            SendSinglePayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        match self
            .settle(EngineCommand::SendSingle(SendSingle {
                destination,
                payload,
            }))
            .await
        {
            Some(Settlement::SendSingle(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    /// Mint a [`CommandId`], register an awaiter for it, issue the command, and park until the
    /// drive loop routes its settlement back — `None` if the node stopped before settling.
    async fn settle(&self, command: EngineCommand) -> Option<Settlement> {
        let id = CommandId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (awaiter, settled) = oneshot::channel();
        lock(&self.pending).insert(id, awaiter);
        if !self.issue(IssuedCommand { id, command }) {
            lock(&self.pending).remove(&id);
            return None;
        }
        settled.await.ok()
    }
}

pub struct TokioBind<S: StorageLayout> {
    host: TokioHost,
    interfaces: std::vec::Vec<InterfaceConfig>,
    inbound: std::vec::Vec<(InterfaceId, TokioGrantConsumer<MAX_WIRE_FRAME_LEN>)>,
    egress_lanes: std::vec::Vec<(InterfaceId, TokioGrantProducer<MAX_WIRE_FRAME_LEN>)>,
    notify_tx: UnboundedSender<InterfaceId>,
    notify_rx: UnboundedReceiver<InterfaceId>,
    command_rx: UnboundedReceiver<HostCommand>,
    pending: Arc<PendingSettlements>,
    _storage: core::marker::PhantomData<S>,
}

impl<S: StorageLayout> TokioBind<S> {
    /// Build an empty binding on `host`, returning it alongside the command handle the app keeps.
    pub fn new(host: TokioHost) -> (Self, TokioCommands) {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let pending: Arc<PendingSettlements> = Arc::new(Mutex::new(HashMap::new()));
        (
            Self {
                host,
                interfaces: std::vec::Vec::new(),
                inbound: std::vec::Vec::new(),
                egress_lanes: std::vec::Vec::new(),
                notify_tx,
                notify_rx,
                command_rx,
                pending: Arc::clone(&pending),
                _storage: core::marker::PhantomData,
            },
            TokioCommands {
                commands: command_tx,
                pending,
                next_id: Arc::new(AtomicU64::new(0)),
            },
        )
    }

    /// Wire the grant lanes for an interface and return its [`TokioInterfaceSeam`]. The caller
    /// spawns the concrete interface's `run(seam)` — keeping the interface future (whose `Send`
    /// the `Interface` trait does not promise) at a monomorphic site where tokio can prove it.
    pub fn attach(&mut self, descriptor: InterfaceConfig) -> TokioInterfaceSeam {
        let id = descriptor.id;
        let (in_producer, in_consumer) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
        let (out_producer, out_consumer) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
        self.interfaces.push(descriptor);
        self.inbound.push((id, in_consumer));
        self.egress_lanes.push((id, out_producer));
        TokioInterfaceSeam::new(id, in_producer, self.notify_tx.clone(), out_consumer)
    }
}

impl<S: StorageLayout> Bind for TokioBind<S> {
    type Storage = S;

    async fn drive(self, engine: EngineState<S>, mut on_event: impl FnMut(PrnsEvent<'_>)) {
        let TokioBind {
            host,
            interfaces,
            inbound,
            egress_lanes,
            notify_rx,
            command_rx,
            pending,
            ..
        } = self;
        let egress = Egress::new(egress_lanes);
        tokio_reactor::run(
            engine,
            interfaces,
            std::vec::Vec::new(),
            host,
            notify_rx,
            inbound,
            command_rx,
            egress,
            |journaled| {
                if let Some(journaled) = intercept_settlement(&pending, journaled) {
                    on_event(PrnsEvent::from(journaled))
                }
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MAX_SEND_SINGLE_PLAINTEXT_LEN;
    use crate::units::Rtt;

    const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

    fn handle() -> (TokioCommands, UnboundedReceiver<HostCommand>) {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let prns = TokioCommands {
            commands,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
        };
        (prns, command_rx)
    }

    #[tokio::test]
    async fn payload_beyond_the_mdu_is_rejected_before_the_wire() {
        let (prns, _command_rx) = handle();
        let oversize = [0u8; MAX_SEND_SINGLE_PLAINTEXT_LEN + 1];
        assert_eq!(
            prns.send_single(PEER, &oversize).await,
            Err(SendError::PayloadTooLarge),
        );
    }

    #[tokio::test]
    async fn a_send_on_a_stopped_node_settles_as_node_stopped() {
        let (prns, command_rx) = handle();
        drop(command_rx);
        assert_eq!(
            prns.send_single(PEER, b"ping").await,
            Err(SendError::NodeStopped),
        );
    }

    #[tokio::test]
    async fn the_settled_proof_resolves_the_awaiting_send() {
        let (prns, mut command_rx) = handle();
        let pending = Arc::clone(&prns.pending);
        let issuer = prns.clone();
        let send = tokio::spawn(async move { issuer.send_single(PEER, b"ping").await });

        let HostCommand::Engine(issued) = command_rx.recv().await.expect("the command was issued")
        else {
            panic!("send_single issues an engine command");
        };
        let settled = Journaled::CommandSettled {
            id: issued.id,
            settlement: Settlement::SendSingle(Ok(Delivered {
                rtt: Rtt::from_millis(7),
            })),
        };
        assert!(intercept_settlement(&pending, settled).is_none());

        assert_eq!(
            send.await.expect("the send task joins"),
            Ok(Delivered {
                rtt: Rtt::from_millis(7)
            }),
        );
    }

    #[test]
    fn an_unawaited_settlement_passes_through_to_on_event() {
        let pending: PendingSettlements = Mutex::new(HashMap::new());
        let settled = Journaled::CommandSettled {
            id: CommandId(7),
            settlement: Settlement::SendSingle(Ok(Delivered {
                rtt: Rtt::from_millis(1),
            })),
        };
        assert!(intercept_settlement(&pending, settled).is_some());
    }
}
