//! The platform seam. Everything above this trait is platform-neutral assembly; everything
//! below it is where embassy (static const-sized grant lanes + `static` channels) and tokio
//! (heap `tokio_grant_lane` + mpsc) genuinely diverge — the wiring the reactor pushed to its
//! call sites, which `s3.rs::engine_task` and the benchmark `scenario_node` each hand-roll today.
//!
//! A `Bind` owns this node's interfaces, their inbound/egress grant lanes, the inbound-notify
//! and command channels, the egress, the [`Host`](crate::reactor::Host), and the reactor call.
//! `Prns::run` builds the engine, sets its transport role, and registers the recipe's starting
//! destinations, then hands the assembled engine here; the binding drives it forever on its
//! reactor, forwarding every `Journaled` to `on_event`.
//!
//! The app constructs the binding for its platform — keeping the command channel's sender so
//! its own tasks (a button, an announce ticker) can issue commands — and the binding is the
//! one place the gnarly per-platform lane wiring lives, distilled once from the two hand-rolls.

use crate::engine::EngineState;
use crate::storage::StorageLayout;

use super::PrnsEvent;

// Send-agnostic on purpose: embassy's reactor runs single-threaded (its future is not Send),
// so the trait must not force `Send` on the returned future the way a desugared
// `-> impl Future + Send` would. Each platform binding's future is whatever its reactor needs.
#[allow(async_fn_in_trait)]
pub trait Bind {
    /// The storage recipe this binding runs on (`Esp32S3<…>`, `GrowableHeap`, …). Choosing it
    /// here is what lets `Prns::run` infer the engine's sizing without a turbofish.
    type Storage: StorageLayout;

    /// Wire this binding's interfaces, lanes, and channels, then drive the assembled engine on
    /// the platform reactor until the node stops (it does not). Each `Journaled` the reactor
    /// emits is forwarded to `on_event`.
    async fn drive(
        self,
        engine: EngineState<Self::Storage>,
        on_event: impl FnMut(PrnsEvent<'_>),
    ) -> !;
}
