use super::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_ordered_pages_roundtrip_wire_and_publish_exactly_once(
        ids in prop::collection::btree_set(1u8..=250, 1..13),
        page_size in 1usize..=4,
    ) {
        let (_, connection) = connected(3);
        let mut inventory = InterfaceInventory::<12>::new(connection);
        let mut request = refresh(&mut inventory);
        let ids: alloc::vec::Vec<_> = ids.into_iter().collect();
        let mut pages = ids.chunks(page_size).peekable();
        while let Some(chunk) = pages.next() {
            let continuation = if pages.peek().is_some() { more(*chunk.last().unwrap()) } else { RemoteControlInterfaceContinuation::Complete };
            let outcome = inventory.step(wire_roundtrip(receive(request, chunk, continuation)));
            if pages.peek().is_some() {
                let ReceiveInterfacePageOutcome::More { request: next } = outcome else { panic!("missing continuation"); };
                prop_assert_eq!(inventory.step(receive(request, chunk, continuation)), ReceiveInterfacePageOutcome::StaleRequest { rejected: receive(request, chunk, continuation) });
                prop_assert_eq!(inventory.step(ReadInterfaces).interfaces, None);
                request = next;
            } else {
                prop_assert_eq!(outcome, ReceiveInterfacePageOutcome::Complete { count: ids.len() });
            }
        }
        let expected = heapless::Vec::from_slice(&ids.iter().copied().map(entry).collect::<alloc::vec::Vec<_>>()).unwrap();
        prop_assert_eq!(inventory.step(ReadInterfaces), InterfaceInventorySnapshot { connection, status: InterfaceInventoryStatus::Ready, interfaces: Some(expected) });
    }
}
