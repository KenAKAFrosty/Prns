use crate::crypto::sha256;
use crate::interfaces::InterfaceKind;
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

/// The bytes of medium hash an id carries after its one kind byte. A distinct-channel
/// collision in this width is birthday-bounded against the count of *concurrent same-kind*
/// interfaces, never the input space (~1 in 14M at 100k concurrent of one kind), and the attach
/// path rejects any live collision loudly rather than aliasing two interfaces onto one id.
const MEDIUM_HASH_LEN: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InterfaceId([u8; TRUNCATED_HASH_BYTE_LEN]);

impl InterfaceId {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }

    /// Derive an id from the interface's [`InterfaceKind`] and its `medium_id` — the bytes that
    /// uniquely tag this channel within its medium (a peer MAC, a remote `ip:port`, a device id).
    /// The id is `[kind] ++ sha256(medium_id)[..7]`: stable while the medium id is (so a
    /// reconnecting peer rebinds its routes), unique across distinct channels (the medium id's
    /// job), and namespaced by kind so two kinds never cross. The one invariant the caller owes:
    /// no two *concurrent* channels may share a `medium_id`.
    #[must_use]
    pub fn from_medium(kind: InterfaceKind, medium_id: &[u8]) -> Self {
        let digest = sha256(medium_id);
        let mut bytes = [0u8; TRUNCATED_HASH_BYTE_LEN];
        bytes[0] = kind as u8;
        bytes[1..1 + MEDIUM_HASH_LEN].copy_from_slice(&digest[..MEDIUM_HASH_LEN]);
        Self(bytes)
    }
}
