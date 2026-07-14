use crate::crypto::Ed25519Signature;
use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::{
    Announce, AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey, ANNOUNCE_ID_WIRE_LEN,
};
use crate::routing::routes::RouteEntry;
use crate::units::HopCount;
use crate::wire::{DestinationHash, TransportId};

/// RNS 1.3.5 `Transport.path_table`'s `received_from` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextHop {
    Direct,
    Via(TransportId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardingRoute {
    pub hops: HopCount,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
}

/// RNS 1.3.5 `Transport.path_is_unresponsive`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteResponsiveness {
    Unknown,
    Responsive,
    Unresponsive,
}

#[derive(Debug, Clone, Copy)]
pub struct ExistingRoute<'a> {
    pub hops: HopCount,
    pub expires_at: InstantMillis,
    pub announce_id_history: &'a [AnnounceId],
    pub responsiveness: RouteResponsiveness,
}

#[derive(Debug, Clone)]
pub struct StoredAnnounce<'a> {
    pub hops: u8,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
    pub announce: Announce<'a>,
}

/// One routing-table row as the persistence codec carries it: the route columns, the announce record that vouches for them, and the replay ring.
#[derive(Debug, Clone)]
pub struct PersistedRouteRow<'a> {
    pub destination: DestinationHash,
    pub entry: RouteEntry,
    pub public_keys: IdentityPublicKeys,
    pub dotted_name_hash: DottedNameHash,
    pub announce_id: AnnounceId,
    pub ratchet: Option<RatchetKey>,
    pub signature: Ed25519Signature,
    pub app_data: &'a [u8],
    pub announce_id_ring: AnnounceIdRing<'a>,
}

/// The per-route replay ring, borrowable from either end of the codec: table slices when flushing, validated wire bytes when seeding.
#[derive(Debug, Clone, Copy)]
pub enum AnnounceIdRing<'a> {
    Table(&'a [AnnounceId]),
    Wire(&'a [u8]),
}

impl AnnounceIdRing<'_> {
    pub fn len(&self) -> usize {
        match self {
            AnnounceIdRing::Table(ids) => ids.len(),
            AnnounceIdRing::Wire(bytes) => bytes.len() / ANNOUNCE_ID_WIRE_LEN,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Oldest first, matching the order `remember` replays them in.
    pub fn ids(&self) -> impl Iterator<Item = AnnounceId> + '_ {
        let (table, wire) = match self {
            AnnounceIdRing::Table(ids) => (Some(ids.iter().copied()), None),
            AnnounceIdRing::Wire(bytes) => (
                None,
                Some(bytes.chunks_exact(ANNOUNCE_ID_WIRE_LEN).map(|chunk| {
                    let mut bytes = [0u8; ANNOUNCE_ID_WIRE_LEN];
                    bytes.copy_from_slice(chunk);
                    AnnounceId::from_wire(bytes)
                })),
            ),
        };
        table
            .into_iter()
            .flatten()
            .chain(wire.into_iter().flatten())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedRouteOutcome {
    Seeded,
    AlreadyPresent,
    TableFull,
    AppDataArenaFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropCause {
    RoutingTableFull,
    PayloadArenaFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertRouteOutcome {
    Inserted,
    Updated,
    Dropped(DropCause),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// repr(C): crosses the dual-core channel inside `Journaled`; see the layout note on `EngineCommand`.
#[repr(C)]
pub enum RouteRemovalCause {
    Expired,
    Evicted,
    InterfaceGone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovedRoute {
    pub destination: DestinationHash,
    pub receiving_interface: InterfaceId,
    pub cause: RouteRemovalCause,
}
