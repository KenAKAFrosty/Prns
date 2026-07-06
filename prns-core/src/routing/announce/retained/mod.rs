use crate::crypto::Ed25519Signature;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::storage::ColumnsFull;

mod impls;

pub use impls::{FixedAnnounceIdHistory, FixedArrayRetainedAnnounceColumns, PackedAppDataArena};
#[cfg(feature = "external-alloc")]
pub use impls::{
    FixedHeapAnnounceIdHistory, FixedHeapPackedAppDataArena, FixedHeapRetainedAnnounceColumns,
};
#[cfg(feature = "alloc")]
pub use impls::{HeapAnnounceIdHistory, HeapRetainedAnnounceColumns, HeapRetainedAppData};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedAnnounceEntry {
    pub public_keys: IdentityPublicKeys,
    pub dotted_name_hash: DottedNameHash,
    pub retained_announce_id: AnnounceId,
    pub signature: Ed25519Signature,
    pub ratchet: Option<RatchetKey>,
    pub maybe_app_data_handle: Option<AppDataHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RememberOutcome {
    AlreadyKnown,
    StoredFresh,
    StoredEvictingOldest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppDataHandle(usize);

impl AppDataHandle {
    pub(crate) const fn new(slot: usize) -> Self {
        Self(slot)
    }

    pub(crate) const fn slot(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedAppDataError {
    ArenaFull,
    TooManyEntries,
}

pub trait RetainedAnnounceColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn public_keys(&self) -> &[IdentityPublicKeys];
    fn dotted_name_hash(&self) -> &[DottedNameHash];
    fn retained_announce_id(&self) -> &[AnnounceId];
    fn ratchet(&self) -> &[Option<RatchetKey>];
    fn signature(&self) -> &[Ed25519Signature];
    fn app_data_handle(&self) -> &[Option<AppDataHandle>];

    fn set_row(&mut self, i: usize, row: RetainedAnnounceEntry);

    fn push(&mut self, row: RetainedAnnounceEntry) -> Result<usize, ColumnsFull>;

    fn swap_remove(&mut self, i: usize);
}

pub trait AnnounceIdHistory {
    fn history(&self, slot: usize) -> &[AnnounceId];
    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome;
    fn swap_remove(&mut self, i: usize, last: usize);
}

pub trait RetainedAppData {
    fn get(&self, handle: AppDataHandle) -> &[u8];
    fn insert(&mut self, bytes: &[u8]) -> Result<AppDataHandle, RetainedAppDataError>;
    fn replace(&mut self, handle: AppDataHandle, bytes: &[u8]) -> Result<(), RetainedAppDataError>;
    fn free(&mut self, handle: AppDataHandle);
}
