use super::snapshot::RuntimeSnapshot;
use crate::engine::{CommandId, Settlement};
use crate::interfaces::InterfaceId;
use crate::routing::delivery::Delivery;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy)]
pub enum PrnsEvent<'a> {
    SnapshotUpdated(&'a RuntimeSnapshot),
    Delivered(Delivery<'a>),
    AnnounceHeard {
        destination: DestinationHash,
        hops: u8,
        source_interface: InterfaceId,
    },
    CommandSettled {
        id: CommandId,
        settlement: Settlement,
    },
}
