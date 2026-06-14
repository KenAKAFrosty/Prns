//! The tokio platform binding — the first concrete [`Bind`], distilled from the hand-roll in the
//! benchmark `scenario_node`. It owns the interface descriptors, their grant lanes, the
//! inbound-notify and command channels, the [`TokioHost`], and the reactor call; the runtime
//! hands it a fully assembled engine and it drives the tokio reactor forever.
//!
//! The command channel's sender comes out of [`TokioBind::new`] as a [`TokioCommands`] handle —
//! the dual-side surface: keep it to drive the node inline before the final `Prns::run`, or move
//! it into other tasks when you taskify the run.

use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::engine::{CommandId, EngineState, IssuedCommand};
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::reactor::impls::tokio_reactor::{
    self, tokio_grant_lane, Egress, HostCommand, RespondAnyHostCommand, TokioGrantConsumer,
    TokioGrantProducer, TokioHost, TokioInterfaceSeam,
};
use crate::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
use crate::storage::StorageLayout;

use super::{Bind, PrnsEvent, Responder};

const LANE_DEPTH: usize = 64;

/// Response command ids are minted from the top of the id space so a runner answering requests
/// never collides with the app's own [`TokioCommands::issue`] ids (which count up from the app's
/// chosen base). The app would have to be issuing 2^63 commands to meet them.
const RESPONSE_COMMAND_ID_BASE: u64 = 1 << 63;

/// A cloneable command sender — the app's handle to drive the running node. Obtained from
/// [`TokioBind::new`]; usable inline (before the final `Prns::run`) or from other tasks. Clones
/// share one response-id counter, so every [`respond`](Self::respond) across them stays unique.
#[derive(Clone)]
pub struct TokioCommands {
    tx: UnboundedSender<HostCommand>,
    respond_ids: Arc<AtomicU64>,
}

impl TokioCommands {
    pub(crate) fn over(tx: UnboundedSender<HostCommand>) -> Self {
        Self {
            tx,
            respond_ids: Arc::new(AtomicU64::new(RESPONSE_COMMAND_ID_BASE)),
        }
    }

    /// Queue an engine command (announce, send single, establish link, …) for the next reactor
    /// cycle. Returns `false` once the node has stopped and the channel is closed.
    pub fn issue(&self, command: IssuedCommand) -> bool {
        self.tx.send(HostCommand::Engine(command)).is_ok()
    }

    /// Answer a request with `body` of any length: the engine picks the rung — a single RESPONSE
    /// packet when it fits the link MDU, an outgoing resource named back to the request when it
    /// doesn't. This is both the request runner's issue path and the app's defer path — keep the
    /// [`Responder`] a handler hands back when it returns `Response::None` and answer later, off
    /// the runner's task. Returns `false` once the node has stopped and the channel is closed.
    pub fn respond(&self, responder: Responder, body: &[u8]) -> bool {
        let id = CommandId(self.respond_ids.fetch_add(1, Ordering::Relaxed));
        self.tx
            .send(HostCommand::RespondAny(RespondAnyHostCommand {
                id,
                link_id: responder.link_id,
                request_id: responder.request_id,
                data: body.to_vec().into(),
                compressed_candidate: None,
            }))
            .is_ok()
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
            TokioCommands::over(command_tx),
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
