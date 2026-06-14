//! The app-facing event lane. Today it is the engine's `Journaled` stream verbatim — the
//! same value the reactor hands to its `on_journaled` callback — so apps observe the engine
//! directly with no translation. The old runtime curated a snapshot-shaped `PrnsEvent`
//! (AnnounceHeard / Delivered / CommandSettled / SnapshotUpdated); that curation can layer
//! back on top here without changing the `Prns::run` signature.

pub use crate::engine::Journaled as PrnsEvent;
