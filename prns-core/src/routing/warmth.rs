//! Who may hold a route warm when its receiving interface is no longer attached.

use crate::interfaces::InterfaceId;
use crate::units::InstantMillis;

/// A source of grace for routes whose receiving interface is absent from the attached interfaces.
/// Tunnels and recent departures both answer, and the routing table holds such a route until the warmest deadline instead of culling it at once.
pub trait RouteWarmth {
    fn warm_until(&self, interface: InterfaceId) -> Option<InstantMillis>;
}

impl RouteWarmth for () {
    fn warm_until(&self, _interface: InterfaceId) -> Option<InstantMillis> {
        None
    }
}

pub struct WarmestOf<'a>(pub &'a dyn RouteWarmth, pub &'a dyn RouteWarmth);

impl RouteWarmth for WarmestOf<'_> {
    fn warm_until(&self, interface: InterfaceId) -> Option<InstantMillis> {
        match (self.0.warm_until(interface), self.1.warm_until(interface)) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Why an interface is no longer attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Departure {
    Forgotten,
    MayReturn,
}

pub const DEPARTED_INTERFACE_GRACE_MS: u64 = 5 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepartedInterface {
    pub interface: InterfaceId,
    pub warm_until: InstantMillis,
}

pub trait DepartedInterfaceColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn interfaces(&self) -> &[InterfaceId];
    fn warm_untils(&self) -> &[InstantMillis];
    fn push(&mut self, entry: DepartedInterface);
    fn swap_remove(&mut self, index: usize);
}

/// A deliberate deviation from RNS 1.3.5, which culls a departed interface's routes at the next `Transport.jobs` pass.
///
/// The reference's interface identity is a live object, so a reconnecting peer arrives as a stranger.
/// But our [`InterfaceId`]s derive from the medium, so a fleet member that bounces (a BLE peer drifting out of range, a WiFi peer roaming) returns as itself.
/// This also applies to things like LoRa radio settings: switching to a different frequency and back within the grace keeps the routes learned on the original frequency.
///
/// Holding the announce-learned routes warm for these situations makes the reconnect seamless instead of waiting for a re-announce (all without requiring explicit use of tunnels on every medium).
#[derive(Debug, Default)]
pub struct DepartedInterfaces<C: DepartedInterfaceColumns> {
    columns: C,
}

impl<C: DepartedInterfaceColumns> DepartedInterfaces<C> {
    pub fn record(&mut self, interface: InterfaceId, departure: Departure, now: InstantMillis) {
        let mut index = 0;
        while index < self.columns.len() {
            if self.columns.interfaces()[index] == interface
                || self.columns.warm_untils()[index] <= now
            {
                self.columns.swap_remove(index);
            } else {
                index += 1;
            }
        }
        if departure == Departure::Forgotten {
            return;
        }
        if self.columns.len() >= self.columns.capacity() {
            self.evict_soonest_expiring();
        }
        self.columns.push(DepartedInterface {
            interface,
            warm_until: InstantMillis(now.0.saturating_add(DEPARTED_INTERFACE_GRACE_MS)),
        });
    }

    pub fn evict_expired(&mut self, now: InstantMillis) {
        while let Some(index) = self
            .columns
            .warm_untils()
            .iter()
            .position(|warm_until| *warm_until <= now)
        {
            self.columns.swap_remove(index);
        }
    }

    fn evict_soonest_expiring(&mut self) {
        let Some(index) = self
            .columns
            .warm_untils()
            .iter()
            .enumerate()
            .min_by_key(|(_, warm_until)| **warm_until)
            .map(|(index, _)| index)
        else {
            return;
        };
        self.columns.swap_remove(index);
    }
}

impl<C: DepartedInterfaceColumns> RouteWarmth for DepartedInterfaces<C> {
    fn warm_until(&self, interface: InterfaceId) -> Option<InstantMillis> {
        self.columns
            .interfaces()
            .iter()
            .position(|candidate| *candidate == interface)
            .map(|index| self.columns.warm_untils()[index])
    }
}

#[derive(Debug)]
pub struct FixedDepartedInterfaceColumns<const MAX_DEPARTED_INTERFACES: usize> {
    len: usize,
    interfaces: [InterfaceId; MAX_DEPARTED_INTERFACES],
    warm_untils: [InstantMillis; MAX_DEPARTED_INTERFACES],
}

impl<const MAX_DEPARTED_INTERFACES: usize> Default
    for FixedDepartedInterfaceColumns<MAX_DEPARTED_INTERFACES>
{
    fn default() -> Self {
        Self {
            len: 0,
            interfaces: [InterfaceId::new([0u8; 8]); MAX_DEPARTED_INTERFACES],
            warm_untils: [InstantMillis(0); MAX_DEPARTED_INTERFACES],
        }
    }
}

impl<const MAX_DEPARTED_INTERFACES: usize> DepartedInterfaceColumns
    for FixedDepartedInterfaceColumns<MAX_DEPARTED_INTERFACES>
{
    fn capacity(&self) -> usize {
        MAX_DEPARTED_INTERFACES
    }
    fn len(&self) -> usize {
        self.len
    }

    fn interfaces(&self) -> &[InterfaceId] {
        &self.interfaces[..self.len]
    }
    fn warm_untils(&self) -> &[InstantMillis] {
        &self.warm_untils[..self.len]
    }

    fn push(&mut self, entry: DepartedInterface) {
        if self.len >= MAX_DEPARTED_INTERFACES {
            return;
        }
        let i = self.len;
        self.interfaces[i] = entry.interface;
        self.warm_untils[i] = entry.warm_until;
        self.len += 1;
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.interfaces[index] = self.interfaces[last];
            self.warm_untils[index] = self.warm_untils[last];
        }
        self.len = last;
    }
}

/// Departures dedup by interface id, so the ledger's size is the count of distinct
/// interfaces departed within the grace window; a daemon-grade cap keeps a hostile
/// connect-churn flood from ballooning memory.
#[cfg(feature = "alloc")]
pub const DEFAULT_MAX_DEPARTED_INTERFACES: usize = 1024;

#[cfg(feature = "alloc")]
#[derive(Debug, Default)]
pub struct HeapDepartedInterfaceColumns {
    interfaces: alloc::vec::Vec<InterfaceId>,
    warm_untils: alloc::vec::Vec<InstantMillis>,
}

#[cfg(feature = "alloc")]
impl DepartedInterfaceColumns for HeapDepartedInterfaceColumns {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_DEPARTED_INTERFACES
    }
    fn len(&self) -> usize {
        self.interfaces.len()
    }

    fn interfaces(&self) -> &[InterfaceId] {
        &self.interfaces
    }
    fn warm_untils(&self) -> &[InstantMillis] {
        &self.warm_untils
    }

    fn push(&mut self, entry: DepartedInterface) {
        if self.len() >= self.capacity() {
            return;
        }
        self.interfaces.push(entry.interface);
        self.warm_untils.push(entry.warm_until);
    }

    fn swap_remove(&mut self, index: usize) {
        self.interfaces.swap_remove(index);
        self.warm_untils.swap_remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Ledger = DepartedInterfaces<FixedDepartedInterfaceColumns<4>>;

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn the_warmest_of_two_sources_wins() {
        struct At(u64);
        impl RouteWarmth for At {
            fn warm_until(&self, _interface: InterfaceId) -> Option<InstantMillis> {
                Some(InstantMillis(self.0))
            }
        }
        assert_eq!(
            WarmestOf(&At(5_000), &At(9_000)).warm_until(iface(1)),
            Some(InstantMillis(9_000)),
        );
        assert_eq!(
            WarmestOf(&(), &At(9_000)).warm_until(iface(1)),
            Some(InstantMillis(9_000)),
        );
        assert_eq!(WarmestOf(&(), &()).warm_until(iface(1)), None);
    }

    #[test]
    fn a_may_return_departure_is_warm_for_the_grace_and_a_forgotten_one_is_not() {
        let mut ledger = Ledger::default();
        ledger.record(iface(1), Departure::MayReturn, InstantMillis(1_000));
        assert_eq!(
            ledger.warm_until(iface(1)),
            Some(InstantMillis(1_000 + DEPARTED_INTERFACE_GRACE_MS)),
        );

        ledger.record(iface(1), Departure::Forgotten, InstantMillis(2_000));
        assert_eq!(
            ledger.warm_until(iface(1)),
            None,
            "a deliberate forget revokes the earlier bounce's grace",
        );
    }

    #[test]
    fn a_repeat_departure_re_arms_the_grace_instead_of_stacking_rows() {
        let mut ledger = Ledger::default();
        ledger.record(iface(1), Departure::MayReturn, InstantMillis(1_000));
        ledger.record(iface(1), Departure::MayReturn, InstantMillis(50_000));
        assert_eq!(
            ledger.warm_until(iface(1)),
            Some(InstantMillis(50_000 + DEPARTED_INTERFACE_GRACE_MS)),
        );
    }

    #[test]
    fn a_full_ledger_evicts_the_soonest_expiring_row_for_the_newcomer() {
        let mut ledger = Ledger::default();
        for n in 0..4u8 {
            ledger.record(
                iface(n),
                Departure::MayReturn,
                InstantMillis(1_000 + u64::from(n)),
            );
        }
        ledger.record(iface(0xFF), Departure::MayReturn, InstantMillis(2_000));
        assert_eq!(
            ledger.warm_until(iface(0)),
            None,
            "the row closest to expiry made room",
        );
        assert!(ledger.warm_until(iface(0xFF)).is_some());
        assert!(ledger.warm_until(iface(1)).is_some());
    }

    #[test]
    fn expired_rows_are_swept() {
        let mut ledger = Ledger::default();
        ledger.record(iface(1), Departure::MayReturn, InstantMillis(1_000));
        ledger.evict_expired(InstantMillis(1_000 + DEPARTED_INTERFACE_GRACE_MS));
        assert_eq!(ledger.warm_until(iface(1)), None);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn heap_columns_hold_a_mass_departure_no_fixed_ledger_could() {
        let mut ledger: DepartedInterfaces<HeapDepartedInterfaceColumns> =
            DepartedInterfaces::default();
        for n in 0..64u8 {
            ledger.record(iface(n), Departure::MayReturn, InstantMillis(1_000));
        }
        assert!(ledger.warm_until(iface(0)).is_some());
        assert!(ledger.warm_until(iface(63)).is_some());
    }
}
