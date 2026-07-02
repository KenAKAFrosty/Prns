use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::tunnel::registry::TunnelColumns;
use crate::routing::tunnel::TunnelId;
use crate::storage::ColumnsFull;

#[derive(Debug)]
pub struct FixedTunnelColumns<const MAX: usize> {
    len: usize,
    tunnel_ids: [TunnelId; MAX],
    interfaces: [InterfaceId; MAX],
    expiries: [InstantMillis; MAX],
}

impl<const MAX: usize> Default for FixedTunnelColumns<MAX> {
    fn default() -> Self {
        Self {
            len: 0,
            tunnel_ids: [TunnelId::new([0u8; 32]); MAX],
            interfaces: [InterfaceId::new([0u8; 8]); MAX],
            expiries: [InstantMillis(0); MAX],
        }
    }
}

impl<const MAX: usize> TunnelColumns for FixedTunnelColumns<MAX> {
    fn capacity(&self) -> usize {
        MAX
    }
    fn len(&self) -> usize {
        self.len
    }

    fn tunnel_ids(&self) -> &[TunnelId] {
        &self.tunnel_ids[..self.len]
    }
    fn interfaces(&self) -> &[InterfaceId] {
        &self.interfaces[..self.len]
    }
    fn expiries(&self) -> &[InstantMillis] {
        &self.expiries[..self.len]
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
    ) -> Result<(), ColumnsFull> {
        if self.len >= MAX {
            return Err(ColumnsFull);
        }
        self.tunnel_ids[self.len] = tunnel_id;
        self.interfaces[self.len] = interface;
        self.expiries[self.len] = expires;
        self.len += 1;
        Ok(())
    }

    fn swap_remove(&mut self, i: usize) {
        let last = self.len - 1;
        self.tunnel_ids[i] = self.tunnel_ids[last];
        self.interfaces[i] = self.interfaces[last];
        self.expiries[i] = self.expiries[last];
        self.len = last;
    }
}
