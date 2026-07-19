use super::super::core::RouteWarmth;
use crate::interfaces::InterfaceId;
use crate::units::InstantMillis;

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

pub trait DepartedInterfaceTable {
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

#[derive(Debug, Default)]
pub struct DepartedInterfaces<C: DepartedInterfaceTable> {
    table: C,
}

impl<C: DepartedInterfaceTable> DepartedInterfaces<C> {
    pub fn record(&mut self, interface: InterfaceId, departure: Departure, now: InstantMillis) {
        let mut index = 0;
        while index < self.table.len() {
            if self.table.interfaces()[index] == interface || self.table.warm_untils()[index] <= now
            {
                self.table.swap_remove(index);
            } else {
                index += 1;
            }
        }
        if departure == Departure::Forgotten {
            return;
        }
        if self.table.len() >= self.table.capacity() {
            self.evict_soonest_expiring();
        }
        self.table.push(DepartedInterface {
            interface,
            warm_until: InstantMillis(now.0.saturating_add(DEPARTED_INTERFACE_GRACE_MS)),
        });
    }

    pub fn evict_expired(&mut self, now: InstantMillis) -> usize {
        let mut evicted = 0;
        while let Some(index) = self
            .table
            .warm_untils()
            .iter()
            .position(|warm_until| *warm_until <= now)
        {
            self.table.swap_remove(index);
            evicted += 1;
        }
        evicted
    }

    fn evict_soonest_expiring(&mut self) {
        let Some(index) = self
            .table
            .warm_untils()
            .iter()
            .enumerate()
            .min_by_key(|(_, warm_until)| **warm_until)
            .map(|(index, _)| index)
        else {
            return;
        };
        self.table.swap_remove(index);
    }
}

impl<C: DepartedInterfaceTable> RouteWarmth for DepartedInterfaces<C> {
    fn warm_until(&self, interface: InterfaceId) -> Option<InstantMillis> {
        self.table
            .interfaces()
            .iter()
            .position(|candidate| *candidate == interface)
            .map(|index| self.table.warm_untils()[index])
    }
}
