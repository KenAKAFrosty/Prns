use alloc::vec::Vec;

use super::super::{HeldAnnounce, HeldAnnounceTable, HeldFull, MAX_HELD_ANNOUNCES_PER_INTERFACE};
use crate::interfaces::InterfaceId;
use crate::routing::announce::stored::AppDataHandle;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapHeldAnnounceTable {
    interfaces: Vec<InterfaceHeld>,
}

#[derive(Debug)]
struct InterfaceHeld {
    interface: InterfaceId,
    held: Vec<HeldAnnounce>,
}

impl HeldAnnounceTable for HeapHeldAnnounceTable {
    type Slot = (usize, usize);

    fn find(&self, interface: InterfaceId, destination: DestinationHash) -> Option<(usize, usize)> {
        let iface = self
            .interfaces
            .iter()
            .position(|held| held.interface == interface)?;
        let slot = self.interfaces[iface]
            .held
            .iter()
            .position(|record| record.destination == destination)?;
        Some((iface, slot))
    }

    fn app_data_handle(&self, (iface, slot): (usize, usize)) -> Option<AppDataHandle> {
        self.interfaces[iface].held[slot]
            .announce
            .maybe_app_data_handle
    }

    fn overwrite(&mut self, (iface, slot): (usize, usize), record: HeldAnnounce) {
        self.interfaces[iface].held[slot] = record;
    }

    fn insert(&mut self, record: HeldAnnounce) -> Result<(), HeldFull> {
        match self
            .interfaces
            .iter_mut()
            .find(|held| held.interface == record.receiving_interface)
        {
            Some(held) => {
                if held.held.len() >= MAX_HELD_ANNOUNCES_PER_INTERFACE {
                    return Err(HeldFull::InterfaceAtCap);
                }
                held.held.push(record);
            }
            None => self.interfaces.push(InterfaceHeld {
                interface: record.receiving_interface,
                held: alloc::vec![record],
            }),
        }
        Ok(())
    }

    fn take_lowest_hop_for(&mut self, interface: InterfaceId) -> Option<HeldAnnounce> {
        let iface = self
            .interfaces
            .iter()
            .position(|held| held.interface == interface)?;
        let slot = self.interfaces[iface]
            .held
            .iter()
            .enumerate()
            .min_by_key(|(_, record)| record.hops)
            .map(|(i, _)| i)?;
        let record = self.interfaces[iface].held.swap_remove(slot);
        if self.interfaces[iface].held.is_empty() {
            self.interfaces.swap_remove(iface);
        }
        Some(record)
    }

    fn drop_interface(
        &mut self,
        interface: InterfaceId,
        mut on_removed: impl FnMut(Option<AppDataHandle>),
    ) {
        let Some(iface) = self
            .interfaces
            .iter()
            .position(|held| held.interface == interface)
        else {
            return;
        };
        for record in &self.interfaces[iface].held {
            on_removed(record.announce.maybe_app_data_handle);
        }
        self.interfaces.swap_remove(iface);
    }

    fn interfaces(&self) -> impl Iterator<Item = InterfaceId> + '_ {
        self.interfaces.iter().map(|held| held.interface)
    }

    fn len(&self) -> usize {
        self.interfaces.iter().map(|held| held.held.len()).sum()
    }
}
