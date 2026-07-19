use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::routes::interface_index::RouteInterfaceIndex;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LinearRouteInterfaceIndex;

impl RouteInterfaceIndex for LinearRouteInterfaceIndex {
    fn insert(&mut self, _row: usize, _interface: InterfaceId) {}

    fn update(&mut self, _row: usize, _previous: InterfaceId, _current: InterfaceId) {}

    fn swap_remove(
        &mut self,
        _removed: usize,
        _last: usize,
        _receiving_interfaces: &[InterfaceId],
    ) {
    }

    fn route_count_via(
        &self,
        interface: InterfaceId,
        receiving_interfaces: &[InterfaceId],
    ) -> usize {
        receiving_interfaces
            .iter()
            .filter(|&&candidate| candidate == interface)
            .count()
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
        let mut moved = 0;
        for (row, interface) in receiving_interfaces.iter_mut().enumerate() {
            if *interface != previous {
                continue;
            }
            *interface = current;
            last_relayed_at[row] = now;
            moved += 1;
        }
        moved
    }
}
