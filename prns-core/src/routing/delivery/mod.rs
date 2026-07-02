pub mod receipts;
pub mod send_group;
pub mod send_single;

use crate::{
    engine::InstantMillis,
    interfaces::InterfaceId,
    routing::links::LinkId,
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
/// a direct neighbor. Anything beyond one hop was relayed against protocol and
/// is dropped.
pub const PLAIN_DATA_MAX_RECEIVED_HOPS: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SingleDelivery<'p> {
    pub destination: DestinationHash,
    pub context: WireContext,
    pub plaintext: &'p [u8],
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupDelivery<'p> {
    pub destination: DestinationHash,
    pub context: WireContext,
    pub plaintext: &'p [u8],
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery<'p> {
    Plain(PlainDelivery<'p>),
    Single(SingleDelivery<'p>),
    Group(GroupDelivery<'p>),
    Link(LinkDelivery<'p>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkDelivery<'p> {
    pub link_id: LinkId,
    pub plaintext: &'p [u8],
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
}
