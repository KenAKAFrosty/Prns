use crate::{
    engine::InstantMillis,
    interfaces::InterfaceId,
    wire::{DestinationHash, WireContext},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlainDelivery<'p> {
    pub destination: DestinationHash,
    pub context: WireContext,
    pub payload: &'p [u8],
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
}

/// RNS 1.3.1 `Transport.packet_filter`: a plain data packet is only heard from
/// a direct neighbor — anything beyond one hop was relayed against protocol and
/// is dropped.
const PLAIN_DATA_MAX_RECEIVED_HOPS: u8 = 1;
