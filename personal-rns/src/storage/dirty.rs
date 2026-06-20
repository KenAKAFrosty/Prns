use crate::interfaces::InterfaceId;

pub trait DirtyInterfaceSet {
    fn mark(&mut self, interface: InterfaceId);
    fn drain(&mut self, visit: impl FnMut(InterfaceId));
}

#[cfg(feature = "alloc")]
impl DirtyInterfaceSet for alloc::collections::BTreeSet<InterfaceId> {
    fn mark(&mut self, interface: InterfaceId) {
        self.insert(interface);
    }

    fn drain(&mut self, mut visit: impl FnMut(InterfaceId)) {
        for interface in core::mem::take(self) {
            visit(interface);
        }
    }
}

impl<const N: usize> DirtyInterfaceSet for heapless::Vec<InterfaceId, N> {
    fn mark(&mut self, interface: InterfaceId) {
        if !self.contains(&interface) {
            let _ = self.push(interface);
        }
    }

    fn drain(&mut self, mut visit: impl FnMut(InterfaceId)) {
        for interface in core::mem::take(self) {
            visit(interface);
        }
    }
}
