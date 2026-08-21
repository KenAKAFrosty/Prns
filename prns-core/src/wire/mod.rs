mod address;
mod context;
mod flags;
mod header;
mod limits;

pub use address::{DestinationHash, TransportId, WireAddress};
pub use context::WireContext;
pub use flags::{ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType};
pub use header::{WireError, WirePacketHeader};
pub use limits::{
    wire_hop_count_is_valid, ANNOUNCE_PUBLIC_KEY_BYTE_LEN, BROADCAST_MDU, BROADCAST_MTU,
    DOTTED_NAME_HASH_BYTE_LEN, HEADER_MAX_LEN, HEADER_MIN_LEN, IFAC_MIN_LEN, MAX_HOP_COUNT,
    RATCHET_BYTE_LEN, SIGNATURE_BYTE_LEN, TRUNCATED_HASH_BYTE_LEN,
};

#[cfg(test)]
mod tests;
