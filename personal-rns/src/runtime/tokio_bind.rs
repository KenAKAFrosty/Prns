//! The tokio platform binding — the first concrete [`Bind`], distilled from the hand-roll in the
//! benchmark `scenario_node`. It owns the interface descriptors, their grant lanes, the
//! inbound-notify and command channels, the [`TokioHost`], and the reactor call; the runtime
//! hands it a fully assembled engine and it drives the tokio reactor forever.
//!
//! The command channel's sender comes out of [`TokioBind::new`] as a [`TokioCommands`] handle —
//! the dual-side surface: keep it to drive the node inline before the final `Prns::run`, or move
//! it into other tasks when you taskify the run.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::engine::{
    CloseLink, CommandId, Delivered, EngineCommand, EngineState, IssuedCommand, SendSingle,
    SendSingleFailure, SendSinglePayload, Settlement,
};
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::reactor::impls::tokio_reactor::{
    self, tokio_grant_lane, Egress, HostCommand, HostResourcePayload, RespondAnyHostCommand,
    TokioGrantConsumer, TokioGrantProducer, TokioHost, TokioInterfaceSeam,
};
use crate::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::{Bind, PrnsEvent, Responder};

const LANE_DEPTH: usize = 64;

/// Why an awaited send never reached `Delivered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError<F> {
    PayloadTooLarge,
    NodeStopped,
    Failed(F),
}

/// A cloneable handle to the running node — bind it as `prns` and drive the node through it.
/// Obtained from [`TokioBind::new`]; usable inline (before the final `Prns::run`) or moved into
/// other tasks. Clones share one node, so an awaited send on one resolves wherever it settles.
///
/// The handle mints every [`CommandId`] from one counter — the app never picks ids, so a
/// fire-and-forget [`issue`](Self::issue) can never collide with an awaited
/// [`send_single`](Self::send_single) or a runner's [`respond`](Self::respond). `issue` returns the
/// id it minted (watch `on_event` for that settlement); `send_single` parks on a oneshot the reactor
/// fires, the completion riding the command with no shared registry; `respond` / `close_link` are
/// the request runner's answer paths.
#[derive(Clone)]
pub struct TokioCommands {
    commands: UnboundedSender<HostCommand>,
    ids: Arc<AtomicU64>,
}

impl TokioCommands {
    #[cfg(test)]
    pub(crate) fn over(commands: UnboundedSender<HostCommand>) -> Self {
        Self {
            commands,
            ids: Arc::new(AtomicU64::new(0)),
        }
    }

    fn mint(&self) -> CommandId {
        CommandId(self.ids.fetch_add(1, Ordering::Relaxed))
    }

    /// Queue an engine command (announce, send single, establish link, …) for the next reactor
    /// cycle and return the [`CommandId`] it was minted under — watch `on_event` for the settlement
    /// tagged with it. `None` once the node has stopped and the channel is closed. This is the
    /// fire-and-forget escape hatch; to await the outcome instead, prefer the typed methods.
    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        let id = self.mint();
        self.commands
            .send(HostCommand::Engine(IssuedCommand { id, command }))
            .ok()?;
        Some(id)
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

    /// Mint a [`CommandId`], issue the command with a oneshot the reactor fires on settlement, and
    /// park on it — `None` if the node stopped before settling (the channel closed, or the reactor
    /// dropped the completion). The completion rides the command, so the correlation registry lives
    /// on the single reactor task with no lock.
    async fn settle(&self, command: EngineCommand) -> Option<Settlement> {
        let id = self.mint();
        let (completion, settled) = oneshot::channel();
        self.commands
            .send(HostCommand::AwaitedEngine {
                issued: IssuedCommand { id, command },
                completion,
            })
            .ok()?;
        settled.await.ok()
    }

    fn send_response(&self, responder: Responder, data: HostResourcePayload) -> bool {
        let id = self.mint();
        self.commands
            .send(HostCommand::RespondAny(RespondAnyHostCommand {
                id,
                link_id: responder.link_id,
                request_id: responder.request_id,
                data,
                compressed_candidate: None,
            }))
            .is_ok()
    }

    /// Answer a request with `body` of any length: the engine picks the rung — a single RESPONSE
    /// packet when it fits the link MDU, an outgoing resource named back to the request when it
    /// doesn't. This is the app's defer path — keep the [`Responder`] a handler hands back when it
    /// returns `Err(Decline::Drop)` and answer later, off the runner's task. `body` is copied; when
    /// you already own the bytes, [`respond_owned`](Self::respond_owned) moves them. Returns `false`
    /// once the node has stopped and the channel is closed.
    pub fn respond(&self, responder: Responder, body: &[u8]) -> bool {
        self.send_response(responder, body.to_vec().into())
    }

    /// Answer with an owned `body` — the request runner's path: the filled grant moves straight into
    /// the response with no copy. Same auto-upgrade and id discipline as [`respond`](Self::respond).
    pub fn respond_owned(&self, responder: Responder, body: std::vec::Vec<u8>) -> bool {
        self.send_response(responder, body.into())
    }

    /// Sever an active link — the runner's path for a handler that returns `Err(Decline::CloseLink)`,
    /// and usable directly to tear a link down. Queues RNS 1.3.1's `Link.teardown` (the sealed
    /// LINKCLOSE) for the next reactor cycle. Returns `false` once the node has stopped.
    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.issue(EngineCommand::CloseLink(CloseLink { link_id }))
            .is_some()
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
    _storage: core::marker::PhantomData<S>,
}

impl<S: StorageLayout> TokioBind<S> {
    /// Build an empty binding on `host`, returning it alongside the command handle the app keeps.
    pub fn new(host: TokioHost) -> (Self, TokioCommands) {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        (
            Self {
                host,
                interfaces: std::vec::Vec::new(),
                inbound: std::vec::Vec::new(),
                egress_lanes: std::vec::Vec::new(),
                notify_tx,
                notify_rx,
                command_rx,
                _storage: core::marker::PhantomData,
            },
            TokioCommands {
                commands: command_tx,
                ids: Arc::new(AtomicU64::new(0)),
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
            |journaled| on_event(PrnsEvent::from(journaled)),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MAX_SEND_SINGLE_PLAINTEXT_LEN;

    const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

    fn handle() -> (TokioCommands, UnboundedReceiver<HostCommand>) {
        let (commands, command_rx) = mpsc::unbounded_channel();
        (TokioCommands::over(commands), command_rx)
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
    async fn an_awaited_send_issues_the_completion_carrying_command() {
        let (prns, mut command_rx) = handle();
        let issuer = prns.clone();
        let send = tokio::spawn(async move { issuer.send_single(PEER, b"ping").await });

        match command_rx.recv().await.expect("the command was issued") {
            HostCommand::AwaitedEngine { issued, completion } => {
                assert!(matches!(issued.command, EngineCommand::SendSingle(_)));
                completion
                    .send(Settlement::SendSingle(Ok(Delivered {
                        rtt: crate::units::Rtt::from_millis(7),
                    })))
                    .expect("the awaiter is still parked");
            }
            _ => panic!("send_single must issue an AwaitedEngine command"),
        }

        assert_eq!(
            send.await.expect("the send task joins"),
            Ok(Delivered {
                rtt: crate::units::Rtt::from_millis(7),
            }),
        );
    }
}
