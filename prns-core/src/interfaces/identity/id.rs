use crate::crypto::sha256;
use crate::interfaces::InterfaceKind;

/// A collision is birthday-bounded against concurrent same-kind interfaces, and the attach path rejects a live collision rather than aliasing two interfaces.
const CHANNEL_TAG_HASH_LEN: usize = 7;

pub const INTERFACE_ID_LEN: usize = 1 + CHANNEL_TAG_HASH_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InterfaceId([u8; INTERFACE_ID_LEN]);

impl InterfaceId {
    pub const fn new(bytes: [u8; INTERFACE_ID_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; INTERFACE_ID_LEN] {
        &self.0
    }

    #[must_use]
    pub fn kind(&self) -> Option<InterfaceKind> {
        InterfaceKind::from_u8(self.0[0])
    }

    /// The caller must not assign the same `channel_tag` to concurrent channels of the same kind.
    #[must_use]
    pub fn from_channel_tag(kind: InterfaceKind, channel_tag: &[u8]) -> Self {
        let digest = sha256(channel_tag);
        let mut bytes = [0u8; INTERFACE_ID_LEN];
        bytes[0] = kind as u8;
        bytes[1..1 + CHANNEL_TAG_HASH_LEN].copy_from_slice(&digest[..CHANNEL_TAG_HASH_LEN]);
        Self(bytes)
    }
}
