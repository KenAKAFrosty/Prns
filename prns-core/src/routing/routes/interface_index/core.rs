use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;

pub trait RouteInterfaceIndex: Default {
    fn insert(&mut self, row: usize, interface: InterfaceId);

    fn update(&mut self, row: usize, previous: InterfaceId, current: InterfaceId);

    fn swap_remove(&mut self, removed: usize, last: usize, receiving_interfaces: &[InterfaceId]);

    fn route_count_via(
        &self,
        interface: InterfaceId,
        receiving_interfaces: &[InterfaceId],
    ) -> usize;

    fn repoint_receiving_interface(
        &mut self,
        previous: InterfaceId,
        current: InterfaceId,
        now: InstantMillis,
        receiving_interfaces: &mut [InterfaceId],
        last_relayed_at: &mut [InstantMillis],
    ) -> usize;
}
