use crate::crypto::Ed25519Signature;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::storage::TablePushError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceRecord {
    pub public_keys: IdentityPublicKeys,
    pub dotted_name_hash: DottedNameHash,
    pub announce_id: AnnounceId,
    pub signature: Ed25519Signature,
    pub ratchet: Option<RatchetKey>,
    pub maybe_app_data_handle: Option<AppDataHandle>,
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
pub enum AnnounceAppDataError {
    ArenaFull,
    TooManyEntries,
}

pub trait AnnounceRecordTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn public_keys(&self) -> &[IdentityPublicKeys];
    fn dotted_name_hashes(&self) -> &[DottedNameHash];
    fn announce_ids(&self) -> &[AnnounceId];
    fn ratchets(&self) -> &[Option<RatchetKey>];
    fn signatures(&self) -> &[Ed25519Signature];
    fn app_data_handles(&self) -> &[Option<AppDataHandle>];

    fn set_row(&mut self, i: usize, row: AnnounceRecord);

    fn push(&mut self, row: AnnounceRecord) -> Result<usize, TablePushError>;

    fn swap_remove(&mut self, i: usize, last: usize);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RememberOutcome {
    AlreadyKnown,
    StoredFresh,
    StoredEvictingOldest,
}

pub trait AnnounceIdHistory {
    fn history(&self, slot: usize) -> &[AnnounceId];
    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome;
    fn swap_remove(&mut self, i: usize, last: usize);
}

pub trait AnnounceAppData {
    fn get(&self, handle: AppDataHandle) -> &[u8];
    fn insert(&mut self, bytes: &[u8]) -> Result<AppDataHandle, AnnounceAppDataError>;
    fn replace(&mut self, handle: AppDataHandle, bytes: &[u8]) -> Result<(), AnnounceAppDataError>;
    fn free(&mut self, handle: AppDataHandle);
}
