use super::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_reconnect_histories_preserve_cancellation_and_reject_every_retired_request(
        histories in prop::collection::vec((any::<bool>(), any::<bool>(), any::<bool>()), 1..24),
    ) {
        let mut registry = registry();
        let device = create(&mut registry);
        pair(&mut registry, device, 1);
        let mut connection = begin(&mut registry, device);
        confirm(&mut registry, connection, FIRST_LINK);
        let mut interfaces = DeviceInterfaces::<4>::new(device);
        let mut request = start(&mut interfaces, &mut registry, connection, FIRST_LINK, None);
        let mut retired = alloc::vec::Vec::new();
        for (complete, close, synchronize_disconnected) in histories {
            let pending = if complete {
                prop_assert_eq!(interfaces.step(empty_response(request)), ReceiveInterfacePageOutcome::Complete { count: 0 });
                None
            } else { Some(request) };
            let pending = if close {
                prop_assert_eq!(interfaces.step(CloseInterfaceInventory), CloseInterfaceInventoryOutcome::Closed { pending });
                None
            } else { pending };
            end(&mut registry, connection, FIRST_LINK);
            let cancelled = if synchronize_disconnected {
                prop_assert_eq!(interfaces.step(SynchronizeDeviceInterfaces { registry: &mut registry }), SynchronizeDeviceInterfacesOutcome::Unavailable { cancelled: pending });
                None
            } else { pending };
            retired.push(request);
            connection = begin(&mut registry, device);
            confirm(&mut registry, connection, FIRST_LINK);
            request = start(&mut interfaces, &mut registry, connection, FIRST_LINK, cancelled);
            for &old in &retired {
                prop_assert_ne!(old, request);
                assert_stale(&mut interfaces, old);
            }
            let before = interfaces.step(ReadDeviceInterfaces);
            prop_assert_eq!(interfaces.step(SynchronizeDeviceInterfaces { registry: &mut registry }), SynchronizeDeviceInterfacesOutcome::Unchanged { connection, link: FIRST_LINK });
            prop_assert_eq!(interfaces.step(ReadDeviceInterfaces), before);
        }
    }
}
