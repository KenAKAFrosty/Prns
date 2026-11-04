//! Durable group obligations, independent of app presentation and runtime state.
use prns_groups_store_redb::change::Stamp;
use prns_host::IdentityHash;

/// A scan visits at most this many catalog rows, opening no more than that many
/// membership namespaces, in one owner turn.
pub const MAX_DEPENDENCY_NAMESPACES_PER_STEP: usize = 4;

/// One validated group-domain revision. These are durable facts, not proof that
/// an executor, network carrier or key lease is quiescent. The owning assembly
/// must combine this with its actual live-resource observations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dependencies {
    pub stamp: Stamp,
    /// Includes locally owned groups. Leaving membership remains active until
    /// an authenticated terminal transition commits.
    pub active_memberships: u64,
    pub owner_groups: u64,
    pub pending_publications: u64,
    pub pending_publication_targets: u64,
    pub pending_receipts: u64,
    /// Invitation protocol obligations, conservatively retained until durable
    /// acknowledgement/maintenance rather than inferred from the wall clock.
    pub pending_enrollment: u64,
    pub pending_departures: u64,
    pub pending_terminal_notices: u64,
    pub pending_local_purges: u64,
    pub incomplete_setup: u64,
    pub unresolved_operations: u64,
    /// Passive evidence below is not unfinished network work.
    pub archived_incarnations: u64,
    pub retired_incarnations: u64,
    pub retained_records: u64,
    /// Passive local history, independent of protocol delivery retention.
    pub archived_records: u64,
    /// Logical archive keys/values/index bytes, not physical file allocation.
    pub archived_bytes: u64,
    /// Indexed service changes, not every backend receipt or MLS output.
    pub retained_result_changes: u64,
    pub terminal_operations: u64,
    pub retained_control_results: u64,
    /// Retired incarnation fences, closure/cancellation proofs, and nonzero
    /// request retirement floors. This is a count of retained rows/floors.
    pub passive_fences: u64,
}
impl Dependencies {
    pub(crate) fn empty(stamp: Stamp) -> Self {
        Self {
            stamp,
            active_memberships: 0,
            owner_groups: 0,
            pending_publications: 0,
            pending_publication_targets: 0,
            pending_receipts: 0,
            pending_enrollment: 0,
            pending_departures: 0,
            pending_terminal_notices: 0,
            pending_local_purges: 0,
            incomplete_setup: 0,
            unresolved_operations: 0,
            archived_incarnations: 0,
            retired_incarnations: 0,
            retained_records: 0,
            archived_records: 0,
            archived_bytes: 0,
            retained_result_changes: 0,
            terminal_operations: 0,
            retained_control_results: 0,
            passive_fences: 0,
        }
    }
}

/// An opaque, disposable, bounded scan continuation. It retains no private
/// membership keys or database transaction between owner turns. A revision
/// change invalidates the entire scan; callers must begin again after Busy.
pub struct DependencyScan {
    pub(crate) identity: IdentityHash,
    pub(crate) next: CatalogCursor,
    pub(crate) result: Dependencies,
}

/// Fixed-width keys keep a scan independent of the catalog's size. Unlike
/// ordinary display pagination, dependency totals require one unchanged
/// revision; the manager checks that stamp around every bounded step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogCursor {
    Incarnations(Option<[u8; 36]>),
    Invitations(Option<[u8; 32]>),
    Notices(Option<[u8; 32]>),
    Terminals(Option<[u8; 32]>),
    Audience(Option<[u8; 48]>),
    Complete,
}

pub(crate) fn add(target: &mut u64, count: u64) -> Result<(), crate::Error> {
    *target = target.checked_add(count).ok_or(crate::Error::Corrupt)?;
    Ok(())
}
