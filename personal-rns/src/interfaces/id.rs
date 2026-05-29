use crate::wire::TRUNCATED_HASH_BYTE_LEN;

/// Stable identity of one interface within a Reticulum runtime.
///
/// The bytes are **opaque to the engine** - a host stamps them however
/// it likes (truncated SHA-256 of the interface's name, a peer's
/// network address, a monotonic counter, etc.) - but **must keep them
/// stable for the lifetime of the interface instance** so the engine
/// can use the id to scope per-interface state (path-table entries,
/// ingress queues, rate caps, fanout decisions).
///
/// Width is [`TRUNCATED_HASH_BYTE_LEN`] so a host that mirrors RNS's
/// [`Interface.get_hash()`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/Interface.py#L138-L139)
/// pattern (`Identity.full_hash(str(self).encode())[:16]`) lands the
/// same id Reticulum itself would have assigned. Hosts that don't care
/// about RNS parity can fill it with anything they like.
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
