pub mod receipts;
pub mod send_group;
pub mod send_plain;
pub mod send_single;

use crate::{
    engine::InstantMillis,
    identity::OpenedBy,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SingleDelivery<'p> {
    pub destination: DestinationHash,
    pub context: WireContext,
    pub plaintext: &'p [u8],
    pub opened_by: OpenedBy,
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
    /// The local receiving destination for a responder link. Initiated links
    /// receive from a remote destination and have no local destination owner.
    pub local_destination: Option<DestinationHash>,
    pub plaintext: &'p [u8],
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
}
