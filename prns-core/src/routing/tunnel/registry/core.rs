use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::tunnel::TunnelId;
use crate::routing::warmth::RouteWarmth;
use crate::storage::TablePushError;

/// RNS 1.4.0 `Transport.TUNNEL_TIMEOUT` (8 hours).
pub const TUNNEL_TIMEOUT_MS: u64 = 8 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelTransition {
    Established,
    Refreshed,
    Reappeared { previous_interface: InterfaceId },
}

/// One tunnel row as it persists: the interface is the one the tunnel last rode, dead by definition after a reboot.
/// It stays in the row because it is what makes the restore work: the seeded tunnel warms that interface's routes past the departed grace, and the peer's next synthesize arrives on a different interface, which reads as a reappearance and repoints them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedTunnelRow {
    pub tunnel_id: TunnelId,
    pub interface: InterfaceId,
    pub expires_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedTunnelOutcome {
    Seeded,
    AlreadyPresent,
    TableFull,
}

pub trait TunnelTable {
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
    ) -> Result<(), TablePushError>;
    fn swap_remove(&mut self, i: usize);
}

#[derive(Debug, Default)]
pub struct Tunnels<C: TunnelTable> {
    table: C,
}

impl<C: TunnelTable> Tunnels<C> {
    fn index_of_tunnel(&self, tunnel_id: TunnelId) -> Option<usize> {
        self.table
            .tunnel_ids()
            .iter()
            .position(|candidate| *candidate == tunnel_id)
    }

    fn soonest_index(&self) -> Option<usize> {
        (0..self.table.len()).min_by_key(|&i| self.table.expiries()[i])
    }

    pub fn observe_synthesize(
        &mut self,
        tunnel_id: TunnelId,
        interface: InterfaceId,
        expires: InstantMillis,
    ) -> TunnelTransition {
        if let Some(i) = self.index_of_tunnel(tunnel_id) {
            let previous = self.table.interfaces()[i];
            self.table.set_row(i, interface, expires);
            if previous == interface {
                TunnelTransition::Refreshed
            } else {
                TunnelTransition::Reappeared {
                    previous_interface: previous,
                }
            }
        } else {
            if self.table.push(tunnel_id, interface, expires).is_err() {
                if let Some(victim) = self.soonest_index() {
                    self.table.swap_remove(victim);
                    let _ = self.table.push(tunnel_id, interface, expires);
                }
            }
            TunnelTransition::Established
        }
    }

    pub fn persisted_rows(&self) -> impl Iterator<Item = PersistedTunnelRow> + '_ {
        (0..self.table.len()).map(|i| PersistedTunnelRow {
            tunnel_id: self.table.tunnel_ids()[i],
            interface: self.table.interfaces()[i],
            expires_at: self.table.expiries()[i],
        })
    }

    /// Boot-restore counterpart of [`observe_synthesize`](Self::observe_synthesize): a live row wins over storage, and a full table refuses instead of evicting — a seed never cannibalizes rows the live network already earned.
    pub fn seed_tunnel(&mut self, row: PersistedTunnelRow) -> SeedTunnelOutcome {
        if self.index_of_tunnel(row.tunnel_id).is_some() {
            return SeedTunnelOutcome::AlreadyPresent;
        }
        match self
            .table
            .push(row.tunnel_id, row.interface, row.expires_at)
        {
            Ok(()) => SeedTunnelOutcome::Seeded,
            Err(TablePushError::TableFull) => SeedTunnelOutcome::TableFull,
        }
    }

    pub fn expire(&mut self, now: InstantMillis) -> usize {
        let mut expired = 0;
        let mut i = 0;
        while i < self.table.len() {
            if now >= self.table.expiries()[i] {
                self.table.swap_remove(i);
                expired += 1;
            } else {
                i += 1;
            }
        }
        expired
    }

    pub fn soonest_expiry(&self) -> Option<InstantMillis> {
        self.table.expiries().iter().copied().min()
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

impl<C: TunnelTable> RouteWarmth for Tunnels<C> {
    fn warm_until(&self, interface: InterfaceId) -> Option<InstantMillis> {
        self.table
            .interfaces()
            .iter()
            .position(|candidate| *candidate == interface)
            .map(|i| self.table.expiries()[i])
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn tid(byte: u8) -> TunnelId {
        TunnelId::new([byte; 32])
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn a_new_tunnel_is_established_and_warms_its_interface() {
        let mut tunnels: Tunnels<FixedTunnelTable<4>> = Tunnels::default();
        let t = tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(8000));
        assert_eq!(t, TunnelTransition::Established);
        assert_eq!(tunnels.warm_until(iface(10)), Some(InstantMillis(8000)));
        assert_eq!(tunnels.warm_until(iface(99)), None);
    }

    #[test]
    fn the_same_interface_resynthesizing_only_refreshes_the_expiry() {
        let mut tunnels: Tunnels<FixedTunnelTable<4>> = Tunnels::default();
        tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(8000));
        let t = tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(9000));
        assert_eq!(t, TunnelTransition::Refreshed);
        assert_eq!(tunnels.warm_until(iface(10)), Some(InstantMillis(9000)));
        assert_eq!(tunnels.len(), 1);
    }

    #[test]
    fn a_reappearance_reports_the_previous_interface_and_moves_the_warmth() {
        let mut tunnels: Tunnels<FixedTunnelTable<4>> = Tunnels::default();
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
        let mut tunnels: Tunnels<FixedTunnelTable<4>> = Tunnels::default();
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
        let mut tunnels: Tunnels<FixedTunnelTable<2>> = Tunnels::default();
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
        let mut tunnels: Tunnels<FixedTunnelTable<0>> = Tunnels::default();
        let t = tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(5000));
        assert_eq!(t, TunnelTransition::Established);
        assert!(tunnels.is_empty());
        assert_eq!(tunnels.warm_until(iface(10)), None);
    }

    #[test]
    fn a_seeded_tunnel_lands_verbatim_and_warms_its_interface() {
        let mut tunnels: Tunnels<FixedTunnelTable<4>> = Tunnels::default();
        let row = PersistedTunnelRow {
            tunnel_id: tid(1),
            interface: iface(10),
            expires_at: InstantMillis(8000),
        };
        assert_eq!(tunnels.seed_tunnel(row), SeedTunnelOutcome::Seeded);
        assert_eq!(tunnels.warm_until(iface(10)), Some(InstantMillis(8000)));
        assert_eq!(tunnels.persisted_rows().next(), Some(row));
    }

    #[test]
    fn a_seed_never_displaces_a_live_row() {
        let mut tunnels: Tunnels<FixedTunnelTable<4>> = Tunnels::default();
        tunnels.observe_synthesize(tid(1), iface(20), InstantMillis(9000));
        let stale = PersistedTunnelRow {
            tunnel_id: tid(1),
            interface: iface(10),
            expires_at: InstantMillis(5000),
        };
        assert_eq!(
            tunnels.seed_tunnel(stale),
            SeedTunnelOutcome::AlreadyPresent
        );
        assert_eq!(tunnels.warm_until(iface(20)), Some(InstantMillis(9000)));
        assert_eq!(tunnels.warm_until(iface(10)), None);
    }

    #[test]
    fn a_seed_refuses_a_full_table_where_a_live_synthesize_would_evict() {
        let mut tunnels: Tunnels<FixedTunnelTable<1>> = Tunnels::default();
        tunnels.observe_synthesize(tid(1), iface(10), InstantMillis(5000));
        let row = PersistedTunnelRow {
            tunnel_id: tid(2),
            interface: iface(20),
            expires_at: InstantMillis(90_000),
        };
        assert_eq!(tunnels.seed_tunnel(row), SeedTunnelOutcome::TableFull);
        assert_eq!(tunnels.warm_until(iface(10)), Some(InstantMillis(5000)));
        assert_eq!(tunnels.warm_until(iface(20)), None);
    }

    #[test]
    fn the_heap_backend_tracks_past_any_fixed_ceiling() {
        let mut tunnels: Tunnels<HeapTunnelTable> = Tunnels::default();
        for n in 0..64u8 {
            tunnels.observe_synthesize(tid(n), iface(n), InstantMillis(1000 + u64::from(n)));
        }
        assert_eq!(tunnels.len(), 64);
        assert_eq!(tunnels.warm_until(iface(17)), Some(InstantMillis(1017)));
    }
}
