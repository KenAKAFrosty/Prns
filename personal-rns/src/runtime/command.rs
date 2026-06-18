//! Platform-neutral command-surface types — shared by every command handle, so a `send_single`
//! resolves to the same `Result` whether tokio's unbounded oneshot or embassy's fixed completion
//! pool carried the awaited settlement.

use crate::engine::{CommandId, Delivered, EngineCommand, InterfaceCounts, SendSingleFailure};
use crate::interfaces::InterfaceId;
use crate::routing::links::LinkId;
use crate::wire::DestinationHash;

use super::request_router::RespondToken;

/// Why an awaited send never reached `Delivered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError<F> {
    /// The payload was larger than a single packet's MDU — rejected before the wire.
    PayloadTooLarge,
    /// The node has stopped: the command channel is closed (host) or the bounded lane is gone.
    NodeStopped,
    /// More awaited sends are in flight than the platform tracks at once — the embedded
    /// `CompletionPool` is full. The unbounded host path never returns it.
    Busy,
    /// The engine settled the send as a typed failure (`SendSingleFailure`, …).
    Failed(F),
}

/// The high-level node API every platform's handle presents — `TokioPrnsHandle` over an unbounded channel
/// and a oneshot, `EmbassyPrnsHandle` over a bounded channel and a static completion pool — the same
/// verbs either way, so engine logic ports between a desktop and a board by recompiling, not
/// rewriting. Each verb is the same command-roundtrip both runtimes already run: [`issue`](Self::issue)
/// mints a [`CommandId`] and returns at once; an awaiting verb issues and then `.await`s the
/// settlement demuxed by that id (a host oneshot, a board completion-pool signal).
///
/// What genuinely differs between the platforms stays *off* this trait, as inherent methods on each
/// concrete handle: interface lifecycle (the host's dynamic `add_interface(iface)` against the board's
/// static slot `activate(slot, config)`), host-only capabilities, and the platform extras a runner
/// needs (tokio's zero-copy `respond_owned`). The trait is the shared core; the divergence is honest.
///
/// The handle mints every [`CommandId`] from one counter, so the app never picks ids and a
/// fire-and-forget [`issue`](Self::issue) can't collide with an awaited [`send_single`](Self::send_single).
#[allow(async_fn_in_trait)]
pub trait PrnsApi {
    /// Queue an engine command and return the [`CommandId`] it was minted under — watch the event
    /// stream for the settlement tagged with it. `None` once the node has stopped (or the bounded
    /// embedded lane is full). The fire-and-forget escape hatch.
    fn issue(&self, command: EngineCommand) -> Option<CommandId>;

    /// Send one Single data packet to `destination` and await its delivery proof — `Ok(Delivered)`
    /// with the measured round trip, or the typed reason it did not deliver.
    async fn send_single(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<Delivered, SendError<SendSingleFailure>>;

    /// Read the live packet and byte counts the engine holds for one interface.
    async fn interface_counts(&self, interface: InterfaceId) -> Option<InterfaceCounts>;

    /// Answer a request with `body`. Returns `false` once the node has stopped (or, on embedded, if
    /// `body` exceeds the single-packet MDU the inline responder can carry).
    fn respond(&self, responder: RespondToken, body: &[u8]) -> bool;

    /// Sever an active link. Returns `false` once the node has stopped.
    fn close_link(&self, link_id: LinkId) -> bool;
}
