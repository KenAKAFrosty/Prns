mod impls;

pub use impls::*;

use crate::crypto::ratchets::SelfRatchetColumns;
use crate::crypto::Ed25519Signature;
use crate::engine::InstantMillis;
use crate::identity::held::HeldIdentityColumns;
use crate::interfaces::InterfaceId;
use crate::routing::announce::held_cache::HeldAnnounces;
use crate::routing::announce::rate_limit::AnnounceRateColumns;
use crate::routing::announce::schedule::RebroadcastQueue;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::routing::dedup::PacketHashHistory;
use crate::routing::delivery::receipts::ReceiptColumns;
use crate::routing::path_requests::pending::PendingPathRequestColumns;
use crate::routing::path_requests::seen::SeenPathRequestColumns;
use crate::routing::reverse_routes::ReverseRouteColumns;
use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;
use crate::routing::{NextHop, RouteResponsiveness};
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteEntry {
    pub hops: u8,
    pub expires: InstantMillis,
    pub responsiveness: RouteResponsiveness,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
}

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
pub struct ColumnsFull;

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

pub trait RouteColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn hops(&self) -> &[u8];
    fn expires(&self) -> &[InstantMillis];
    fn responsiveness(&self) -> &[RouteResponsiveness];
    fn receiving_interfaces(&self) -> &[InterfaceId];
    fn next_hops(&self) -> &[NextHop];

    fn set_row(&mut self, i: usize, row: RouteEntry);

    fn push(&mut self, destination: DestinationHash, row: RouteEntry)
        -> Result<usize, ColumnsFull>;

    fn swap_remove(&mut self, i: usize);
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

/// A storage recipe: the bundle of column backends an engine runs on. One type
/// (`FixedInline` for no_std, `GrowableHeap` for a std host) picks every backend
/// at once; the `Default` bounds let the engine build an empty bundle.
pub trait EngineStorage {
    type Routes: RouteColumns + Default;
    type Announces: RetainedAnnounceColumns + Default;
    type History: AnnounceIdHistory + Default;
    type AppData: RetainedAppData + Default;
    type Pending: RebroadcastQueue + Default;
    type Held: HeldAnnounces + Default;
    type UpstreamAppDestinations: UpstreamAppDestinationColumns + Default;
    type HeldIdentities: HeldIdentityColumns + Default;
    type SelfRatchets: SelfRatchetColumns + Default;
    type Receipts: ReceiptColumns + Default;
    type PacketHashes: PacketHashHistory + Default;
    type ReverseRoutes: ReverseRouteColumns + Default;
    type PendingPathRequests: PendingPathRequestColumns + Default;
    type SeenPathRequests: SeenPathRequestColumns + Default;
    type AnnounceRates: AnnounceRateColumns + Default;
}
