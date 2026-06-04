use super::snapshot::RuntimeSnapshot;

#[derive(Debug, Clone, Copy)]
pub enum PrnsEvent<'a> {
    SnapshotUpdated(&'a RuntimeSnapshot),
}
