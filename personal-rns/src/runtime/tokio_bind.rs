//! The tokio platform binding — the first concrete [`Bind`], distilled from the hand-roll in the
//! benchmark `scenario_node`. It owns the interface descriptors, their grant lanes, the
//! inbound-notify and command channels, the [`TokioHost`], and the reactor call; the runtime
//! hands it a fully assembled engine and it drives the tokio reactor forever.
//!
//! The command channel's sender comes out of [`TokioBind::new`] as a [`TokioCommands`] handle —
//! the dual-side surface: keep it to drive the node inline before the final `Prns::run`, or move
//! it into other tasks when you taskify the run.

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::engine::{EngineState, IssuedCommand};
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::reactor::impls::tokio_reactor::{
    self, tokio_grant_lane, Egress, HostCommand, TokioGrantConsumer, TokioGrantProducer, TokioHost,
    TokioInterfaceSeam,
};
use crate::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
use crate::storage::StorageLayout;

use super::{Bind, PrnsEvent};

const LANE_DEPTH: usize = 64;

/// A cloneable command sender — the app's handle to drive the running node. Obtained from
/// [`TokioBind::new`]; usable inline (before the final `Prns::run`) or from other tasks.
#[derive(Clone)]
pub struct TokioCommands(UnboundedSender<HostCommand>);

impl TokioCommands {
    /// Queue an engine command (announce, send single, establish link, …) for the next reactor
    /// cycle. Returns `false` once the node has stopped and the channel is closed.
    pub fn issue(&self, command: IssuedCommand) -> bool {
        self.0.send(HostCommand::Engine(command)).is_ok()
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
            TokioCommands(command_tx),
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
