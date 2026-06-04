use heapless::Vec as HeaplessVec;

use crate::interfaces::{InterfaceId, MAX_REGISTERED_INTERFACES};

pub(crate) struct TrafficLedger {
    entries: HeaplessVec<InterfaceTraffic, MAX_REGISTERED_INTERFACES>,
}

struct InterfaceTraffic {
    id: InterfaceId,
    reticulum_rx_byte_count: u64,
    reticulum_tx_byte_count: u64,
}

impl TrafficLedger {
    pub(crate) const fn new() -> Self {
        Self {
            entries: HeaplessVec::new(),
        }
    }

    fn row_mut(&mut self, id: InterfaceId) -> Option<&mut InterfaceTraffic> {
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            return Some(&mut self.entries[i]);
        }
        self.entries
            .push(InterfaceTraffic {
                id,
                reticulum_rx_byte_count: 0,
                reticulum_tx_byte_count: 0,
            })
            .ok()?;
        self.entries.last_mut()
    }

    pub(crate) fn add_rx(&mut self, id: InterfaceId, bytes: u64) {
        if let Some(row) = self.row_mut(id) {
            row.reticulum_rx_byte_count = row.reticulum_rx_byte_count.wrapping_add(bytes);
        }
    }

    pub(crate) fn add_tx(&mut self, id: InterfaceId, bytes: u64) {
        if let Some(row) = self.row_mut(id) {
            row.reticulum_tx_byte_count = row.reticulum_tx_byte_count.wrapping_add(bytes);
        }
    }

    pub(crate) fn totals_for(&self, id: InterfaceId) -> (u64, u64) {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| (e.reticulum_rx_byte_count, e.reticulum_tx_byte_count))
            .unwrap_or((0, 0))
    }
}
