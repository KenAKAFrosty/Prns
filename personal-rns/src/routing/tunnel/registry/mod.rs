mod impls;

pub use impls::*;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::tunnel::TunnelId;
use crate::storage::ColumnsFull;

pub const TUNNEL_TIMEOUT_MS: u64 = 8 * 60 * 60 * 1000;

pub trait TunnelWarmth {
    fn warm_until(&self, interface: InterfaceId) -> Option<InstantMillis>;
}

impl TunnelWarmth for () {
    fn warm_until(&self, _interface: InterfaceId) -> Option<InstantMillis> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelTransition {
    Established,
    Refreshed,
    Reappeared { previous_interface: InterfaceId },
}

pub trait TunnelColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn tunnel_ids(&self) -> &[TunnelId];
    fn interfaces(&self) -> &[InterfaceId];
    fn expiries(&self) -> &[InstantMillis];

    fn set_row(&mut self, i: usize, interface: InterfaceId, expires: InstantMillis);
    fn push(
        &mut self,
        tunnel_id: TunnelId,
        interface: InterfaceId,
        expires: InstantMillis,
    ) -> Result<(), ColumnsFull>;
    fn swap_remove(&mut self, i: usize);
}

#[derive(Debug, Default)]
pub struct Tunnels<C: TunnelColumns> {
    columns: C,
}

impl<C: TunnelColumns> Tunnels<C> {
    fn index_of_tunnel(&self, tunnel_id: TunnelId) -> Option<usize> {
        self.columns
            .tunnel_ids()
            .iter()
            .position(|candidate| *candidate == tunnel_id)
    }

    fn soonest_index(&self) -> Option<usize> {
        (0..self.columns.len()).min_by_key(|&i| self.columns.expiries()[i])
    }

    pub fn observe_synthesize(
        &mut self,
        tunnel_id: TunnelId,
        interface: InterfaceId,
        expires: InstantMillis,
    ) -> TunnelTransition {
        if let Some(i) = self.index_of_tunnel(tunnel_id) {
            let previous = self.columns.interfaces()[i];
            self.columns.set_row(i, interface, expires);
            if previous == interface {
                TunnelTransition::Refreshed
            } else {
                TunnelTransition::Reappeared {
                    previous_interface: previous,
                }
            }
        } else {
            if self.columns.push(tunnel_id, interface, expires).is_err() {
                if let Some(victim) = self.soonest_index() {
                    self.columns.swap_remove(victim);
                    let _ = self.columns.push(tunnel_id, interface, expires);
                }
            }
            TunnelTransition::Established
        }
    }

    pub fn expire(&mut self, now: InstantMillis) -> usize {
        let mut expired = 0;
        let mut i = 0;
        while i < self.columns.len() {
            if now >= self.columns.expiries()[i] {
                self.columns.swap_remove(i);
                expired += 1;
            } else {
                i += 1;
            }
        }
        expired
    }

    pub fn soonest_expiry(&self) -> Option<InstantMillis> {
        self.columns.expiries().iter().copied().min()
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

impl<C: TunnelColumns> TunnelWarmth for Tunnels<C> {
    fn warm_until(&self, interface: InterfaceId) -> Option<InstantMillis> {
        self.columns
            .interfaces()
            .iter()
            .position(|candidate| *candidate == interface)
            .map(|i| self.columns.expiries()[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid(byte: u8) -> TunnelId {
        TunnelId::new([byte; 32])
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn a_new_tunnel_is_established_and_warms_its_interface() {
        let mut tunnels: Tunnels<FixedTunnelColumns<4>> = Tunnels::default();
        let t = tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(8000));
        assert_eq!(t, TunnelTransition::Established);
        assert_eq!(tunnels.warm_until(iface(10)), Some(InstantMillis(8000)));
        assert_eq!(tunnels.warm_until(iface(99)), None);
    }

    #[test]
    fn the_same_interface_resynthesizing_only_refreshes_the_expiry() {
        let mut tunnels: Tunnels<FixedTunnelColumns<4>> = Tunnels::default();
        tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(8000));
        let t = tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(9000));
        assert_eq!(t, TunnelTransition::Refreshed);
        assert_eq!(tunnels.warm_until(iface(10)), Some(InstantMillis(9000)));
        assert_eq!(tunnels.len(), 1);
    }

    #[test]
    fn a_reappearance_reports_the_previous_interface_and_moves_the_warmth() {
        let mut tunnels: Tunnels<FixedTunnelColumns<4>> = Tunnels::default();
        tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(8000));
        let t = tunnels.observe_synthesize(tid(1), iface(20), InstantMillis(16000));
        assert_eq!(
            t,
            TunnelTransition::Reappeared {
                previous_interface: iface(10),
            }
        );
        assert_eq!(tunnels.warm_until(iface(10)), None);
        assert_eq!(tunnels.warm_until(iface(20)), Some(InstantMillis(16000)));
        assert_eq!(tunnels.len(), 1);
    }

    #[test]
    fn expiry_forgets_timed_out_tunnels_and_keeps_live_ones() {
        let mut tunnels: Tunnels<FixedTunnelColumns<4>> = Tunnels::default();
        tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(5000));
        tunnels.observe_synthesize(tid(2), iface(20), InstantMillis(15000));
        assert_eq!(tunnels.soonest_expiry(), Some(InstantMillis(5000)));

        let gone = tunnels.expire(InstantMillis(10000));
        assert_eq!(gone, 1);
        assert_eq!(tunnels.warm_until(iface(10)), None);
        assert_eq!(tunnels.warm_until(iface(20)), Some(InstantMillis(15000)));
    }

    #[test]
    fn a_full_table_evicts_the_soonest_expiring_to_admit_a_fresh_tunnel() {
        let mut tunnels: Tunnels<FixedTunnelColumns<2>> = Tunnels::default();
        tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(5000));
        tunnels.observe_synthesize(tid(2), iface(20), InstantMillis(9000));
        tunnels.observe_synthesize(tid(3), iface(30), InstantMillis(12000));
        assert_eq!(tunnels.len(), 2);
        assert_eq!(tunnels.warm_until(iface(10)), None);
        assert_eq!(tunnels.warm_until(iface(20)), Some(InstantMillis(9000)));
        assert_eq!(tunnels.warm_until(iface(30)), Some(InstantMillis(12000)));
    }

    #[test]
    fn a_zero_capacity_table_tracks_nothing() {
        let mut tunnels: Tunnels<FixedTunnelColumns<0>> = Tunnels::default();
        let t = tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(5000));
        assert_eq!(t, TunnelTransition::Established);
        assert!(tunnels.is_empty());
        assert_eq!(tunnels.warm_until(iface(10)), None);
    }

    #[test]
    fn the_heap_backend_tracks_past_any_fixed_ceiling() {
        let mut tunnels: Tunnels<HeapTunnelColumns> = Tunnels::default();
        for n in 0..64u8 {
            tunnels.observe_synthesize(tid(n), iface(n), InstantMillis(1000 + u64::from(n)));
        }
        assert_eq!(tunnels.len(), 64);
        assert_eq!(tunnels.warm_until(iface(17)), Some(InstantMillis(1017)));
    }
}
