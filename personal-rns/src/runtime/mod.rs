//! The runtime layer — the engine-bolt above the passive engine.
//!
//! [`Manifold`] owns the engine and its registered worker and turns one drive
//! cycle into intake → step → exhaust. It is substrate-neutral (no clock, RNG,
//! sockets, or `.await`); the per-platform runtime loop that drives it — drawing
//! entropy, aggregating each worker's inbound, and deciding when to cycle —
//! lives with that platform's worker shells.

mod manifold;

pub use manifold::Manifold;
