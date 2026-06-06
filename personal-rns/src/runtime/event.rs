use super::snapshot::RuntimeSnapshot;
use crate::engine::{AnnounceNowError, WriteSelfAnnounceError};
use crate::routing::delivery::Delivery;

#[derive(Debug, Clone, Copy)]
pub enum PrnsEvent<'a> {
    SnapshotUpdated(&'a RuntimeSnapshot),
    Delivered(Delivery<'a>),
    CommandFailed(CommandFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandFailure {
    AnnounceRejected(AnnounceNowError),
    AnnounceWriteFailed(WriteSelfAnnounceError),
}
