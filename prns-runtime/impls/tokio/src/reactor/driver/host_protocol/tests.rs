use std::sync::Arc;

use super::{HostResourcePayload, HostResourcePayloadError};

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
