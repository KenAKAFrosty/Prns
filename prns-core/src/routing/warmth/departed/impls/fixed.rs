use super::super::core::{DepartedInterface, DepartedInterfaceTable};
use crate::interfaces::InterfaceId;
use crate::units::InstantMillis;

#[derive(Debug)]
pub struct FixedDepartedInterfaceTable<const MAX_DEPARTED_INTERFACES: usize> {
    len: usize,
    interfaces: [InterfaceId; MAX_DEPARTED_INTERFACES],
    warm_untils: [InstantMillis; MAX_DEPARTED_INTERFACES],
}

impl<const MAX_DEPARTED_INTERFACES: usize> Default
    for FixedDepartedInterfaceTable<MAX_DEPARTED_INTERFACES>
{
    fn default() -> Self {
        Self {
            len: 0,
            interfaces: [InterfaceId::new([0u8; 8]); MAX_DEPARTED_INTERFACES],
            warm_untils: [InstantMillis(0); MAX_DEPARTED_INTERFACES],
        }
    }
}

impl<const MAX_DEPARTED_INTERFACES: usize> DepartedInterfaceTable
    for FixedDepartedInterfaceTable<MAX_DEPARTED_INTERFACES>
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
