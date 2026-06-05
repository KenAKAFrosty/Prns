use super::snapshot::RuntimeSnapshot;
use crate::routing::delivery::PlainDelivery;

#[derive(Debug, Clone, Copy)]
pub enum PrnsEvent<'a> {
    SnapshotUpdated(&'a RuntimeSnapshot),
    Delivered(PlainDelivery<'a>),
}
