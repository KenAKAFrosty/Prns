use heapless::Vec as HeaplessVec;

use crate::interfaces::{ConnectionState, InterfaceId, MAX_REGISTERED_INTERFACES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceView {
    pub id: InterfaceId,
    pub connection_state: ConnectionState,
    pub reticulum_rx_byte_count: u64,
    pub reticulum_tx_byte_count: u64,
    pub tracked_destinations: u32,
}

#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub interfaces: HeaplessVec<InterfaceView, MAX_REGISTERED_INTERFACES>,
}
