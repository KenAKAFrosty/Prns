//! Platform-neutral command-surface types — shared by every [`Bind`]'s command handle, so a
//! `send_single` resolves to the same `Result` whether tokio's unbounded oneshot or embassy's
//! fixed completion pool carried the awaited settlement.
//!
//! [`Bind`]: super::Bind

use crate::engine::{CommandId, Delivered, EngineCommand, SendSingleFailure};
use crate::routing::links::LinkId;
use crate::wire::DestinationHash;

use super::Responder;

/// Why an awaited send never reached `Delivered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError<F> {
    /// The payload was larger than a single packet's MDU — rejected before the wire.
    PayloadTooLarge,
    /// The node has stopped: the command channel is closed (host) or the bounded lane is gone.
    NodeStopped,
    /// More awaited sends are in flight than the platform tracks at once — the embedded
    /// [`CompletionPool`](super::CompletionPool) is full. The unbounded host path never returns it.
    Busy,
    /// The engine settled the send as a typed failure (`SendSingleFailure`, …).
    Failed(F),
}

/// The one command surface every platform's handle presents — `TokioCommands` over an unbounded
/// channel and a oneshot, `EmbassyCommands` over a bounded channel and a static completion pool,
/// the same four verbs either way. Consumer code (and the request runner) takes `impl Commands` and
/// runs on whichever platform supplied it; each handle also keeps the platform-specific extras its
/// runner needs (tokio's zero-copy `respond_owned`) as inherent methods outside this trait.
///
/// The handle mints every [`CommandId`] from one counter, so the app never picks ids and a
/// fire-and-forget [`issue`](Self::issue) can't collide with an awaited [`send_single`](Self::send_single).
#[allow(async_fn_in_trait)]
pub trait Commands {
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

    /// Answer a request with `body`. Returns `false` once the node has stopped (or, on embedded, if
    /// `body` exceeds the single-packet MDU the inline responder can carry).
    fn respond(&self, responder: Responder, body: &[u8]) -> bool;

    /// Sever an active link. Returns `false` once the node has stopped.
    fn close_link(&self, link_id: LinkId) -> bool;
}
