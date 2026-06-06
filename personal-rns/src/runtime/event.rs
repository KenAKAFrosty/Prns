use super::snapshot::RuntimeSnapshot;
use crate::engine::{CommandId, Settlement};
use crate::routing::delivery::Delivery;

#[derive(Debug, Clone, Copy)]
pub enum PrnsEvent<'a> {
    SnapshotUpdated(&'a RuntimeSnapshot),
    Delivered(Delivery<'a>),
    CommandSettled {
        id: CommandId,
        settlement: Settlement,
    },
}
