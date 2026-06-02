//! App-facing runtime state published once per drive cycle.

use heapless::Vec as HeaplessVec;

use crate::engine::MAX_REGISTERED_INTERFACES;
use crate::interfaces::InterfaceId;

/// Per-interface slice of a [`RuntimeSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceView {
    pub id: InterfaceId,
    /// Whether the interface link is currently up.
    pub online: bool,
    /// Cumulative Reticulum bytes ingested from this interface since boot.
    pub reticulum_rx_bytes: u64,
    /// Cumulative Reticulum bytes emitted to this interface since boot.
    pub reticulum_tx_bytes: u64,
    /// Tracked destinations whose accepted announce arrived on this interface.
    pub tracked_destinations: u32,
}

/// Snapshot of every registered interface after one drive cycle.
#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub interfaces: HeaplessVec<InterfaceView, MAX_REGISTERED_INTERFACES>,
}
