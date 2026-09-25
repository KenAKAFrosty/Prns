use std::collections::{btree_map::Entry, BTreeMap};
use std::sync::Arc;

use super::{BleAddress, Connection};

/// Admission bounds each radio's entries by its connection budget. Retired entries are
/// reclaimed on that radio's next operation. Endpoint teardown only closes the shared
/// lifecycle; it never takes the network lock.
#[derive(Default)]
pub(in crate::ble) struct ConnectionIndex {
    by_radio: BTreeMap<BleAddress, Vec<Arc<Connection>>>,
}

impl ConnectionIndex {
    pub(in crate::ble) fn insert(&mut self, connection: Arc<Connection>) {
        for address in connection.addresses {
            let connections = self.by_radio.entry(address).or_default();
            connections.retain(|connection| !connection.is_closed());
            connections.push(connection.clone());
        }
    }

    pub(in crate::ble) fn active_count(&self) -> usize {
        self.by_radio
            .iter()
            .map(|(address, connections)| {
                connections
                    .iter()
                    .filter(|connection| {
                        connection.addresses[0] == *address && !connection.is_closed()
                    })
                    .count()
            })
            .sum()
    }

    pub(in crate::ble) fn count_for(&mut self, address: BleAddress) -> usize {
        self.prune(address);
        self.by_radio.get(&address).map_or(0, Vec::len)
    }

    pub(in crate::ble) fn disconnect_between(
        &mut self,
        first: BleAddress,
        second: BleAddress,
    ) -> usize {
        let closed = self.close_matching(first, |connection| connection.connects(second));
        self.prune(second);
        closed
    }

    pub(in crate::ble) fn disconnect_radio(&mut self, address: BleAddress) -> usize {
        self.close_matching(address, |_| true)
    }

    fn close_matching(
        &mut self,
        address: BleAddress,
        matches: impl Fn(&Connection) -> bool,
    ) -> usize {
        let closed = self
            .by_radio
            .get(&address)
            .into_iter()
            .flatten()
            .filter(|connection| matches(connection) && connection.close())
            .count();
        self.prune(address);
        closed
    }

    fn prune(&mut self, address: BleAddress) {
        if let Entry::Occupied(mut entry) = self.by_radio.entry(address) {
            entry.get_mut().retain(|connection| !connection.is_closed());
            if entry.get().is_empty() {
                let _ = entry.remove();
            }
        }
    }
}

#[cfg(test)]
mod tests;
