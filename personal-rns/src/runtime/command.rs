//! Platform-neutral command-surface types — shared by every [`Bind`]'s command handle, so a
//! `send_single` resolves to the same `Result` whether tokio's unbounded oneshot or embassy's
//! fixed completion pool carried the awaited settlement.
//!
//! [`Bind`]: super::Bind

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
