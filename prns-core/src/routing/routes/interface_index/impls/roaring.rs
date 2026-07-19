use alloc::vec::Vec;
use core::mem;

use roaring::RoaringBitmap;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::lemire_index::{HeapLemireIndex, IndexRow};
use crate::routing::routes::interface_index::RouteInterfaceIndex;

#[derive(Debug)]
struct InterfaceRouteGroup {
    interface: InterfaceId,
    rows: RoaringBitmap,
}

impl InterfaceRouteGroup {
    fn new(interface: InterfaceId) -> Self {
        Self {
            interface,
            rows: RoaringBitmap::new(),
        }
    }
}

impl IndexRow for InterfaceRouteGroup {
    type Key = InterfaceId;

    fn index_key(&self) -> &Self::Key {
        &self.interface
    }
}

#[derive(Debug, Default)]
pub struct RoaringRouteInterfaceIndex {
    groups: Vec<InterfaceRouteGroup>,
    index: HeapLemireIndex,
}

impl RoaringRouteInterfaceIndex {
    fn row_id(row: usize) -> u32 {
        assert!(
            row < HeapLemireIndex::MAX_ROWS,
            "route row exceeds the roaring index address space"
        );
        row as u32
    }

    fn group_for(&self, interface: InterfaceId) -> Option<usize> {
        self.index.get(&interface, &self.groups)
    }

    fn group_for_or_insert(&mut self, interface: InterfaceId) -> usize {
        if let Some(group) = self.group_for(interface) {
            return group;
        }
        let group = self.groups.len();
        self.groups.push(InterfaceRouteGroup::new(interface));
        self.index.insert(group, &self.groups);
        group
    }

    fn remove_group(&mut self, group: usize) {
        let last = self.groups.len() - 1;
        let removed = self.groups[group].interface;
        self.index.remove(&removed, &self.groups);
        if group != last {
            let moved = self.groups[last].interface;
            self.index.repoint(&moved, group, &self.groups);
        }
        self.groups.swap_remove(group);
    }

    fn insert_row(&mut self, row: usize, interface: InterfaceId) {
        let row = Self::row_id(row);
        let group = self.group_for_or_insert(interface);
        let inserted = self.groups[group].rows.insert(row);
        debug_assert!(inserted);
    }

    fn remove_row(&mut self, row: usize, interface: InterfaceId) -> bool {
        let Some(group) = self.group_for(interface) else {
            return false;
        };
        if !self.groups[group].rows.remove(Self::row_id(row)) {
            return false;
        }
        if self.groups[group].rows.is_empty() {
            self.remove_group(group);
        }
        true
    }

    fn take_rows(&mut self, interface: InterfaceId) -> Option<RoaringBitmap> {
        let group = self.group_for(interface)?;
        let rows = mem::take(&mut self.groups[group].rows);
        self.remove_group(group);
        Some(rows)
    }

    fn merge_rows(&mut self, interface: InterfaceId, rows: RoaringBitmap) {
        if rows.is_empty() {
            return;
        }
        let group = self.group_for_or_insert(interface);
        self.groups[group].rows |= rows;
    }
}

impl RouteInterfaceIndex for RoaringRouteInterfaceIndex {
    fn insert(&mut self, row: usize, interface: InterfaceId) {
        self.insert_row(row, interface);
    }

    fn update(&mut self, row: usize, previous: InterfaceId, current: InterfaceId) {
        if previous == current {
            return;
        }
        assert!(
            self.remove_row(row, previous),
            "route interface membership missing during update"
        );
        self.insert_row(row, current);
    }

    fn swap_remove(&mut self, removed: usize, last: usize, receiving_interfaces: &[InterfaceId]) {
        assert!(
            self.remove_row(removed, receiving_interfaces[removed]),
            "route interface membership missing during removal"
        );
        if removed == last {
            return;
        }
        let moved_interface = receiving_interfaces[last];
        assert!(
            self.remove_row(last, moved_interface),
            "moved route interface membership missing during removal"
        );
        self.insert_row(removed, moved_interface);
    }

    fn route_count_via(
        &self,
        interface: InterfaceId,
        _receiving_interfaces: &[InterfaceId],
    ) -> usize {
        self.group_for(interface)
            .map(|group| self.groups[group].rows.len() as usize)
            .unwrap_or(0)
    }

    fn repoint_receiving_interface(
        &mut self,
        previous: InterfaceId,
        current: InterfaceId,
        now: InstantMillis,
        receiving_interfaces: &mut [InterfaceId],
        last_relayed_at: &mut [InstantMillis],
    ) -> usize {
        debug_assert_eq!(receiving_interfaces.len(), last_relayed_at.len());
        let Some(rows) = self.take_rows(previous) else {
            return 0;
        };
        let moved = rows.len() as usize;
        for row in rows.iter() {
            let row = row as usize;
            receiving_interfaces[row] = current;
            last_relayed_at[row] = now;
        }
        self.merge_rows(current, rows);
        moved
    }
}
