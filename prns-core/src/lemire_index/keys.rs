use super::core::{lemire_key_from_prefix, IndexKey};
use crate::routing::dedup::PacketHash;
use crate::wire::DestinationHash;

impl IndexKey for DestinationHash {
    fn lemire_key(&self) -> u64 {
        lemire_key_from_prefix(self.as_bytes())
    }
}

impl IndexKey for crate::identity::IdentityHash {
    fn lemire_key(&self) -> u64 {
        lemire_key_from_prefix(self.as_bytes())
    }
}

impl IndexKey for PacketHash {
    fn lemire_key(&self) -> u64 {
        lemire_key_from_prefix(self.as_bytes())
    }
}

impl IndexKey for crate::routing::links::LinkId {
    fn lemire_key(&self) -> u64 {
        lemire_key_from_prefix(self.as_bytes())
    }
}

impl IndexKey for crate::interfaces::InterfaceId {
    /// Little-endian, unlike the hash keys above: an id's first byte is its kind, shared by every interface of that kind (a server's thousand TCP clients all match), and the bucket reduction weighs high bits most.
    /// Read low-endian, the kind byte lands in the bits the reduction barely sees and the channel-tag hash bytes pick the bucket.
    fn lemire_key(&self) -> u64 {
        u64::from_le_bytes(*self.as_bytes())
    }
}
