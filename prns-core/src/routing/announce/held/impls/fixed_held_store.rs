use heapless::Vec;

use super::soa::{DirEntry, HeldCold, Probe, SoaColumns, SoaHeldStore};
use crate::wire::DestinationHash;

pub type FixedHeldStore<const CAP: usize> = SoaHeldStore<FixedSoaColumns<CAP>>;

#[derive(Debug)]
pub struct FixedSoaColumns<const CAP: usize> {
    probe: Vec<Probe, CAP>,
    dest: Vec<DestinationHash, CAP>,
    cold: Vec<HeldCold, CAP>,
    dir: [Option<DirEntry>; CAP],
}

impl<const CAP: usize> Default for FixedSoaColumns<CAP> {
    fn default() -> Self {
        Self {
            probe: Vec::new(),
            dest: Vec::new(),
            cold: Vec::new(),
            dir: [None; CAP],
        }
    }
}

impl<const CAP: usize> SoaColumns for FixedSoaColumns<CAP> {
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
        let _ = self.probe.push(probe);
        let _ = self.dest.push(dest);
        let _ = self.cold.push(cold);
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
