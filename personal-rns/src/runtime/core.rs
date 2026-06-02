//! The runtime's per-interface traffic meter, shared by the
//! [`ContractRuntime`](super::ContractRuntime).

use heapless::Vec as HeaplessVec;

use crate::engine::MAX_REGISTERED_INTERFACES;
use crate::interfaces::InterfaceId;

/// Cumulative Reticulum byte totals per interface, the runtime's own meter of the
/// fabric traffic crossing its seam. Capacity is [`MAX_REGISTERED_INTERFACES`] — the
/// same cap the engine's fanout uses — so an id from any registered interface always
/// has a slot. `pub(crate)` so the [`ContractRuntime`](super::ContractRuntime) meters
/// through it without duplicating the ledger.
pub(crate) struct TrafficLedger {
    entries: HeaplessVec<InterfaceTraffic, MAX_REGISTERED_INTERFACES>,
}

struct InterfaceTraffic {
    id: InterfaceId,
    reticulum_rx_bytes: u64,
    reticulum_tx_bytes: u64,
}

impl TrafficLedger {
    pub(crate) const fn new() -> Self {
        Self {
            entries: HeaplessVec::new(),
        }
    }

    /// The interface's row, inserting a zeroed one on first sight. `None` only if more
    /// than [`MAX_REGISTERED_INTERFACES`] distinct ids appear — which the engine's own
    /// registration cap forbids — so callers treat it as a benign no-op.
    fn row_mut(&mut self, id: InterfaceId) -> Option<&mut InterfaceTraffic> {
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            return Some(&mut self.entries[i]);
        }
        self.entries
            .push(InterfaceTraffic {
                id,
                reticulum_rx_bytes: 0,
                reticulum_tx_bytes: 0,
            })
            .ok()?;
        self.entries.last_mut()
    }

    pub(crate) fn add_rx(&mut self, id: InterfaceId, bytes: u64) {
        if let Some(row) = self.row_mut(id) {
            row.reticulum_rx_bytes = row.reticulum_rx_bytes.wrapping_add(bytes);
        }
    }

    pub(crate) fn add_tx(&mut self, id: InterfaceId, bytes: u64) {
        if let Some(row) = self.row_mut(id) {
            row.reticulum_tx_bytes = row.reticulum_tx_bytes.wrapping_add(bytes);
        }
    }

    /// `(rx, tx)` totals for `id`; `(0, 0)` for an interface that hasn't yet carried
    /// any Reticulum traffic.
    pub(crate) fn totals_for(&self, id: InterfaceId) -> (u64, u64) {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| (e.reticulum_rx_bytes, e.reticulum_tx_bytes))
            .unwrap_or((0, 0))
    }
}
