use crate::crypto::sha256;
use crate::interfaces::InterfaceKind;

/// The bytes of channel-tag hash an id carries after its one kind byte. A distinct-channel
/// collision in this width is birthday-bounded against the count of *concurrent same-kind*
/// interfaces, never the input space (~1 in 14M at 100k concurrent of one kind), and the attach
/// path rejects any live collision loudly rather than aliasing two interfaces onto one id.
const CHANNEL_TAG_HASH_LEN: usize = 7;

/// An interface id is exactly one kind byte followed by `CHANNEL_TAG_HASH_LEN` hash bytes —
/// eight in all. It used to be sixteen (the engine's truncated-hash width), but the top eight bytes
/// were always zero padding; an id is its own namespace, not a destination hash, so it carries only
/// the bytes it needs. Halving it lightens every retained route, reverse route, and member slot that
/// stores one inline.
pub const INTERFACE_ID_LEN: usize = 1 + CHANNEL_TAG_HASH_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InterfaceId([u8; INTERFACE_ID_LEN]);

impl InterfaceId {
    pub const fn new(bytes: [u8; INTERFACE_ID_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; INTERFACE_ID_LEN] {
        &self.0
    }

    /// This id's interface kind — its first byte. `None` for an unknown discriminant (a foreign or
    /// corrupt id). Lets the reactor route a frame to its medium's lane straight off the id.
    #[must_use]
    pub fn kind(&self) -> Option<InterfaceKind> {
        InterfaceKind::from_u8(self.0[0])
    }

    /// Derive an id from the interface's [`InterfaceKind`] and its `channel_tag` — the
    /// bytes that uniquely tag this channel within its medium (a peer MAC, a remote `ip:port`,
    /// a device id). The id is `[kind] ++ sha256(channel_tag)[..7]`: stable while the
    /// channel tag is (so a reconnecting peer rebinds its routes), unique across distinct
    /// channels (the channel tag's job), and namespaced by kind so two kinds never cross.
    /// The one invariant the caller owes: no two *concurrent* channels may share a
    /// `channel_tag`.
    #[must_use]
    pub fn from_channel_tag(kind: InterfaceKind, channel_tag: &[u8]) -> Self {
        let digest = sha256(channel_tag);
        let mut bytes = [0u8; INTERFACE_ID_LEN];
        bytes[0] = kind as u8;
        bytes[1..1 + CHANNEL_TAG_HASH_LEN].copy_from_slice(&digest[..CHANNEL_TAG_HASH_LEN]);
        Self(bytes)
    }
}
