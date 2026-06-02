use crate::wire::TRUNCATED_HASH_BYTE_LEN;

/// Stable identity of one interface within a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InterfaceId([u8; TRUNCATED_HASH_BYTE_LEN]);

impl InterfaceId {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}
