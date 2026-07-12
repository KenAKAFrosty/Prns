use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::tunnel::registry::TunnelTable;
use crate::routing::tunnel::TunnelId;
use crate::storage::TablePushError;

#[derive(Debug, Default)]
pub struct HeapTunnelTable {
    tunnel_ids: Vec<TunnelId>,
    interfaces: Vec<InterfaceId>,
    expiries: Vec<InstantMillis>,
}

impl TunnelTable for HeapTunnelTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.tunnel_ids.len()
    }

    fn tunnel_ids(&self) -> &[TunnelId] {
        &self.tunnel_ids
    }
    fn interfaces(&self) -> &[InterfaceId] {
        &self.interfaces
    }
    fn expiries(&self) -> &[InstantMillis] {
        &self.expiries
    }

    fn set_row(&mut self, i: usize, interface: InterfaceId, expires: InstantMillis) {
        self.interfaces[i] = interface;
        self.expiries[i] = expires;
    }

    fn push(
        &mut self,
        tunnel_id: TunnelId,
        interface: InterfaceId,
        expires: InstantMillis,
    ) -> Result<(), TablePushError> {
        self.tunnel_ids.push(tunnel_id);
        self.interfaces.push(interface);
        self.expiries.push(expires);
        Ok(())
    }

    fn swap_remove(&mut self, i: usize) {
        self.tunnel_ids.swap_remove(i);
        self.interfaces.swap_remove(i);
        self.expiries.swap_remove(i);
    }
}
