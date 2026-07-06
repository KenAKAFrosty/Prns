use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::vec::Vec;

use super::soa::{DirEntry, HeldCold, Probe, SoaColumns, SoaHeldStore};
use crate::wire::DestinationHash;

pub type FixedHeapHeldStore<const CAP: usize, A = Global> =
    SoaHeldStore<FixedHeapSoaColumns<CAP, A>>;

pub struct FixedHeapSoaColumns<const CAP: usize, A: Allocator = Global> {
    probe: Vec<Probe, A>,
    dest: Vec<DestinationHash, A>,
    cold: Vec<HeldCold, A>,
    dir: Vec<Option<DirEntry>, A>,
}

impl<const CAP: usize, A: Allocator + Default> Default for FixedHeapSoaColumns<CAP, A> {
    fn default() -> Self {
        let mut dir = Vec::with_capacity_in(CAP, A::default());
        dir.resize(CAP, None);
        Self {
            probe: Vec::with_capacity_in(CAP, A::default()),
            dest: Vec::with_capacity_in(CAP, A::default()),
            cold: Vec::with_capacity_in(CAP, A::default()),
            dir,
        }
    }
}

impl<const CAP: usize, A: Allocator> SoaColumns for FixedHeapSoaColumns<CAP, A> {
    fn probe(&self) -> &[Probe] {
        &self.probe
    }

    fn dest(&self) -> &[DestinationHash] {
        &self.dest
    }

    fn cold(&self) -> &[HeldCold] {
        &self.cold
    }

    fn dir(&self) -> &[Option<DirEntry>] {
        &self.dir
    }

    fn dir_mut(&mut self) -> &mut [Option<DirEntry>] {
        &mut self.dir
    }

    fn push(&mut self, probe: Probe, dest: DestinationHash, cold: HeldCold) {
        self.probe.push(probe);
        self.dest.push(dest);
        self.cold.push(cold);
    }

    fn overwrite_at(&mut self, slot: usize, probe: Probe, dest: DestinationHash, cold: HeldCold) {
        self.probe[slot] = probe;
        self.dest[slot] = dest;
        self.cold[slot] = cold;
    }

    fn swap_remove(&mut self, slot: usize) {
        self.probe.swap_remove(slot);
        self.dest.swap_remove(slot);
        self.cold.swap_remove(slot);
    }

    fn capacity(&self) -> usize {
        CAP
    }
}
