use crate::wire::TRUNCATED_HASH_BYTE_LEN;

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

#[cfg(feature = "std")]
impl InterfaceId {
    /// Mint a process-unique id from one shared counter, so a host interface never has to
    /// invent a unique value (the way the runtime mints every `CommandId`). The counter starts
    /// at 1, so a minted id never collides with the all-zero sentinel a fixed routing column
    /// fills its empty slots with.
    #[must_use]
    pub fn mint() -> Self {
        use core::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut bytes = [0u8; TRUNCATED_HASH_BYTE_LEN];
        bytes[..8].copy_from_slice(&serial.to_be_bytes());
        Self(bytes)
    }
}
