use super::snapshot::RuntimeSnapshot;
use crate::routing::delivery::Delivery;

#[derive(Debug, Clone, Copy)]
pub enum PrnsEvent<'a> {
    SnapshotUpdated(&'a RuntimeSnapshot),
    Delivered(Delivery<'a>),
}
