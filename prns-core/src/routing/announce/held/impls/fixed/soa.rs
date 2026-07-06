use crate::interfaces::InterfaceId;
use crate::routing::announce::held::{
    HeldAnnounce, HeldFull, HeldStore, MAX_HELD_ANNOUNCES_PER_INTERFACE,
};
use crate::routing::announce::stored::{AnnounceRecord, AppDataHandle};
use crate::routing::NextHop;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub iface_idx: u16,
    pub hops: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirEntry {
    pub interface: InterfaceId,
    pub held: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldCold {
    pub next_hop: NextHop,
    pub is_path_response: bool,
    pub entry: AnnounceRecord,
}

pub trait SoaColumns {
    fn probe(&self) -> &[Probe];
    fn dest(&self) -> &[DestinationHash];
    fn cold(&self) -> &[HeldCold];
    fn dir(&self) -> &[Option<DirEntry>];
    fn dir_mut(&mut self) -> &mut [Option<DirEntry>];
    fn push(&mut self, probe: Probe, dest: DestinationHash, cold: HeldCold);
    fn overwrite_at(&mut self, slot: usize, probe: Probe, dest: DestinationHash, cold: HeldCold);
    fn swap_remove(&mut self, slot: usize);
    fn capacity(&self) -> usize;

    fn len(&self) -> usize {
        self.probe().len()
    }

    fn is_empty(&self) -> bool {
        self.probe().is_empty()
    }
}

#[derive(Debug, Default)]
pub struct SoaHeldStore<C: SoaColumns> {
    columns: C,
}

impl<C: SoaColumns> SoaHeldStore<C> {
    fn iface_idx(&self, interface: InterfaceId) -> Option<usize> {
        self.columns
            .dir()
            .iter()
            .position(|slot| matches!(slot, Some(entry) if entry.interface == interface))
    }

    fn claim_iface_idx(&mut self, interface: InterfaceId) -> Option<usize> {
        if let Some(idx) = self.iface_idx(interface) {
            return Some(idx);
        }
        let free = self.columns.dir().iter().position(Option::is_none)?;
        self.columns.dir_mut()[free] = Some(DirEntry { interface, held: 0 });
        Some(free)
    }

    fn reconstruct(&self, slot: usize, interface: InterfaceId) -> HeldAnnounce {
        let cold = self.columns.cold()[slot];
        HeldAnnounce {
            destination: self.columns.dest()[slot],
            hops: self.columns.probe()[slot].hops,
            receiving_interface: interface,
            next_hop: cold.next_hop,
            is_path_response: cold.is_path_response,
            announce: cold.entry,
        }
    }
}

fn cold_of(record: &HeldAnnounce) -> HeldCold {
    HeldCold {
        next_hop: record.next_hop,
        is_path_response: record.is_path_response,
        entry: record.announce,
    }
}

impl<C: SoaColumns> HeldStore for SoaHeldStore<C> {
    type Slot = usize;

    fn find(&self, interface: InterfaceId, destination: DestinationHash) -> Option<usize> {
        let iface_idx = self.iface_idx(interface)? as u16;
        let probe = self.columns.probe();
        let dest = self.columns.dest();
        (0..probe.len()).find(|&i| probe[i].iface_idx == iface_idx && dest[i] == destination)
    }

    fn app_data_handle(&self, slot: usize) -> Option<AppDataHandle> {
        self.columns.cold()[slot].entry.maybe_app_data_handle
    }

    fn overwrite(&mut self, slot: usize, record: HeldAnnounce) {
        let iface_idx = self.columns.probe()[slot].iface_idx;
        self.columns.overwrite_at(
            slot,
            Probe {
                iface_idx,
                hops: record.hops,
            },
            record.destination,
            cold_of(&record),
        );
    }

    fn insert(&mut self, record: HeldAnnounce) -> Result<(), HeldFull> {
        let idx = self
            .claim_iface_idx(record.receiving_interface)
            .ok_or(HeldFull::PoolFull)?;
        let held = self.columns.dir()[idx].map_or(0, |entry| entry.held);
        if held as usize >= MAX_HELD_ANNOUNCES_PER_INTERFACE {
            return Err(HeldFull::InterfaceAtCap);
        }
        if self.columns.len() >= self.columns.capacity() {
            if held == 0 {
                self.columns.dir_mut()[idx] = None;
            }
            return Err(HeldFull::PoolFull);
        }
        self.columns.push(
            Probe {
                iface_idx: idx as u16,
                hops: record.hops,
            },
            record.destination,
            cold_of(&record),
        );
        if let Some(entry) = self.columns.dir_mut()[idx].as_mut() {
            entry.held += 1;
        }
        Ok(())
    }

    fn take_lowest_hop_for(&mut self, interface: InterfaceId) -> Option<HeldAnnounce> {
        let idx = self.iface_idx(interface)?;
        let iface_idx = idx as u16;
        let probe = self.columns.probe();
        let slot = (0..probe.len())
            .filter(|&i| probe[i].iface_idx == iface_idx)
            .min_by_key(|&i| probe[i].hops)?;
        let record = self.reconstruct(slot, interface);
        self.columns.swap_remove(slot);
        if let Some(entry) = self.columns.dir_mut()[idx].as_mut() {
            entry.held -= 1;
            if entry.held == 0 {
                self.columns.dir_mut()[idx] = None;
            }
        }
        Some(record)
    }

    fn drop_interface(
        &mut self,
        interface: InterfaceId,
        mut on_removed: impl FnMut(Option<AppDataHandle>),
    ) {
        let Some(idx) = self.iface_idx(interface) else {
            return;
        };
        let iface_idx = idx as u16;
        let mut i = 0;
        while i < self.columns.len() {
            if self.columns.probe()[i].iface_idx == iface_idx {
                on_removed(self.columns.cold()[i].entry.maybe_app_data_handle);
                self.columns.swap_remove(i);
            } else {
                i += 1;
            }
        }
        self.columns.dir_mut()[idx] = None;
    }

    fn interfaces(&self) -> impl Iterator<Item = InterfaceId> + '_ {
        self.columns
            .dir()
            .iter()
            .filter_map(|slot| slot.map(|entry| entry.interface))
    }

    fn len(&self) -> usize {
        self.columns.len()
    }
}
