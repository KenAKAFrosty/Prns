use crate::crypto::Ed25519Signature;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::storage::ColumnsFull;

mod impls;

pub use impls::{FixedArrayRetainedAnnounceColumns, PackedAppDataArena, TieredAnnounceIdHistory};
#[cfg(feature = "alloc")]
pub use impls::{HeapAnnounceIdHistory, HeapRetainedAnnounceColumns, HeapRetainedAppData};
#[cfg(feature = "external-alloc")]
pub use impls::{
    FixedHeapPackedAppDataArena, FixedHeapRetainedAnnounceColumns, FixedHeapTieredAnnounceIdHistory,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedAnnounceEntry {
    pub public_keys: IdentityPublicKeys,
    pub dotted_name_hash: DottedNameHash,
    pub retained_announce_id: AnnounceId,
    pub signature: Ed25519Signature,
    pub maybe_ratchet: Option<RatchetKey>,
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
    fn history(&self, slot: usize) -> AnnounceIdHistoryView<'_>;
    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome;
    fn swap_remove(&mut self, i: usize, last: usize);
}

#[derive(Debug, Clone, Copy)]
pub struct AnnounceIdHistoryView<'a> {
    floor: &'a [AnnounceId],
    overflow: &'a [AnnounceId],
}

impl<'a> AnnounceIdHistoryView<'a> {
    pub const EMPTY: AnnounceIdHistoryView<'static> = AnnounceIdHistoryView {
        floor: &[],
        overflow: &[],
    };

    pub fn from_slices(floor: &'a [AnnounceId], overflow: &'a [AnnounceId]) -> Self {
        Self { floor, overflow }
    }

    pub fn len(&self) -> usize {
        self.floor.len() + self.overflow.len()
    }

    pub fn is_empty(&self) -> bool {
        self.floor.is_empty() && self.overflow.is_empty()
    }

    pub fn iter(
        &self,
    ) -> core::iter::Chain<core::slice::Iter<'a, AnnounceId>, core::slice::Iter<'a, AnnounceId>>
    {
        self.floor.iter().chain(self.overflow.iter())
    }

    pub fn contains(&self, id: &AnnounceId) -> bool {
        self.floor.contains(id) || self.overflow.contains(id)
    }
}

pub trait RetainedAppData {
    fn get(&self, handle: AppDataHandle) -> &[u8];
    fn insert(&mut self, bytes: &[u8]) -> Result<AppDataHandle, RetainedAppDataError>;
    fn replace(&mut self, handle: AppDataHandle, bytes: &[u8]) -> Result<(), RetainedAppDataError>;
    fn free(&mut self, handle: AppDataHandle);
}
