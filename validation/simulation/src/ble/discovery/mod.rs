use std::collections::VecDeque;
use std::num::NonZeroUsize;

use super::{BleAddress, BleRadioId};

/// The observed incarnation of a peer, not just its reusable Bluetooth address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BleDiscoveredPeer {
    pub address: BleAddress,
    pub radio: BleRadioId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleDiscoverySnapshot {
    pub capacity: NonZeroUsize,
    /// Least recently observed first. Dial attempts do not refresh this order.
    pub peers: Vec<BleDiscoveredPeer>,
    /// Saturating lifetime count of capacity evictions; radio shutdown does not reset it.
    pub evicted_peers: u64,
}

pub(super) struct DiscoveryCache {
    capacity: NonZeroUsize,
    peers: VecDeque<BleDiscoveredPeer>,
    evicted_peers: u64,
}

impl DiscoveryCache {
    pub(super) fn new(capacity: NonZeroUsize) -> Self {
        Self {
            capacity,
            peers: VecDeque::new(),
            evicted_peers: 0,
        }
    }

    pub(super) fn observe(&mut self, peer: BleDiscoveredPeer) {
        if let Some(index) = self
            .peers
            .iter()
            .position(|known| known.address == peer.address)
        {
            let _ = self.peers.remove(index);
        } else if self.peers.len() == self.capacity.get() {
            let _ = self.peers.pop_front();
            self.evicted_peers = self.evicted_peers.saturating_add(1);
        }
        self.peers.push_back(peer);
    }

    pub(super) fn radio_for(&self, address: BleAddress) -> Option<BleRadioId> {
        self.peers
            .iter()
            .find(|peer| peer.address == address)
            .map(|peer| peer.radio)
    }

    pub(super) fn clear(&mut self) {
        self.peers.clear();
    }

    pub(super) fn snapshot(&self) -> BleDiscoverySnapshot {
        BleDiscoverySnapshot {
            capacity: self.capacity,
            peers: self.peers.iter().copied().collect(),
            evicted_peers: self.evicted_peers,
        }
    }
}

#[cfg(test)]
mod backend_tests;
#[cfg(test)]
mod tests;
