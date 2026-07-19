use super::super::core::{DepartedInterface, DepartedInterfaceTable};
use crate::interfaces::InterfaceId;
use crate::units::InstantMillis;

#[derive(Debug, Default)]
pub struct HeapDepartedInterfaceTable {
    interfaces: alloc::vec::Vec<InterfaceId>,
    warm_untils: alloc::vec::Vec<InstantMillis>,
}

impl DepartedInterfaceTable for HeapDepartedInterfaceTable {
    fn capacity(&self) -> usize {
        usize::MAX
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
        self.interfaces.push(entry.interface);
        self.warm_untils.push(entry.warm_until);
    }

    fn swap_remove(&mut self, index: usize) {
        self.interfaces.swap_remove(index);
        self.warm_untils.swap_remove(index);
    }
}
