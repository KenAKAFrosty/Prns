//! Bounded encrypted NOR storage with strict capsule retirement.
//!
//! This format is experimental. Its platform must supply fresh cryptographic
//! entropy, a protected wrapping key, durable NOR writes and qualified erasure.
//! It does not detect restoration of an entire previously valid flash image.
//! Finalizing receipts is a trusted caller attestation that recovery obligations
//! are discharged, not an application or network acknowledgement inferred here.

mod codec;
mod journal;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use embedded_storage::nor_flash::NorFlash;
use zeroize::Zeroizing;

use crate::{flash::Partition, CommitReceipt, OperationTicket, Revision, StoreIncarnation};

/// Fill from a CSPRNG independently seeded on every boot. Reusing deterministic
/// test streams across boots is not a production entropy implementation.
pub trait Entropy {
    type Error;
    fn fill(&mut self, output: &mut [u8]) -> Result<(), Self::Error>;
}

/// Supply the device's domain-separated capsule wrapping key. Implementations
/// must not persist that key in this partition or include it in diagnostics.
pub trait KeyProvider {
    type Error;
    fn wrapping_key(&mut self, output: &mut [u8; 32]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecureLimits {
    pub capsule_sectors: usize,
    pub max_live_objects: usize,
    pub max_catalog_bytes: usize,
    pub max_key_bytes: usize,
    pub max_value_bytes: usize,
    pub max_changes: usize,
    pub max_receipts: usize,
    /// Headroom that each compacted live set must leave for subsequent work.
    pub reserve_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecureUsage {
    pub generation: u64,
    pub revision: Revision,
    pub retired_through: u64,
    pub live_objects: usize,
    pub receipts: usize,
    pub active_arena: usize,
    pub used_bytes: usize,
    pub arena_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecureRequest {
    Compact,
    /// The trusted owner attests that every ticket through this sequence is
    /// finalized. This refuses wrong incarnations and never moves beyond head.
    FinalizeThrough(OperationTicket),
    EraseRetired,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SecureError<F, K, R> {
    Flash(F),
    Key(K),
    Entropy(R),
    Geometry,
    NotErased,
    Empty,
    Corrupt,
    Configuration,
    Authentication,
    Capacity,
    Allocation,
    CounterExhausted,
}

impl<F: core::fmt::Debug, K: core::fmt::Debug, R: core::fmt::Debug> core::fmt::Display
    for SecureError<F, K, R>
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "secure flash error: {self:?}")
    }
}
impl<F: core::fmt::Debug, K: core::fmt::Debug, R: core::fmt::Debug> core::error::Error
    for SecureError<F, K, R>
{
}

pub type ErrorOf<F, K, R> = SecureError<
    <F as embedded_storage::nor_flash::ErrorType>::Error,
    <K as KeyProvider>::Error,
    <R as Entropy>::Error,
>;

const BLOCK: usize = 256;
const MARKER: [u8; BLOCK] = [0x3c; BLOCK];
const MAGIC: &[u8; 8] = b"PRNSSK01";
const NO_PARENT: u32 = u32::MAX;

struct Object {
    key: Zeroizing<Vec<u8>>,
    version: [u8; 16],
    dek: Zeroizing<Vec<u8>>,
    nonce: [u8; 24],
    location: u32,
    length: usize,
    digest: [u8; 32],
}

struct Catalog {
    revision: Revision,
    floor: u64,
    arena: usize,
    objects: Vec<Object>,
    receipts: Vec<CommitReceipt>,
}

#[derive(Clone, Copy)]
struct Root {
    generation: u64,
    slot: usize,
    parent: u32,
    catalog_at: u32,
    catalog_bytes: usize,
    catalog_nonce: [u8; 24],
    digest: [u8; 32],
}

type OpenedCapsule = (Root, Zeroizing<[u8; 32]>);

/// One serialized owner; operations return only after authenticating the live
/// set and verifying erasure of all non-current key capsules. Old ciphertext
/// remains until GC; archived capsules and physical rollback are not prevented.
pub struct SecureBackend<F: NorFlash, K: KeyProvider, R: Entropy> {
    flash: F,
    key_provider: K,
    entropy: R,
    partition: Partition,
    limits: SecureLimits,
    incarnation: StoreIncarnation,
    config: [u8; 32],
    wrapping: Zeroizing<[u8; 32]>,
    root: Option<Root>,
    catalog: Catalog,
    arena_start: [u32; 2],
    arena_bytes: usize,
    cursor: usize,
    dirty: bool,
}

impl<F: NorFlash, K: KeyProvider, R: Entropy> SecureBackend<F, K, R> {
    pub fn usage(&self) -> SecureUsage {
        SecureUsage {
            generation: self.root.map_or(0, |r| r.generation),
            revision: self.catalog.revision,
            retired_through: self.catalog.floor,
            live_objects: self.catalog.objects.len(),
            receipts: self.catalog.receipts.len(),
            active_arena: self.catalog.arena,
            used_bytes: self.cursor,
            arena_bytes: self.arena_bytes,
        }
    }

    pub fn into_parts(self) -> (F, K, R) {
        (self.flash, self.key_provider, self.entropy)
    }
}
