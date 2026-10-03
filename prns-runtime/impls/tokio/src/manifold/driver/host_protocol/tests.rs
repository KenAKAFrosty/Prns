use std::sync::Arc;

use super::{HostResourcePayload, HostResourcePayloadError, HostResourceRecycler};

#[test]
fn host_resource_payload_supports_owned_and_shared_prefix_bytes() {
    let owned: HostResourcePayload = std::vec![1, 2, 3].into();
    assert_eq!(owned.as_slice(), &[1, 2, 3]);
    assert_eq!(owned.len(), 3);

    let shared: Arc<[u8]> = std::vec![4, 5, 6, 7].into();
    let prefix = HostResourcePayload::shared_prefix(Arc::clone(&shared), 3).unwrap();
    assert_eq!(prefix.as_slice(), &[4, 5, 6]);
    assert_eq!(prefix.len(), 3);
    assert_eq!(
        HostResourcePayload::shared_prefix(shared, 5).unwrap_err(),
        HostResourcePayloadError::PrefixOutOfRange
    );
}

#[test]
fn recyclable_payload_returns_its_allocation_after_drop() {
    let recycler = HostResourceRecycler::bounded(2);
    let mut bytes = std::vec::Vec::with_capacity(128);
    bytes.extend_from_slice(&[1, 2, 3, 4]);
    let capacity = bytes.capacity();
    let payload = HostResourcePayload::recyclable(bytes, recycler.clone());

    assert_eq!(payload.as_slice(), &[1, 2, 3, 4]);
    drop(payload);

    let recycled = recycler.take();
    assert_eq!(recycled.as_slice(), &[1, 2, 3, 4]);
    assert_eq!(recycled.capacity(), capacity);
    assert!(recycler.take().is_empty());
}

#[test]
fn recyclable_payload_drops_normally_while_the_free_list_is_contended() {
    let recycler = HostResourceRecycler::bounded(1);
    let guard = recycler.buffers.lock().unwrap();
    let payload = HostResourcePayload::recyclable(std::vec![1, 2, 3], recycler.clone());

    drop(payload);
    drop(guard);

    assert!(recycler.take().is_empty());
}

#[test]
fn recycler_retains_only_its_bounded_number_of_allocations() {
    let recycler = HostResourceRecycler::bounded(1);

    drop(HostResourcePayload::recyclable(
        std::vec![1, 2, 3],
        recycler.clone(),
    ));
    drop(HostResourcePayload::recyclable(
        std::vec![4, 5, 6],
        recycler.clone(),
    ));

    assert_eq!(recycler.take(), std::vec![1, 2, 3]);
    assert!(recycler.take().is_empty());
}
