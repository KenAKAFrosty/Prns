//! Enrollment's indexed catalog. A read owns one redb snapshot and opens only
//! the selected, named tables. Payloads are decoded by exact key or a bounded
//! range; opening this view never collects memberships or invitation rows.
//!
//! The header is also the exclusive carrier's commit witness. Every mutation
//! replaces it together with its row/index/accounting changes. An uncertain
//! carrier can therefore compare its exact before/after witnesses without
//! hashing unrelated groups or mistaking another revision for its own result.
use super::*;
use redb::{ReadOnlyTable, ReadTransaction, ReadableTable, ReadableTableMetadata};

#[cfg(feature = "archive")]
mod archive;
pub(super) mod dependencies;
mod inspect;
pub(super) mod keys;
#[cfg(feature = "archive")]
pub(super) use archive::archive_binding_in;
mod maintenance;
mod obligations;
mod pages;
pub(super) mod policy;
pub(super) mod purge;
mod queries;
pub(super) mod rows;
mod terminal_obligations;
mod write;
use policy::CatalogLimits;
pub(super) use write::{Edit, Write};
#[cfg(test)]
mod tests;

pub(super) mod work;

pub(super) const VERSION: u8 = 5;
pub(super) const HEADER: TableDefinition<u8, &[u8]> = storage::CATALOG;
pub(super) const CURRENT: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_current_v5");
pub(super) const INCARNATIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_incarnations_v5");
pub(super) const NAMESPACES: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_namespaces_v5");
pub(super) const INCARNATION_IDS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_incarnation_ids_v5");
pub(super) const INVITATIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_invitations_v5");
pub(super) const INVITATION_GROUPS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_invitation_groups_v5");
pub(super) const INVITATION_PEERS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_invitation_peers_v5");
pub(super) const INVITATION_EXPIRY: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_invitation_expiry_v5");
pub(super) const INVITATION_BINDINGS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_invitation_bindings_v5");
pub(super) const AUDIENCE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_audience_v5");
pub(super) const REQUESTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_requests_v5");
pub(super) const REQUEST_EXPIRY: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_request_expiry_v5");
pub(super) const DEPARTURES: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_departures_v5");
pub(super) const NOTICES: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_notices_v5");
pub(super) const TERMINALS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_terminals_v5");
pub(super) const TERMINAL_GROUPS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_terminal_groups_v5");
pub(super) const TERMINAL_SUBJECTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_terminal_subjects_v5");
pub(super) const CANCELLATIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_cancellations_v5");
pub(super) const CANCELLATION_EXPIRY: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_cancellation_expiry_v5");
pub(super) const PENDING_INVITATIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_pending_invitations_v5");
pub(super) const UNSOLICITED_CANCELLATIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_unsolicited_cancellations_v5");
pub(super) const INCOMING_PEERS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_incoming_peers_v5");
pub(super) const PENDING_RECIPIENTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_pending_recipients_v5");
pub(super) const WORK_IDS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_work_ids_v5");

pub(super) const TERMINAL_OBLIGATIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_enrollment_terminal_obligations_v5");

/// The empty key is outside every fixed-width domain-key range. Requiring this
/// row also detects a missing empty table inside a write transaction: redb's
/// write-side open may create a table, but the caller must abort without its
/// marker. No table enumeration or open/create/delete presence probe is needed.
pub(super) const MARKER: &[u8] = b"prns.groups.enrollment.index.v5";

const ROW_TABLES: [TableDefinition<&[u8], &[u8]>; 25] = [
    CURRENT,
    INCARNATIONS,
    NAMESPACES,
    INCARNATION_IDS,
    INVITATIONS,
    INVITATION_GROUPS,
    INVITATION_PEERS,
    INVITATION_EXPIRY,
    INVITATION_BINDINGS,
    AUDIENCE,
    REQUESTS,
    REQUEST_EXPIRY,
    DEPARTURES,
    NOTICES,
    TERMINALS,
    TERMINAL_GROUPS,
    TERMINAL_SUBJECTS,
    CANCELLATIONS,
    CANCELLATION_EXPIRY,
    PENDING_INVITATIONS,
    UNSOLICITED_CANCELLATIONS,
    INCOMING_PEERS,
    PENDING_RECIPIENTS,
    WORK_IDS,
    TERMINAL_OBLIGATIONS,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Counts {
    pub groups: u64,
    pub live: u64,
    pub archived: u64,
    pub retired: u64,
    pub invitations: u64,
    pub invitation_bindings: u64,
    pub audience: u64,
    pub requests: u64,
    pub departures: u64,
    pub notices: u64,
    pub terminals: u64,
    pub cancellations: u64,
    pub pending_invitations: u64,
    pub unsolicited_cancellations: u64,
    pub incoming_peers: u64,
    pub pending_recipients: u64,
    pub work_ids: u64,
    pub terminal_obligations: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Header {
    pub version: u8,
    pub local: [u8; 16],
    pub limits: CatalogLimits,
    pub revision: u64,
    pub witness: [u8; 32],
    /// Logical primary/index key and value bytes. The fixed header, fixed schema
    /// markers and redb page overhead are separate from this admission quota.
    pub bytes: u64,
    /// Funded future catalog cleanup obligations of already-admitted rows.
    /// This never multiplies a supported member or group ceiling by a frame.
    pub reserved: u64,
    pub counts: Counts,
    pub request_retired_before: u64,
    pub invitation_time_floor: u64,
    pub invitation_maintenance_after: Option<maintenance::Cursor>,
    pub output_maintenance_after: Option<[u8; 32]>,
    pub maintenance_phase: maintenance::Phase,
    pub purge_after: Option<purge::Cursor>,
    pub terminal_after: Option<[u8; 32]>,
    pub obligation_after: Option<terminal_obligations::Cursor>,
}

impl Header {
    // A fixed bound on this collection-free encoding, not on catalog capacity.
    // Its largest canonical representation is tested with every scalar at its
    // representation limit; row payload budgets are selected independently.
    const MAX_ENCODED_BYTES: usize = 4096;

    pub(super) fn fingerprint(&self) -> Result<[u8; 32], EnrollmentError> {
        Ok(hash(&encode_with_limit(self, Self::MAX_ENCODED_BYTES)?))
    }

    pub(super) fn decode(bytes: &[u8], local: IdentityHash) -> Result<Self, EnrollmentError> {
        if bytes.len() > Self::MAX_ENCODED_BYTES {
            return Err(EnrollmentError::UnsupportedVersion);
        }
        #[derive(Deserialize)]
        struct Version {
            version: u8,
        }
        let version: Version =
            serde_json::from_slice(bytes).map_err(|_| EnrollmentError::UnsupportedVersion)?;
        if version.version != VERSION {
            return Err(EnrollmentError::UnsupportedVersion);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| EnrollmentError::Corrupt)?;
        if value.local != local.into_bytes() {
            return Err(EnrollmentError::Authentication);
        }
        value
            .limits
            .validate()
            .map_err(|_| EnrollmentError::Corrupt)?;
        if value
            .bytes
            .checked_add(value.reserved)
            .is_none_or(|total| total > value.limits.max_catalog_bytes)
            || value.counts.pending_recipients > value.counts.invitations
            || value.counts.pending_invitations > value.counts.invitations
            || value.counts.unsolicited_cancellations > value.counts.cancellations
            || value.counts.pending_invitations > u64::from(value.limits.max_incoming_invitations)
            || value.counts.unsolicited_cancellations
                > u64::from(value.limits.max_incoming_invitations)
            || value.counts.live > value.counts.groups
            || value
                .limits
                .max_groups
                .is_some_and(|limit| value.counts.live > limit)
        {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(value)
    }
}

pub(super) struct Read {
    pub header: Header,
    pub current: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub incarnations: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub namespaces: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub incarnation_ids: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub invitations: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub invitation_groups: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub invitation_peers: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub invitation_expiry: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub invitation_bindings: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub audience: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub requests: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub request_expiry: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub departures: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub notices: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub terminals: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub terminal_groups: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub terminal_subjects: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub cancellations: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub cancellation_expiry: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub pending_invitations: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub unsolicited_cancellations: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub incoming_peers: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub pending_recipients: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub work_ids: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub terminal_obligations: ReadOnlyTable<&'static [u8], &'static [u8]>,
}

impl Read {
    pub(super) fn open(tx: &ReadTransaction, local: IdentityHash) -> Result<Self, EnrollmentError> {
        let table = tx.open_table(HEADER).map_err(|error| match error {
            redb::TableError::TableDoesNotExist(_) => EnrollmentError::MissingCatalog,
            error => db_error(error),
        })?;
        if table.len().map_err(db_error)? != 1 {
            return Err(EnrollmentError::Corrupt);
        }
        let header = Header::decode(
            table
                .get(0)
                .map_err(db_error)?
                .ok_or(EnrollmentError::Corrupt)?
                .value(),
            local,
        )?;
        let memberships = header
            .counts
            .live
            .checked_add(header.counts.archived)
            .and_then(|n| n.checked_add(header.counts.retired))
            .ok_or(EnrollmentError::Corrupt)?;
        let selected = |definition, rows| open_rows(tx, definition, rows);
        Ok(Self {
            current: selected(CURRENT, header.counts.groups)?,
            incarnations: selected(INCARNATIONS, memberships)?,
            namespaces: selected(NAMESPACES, memberships)?,
            incarnation_ids: selected(INCARNATION_IDS, memberships)?,
            invitations: selected(INVITATIONS, header.counts.invitations)?,
            invitation_groups: selected(INVITATION_GROUPS, header.counts.invitations)?,
            invitation_peers: selected(INVITATION_PEERS, header.counts.invitations)?,
            invitation_expiry: selected(INVITATION_EXPIRY, header.counts.invitations)?,
            invitation_bindings: selected(INVITATION_BINDINGS, header.counts.invitation_bindings)?,
            audience: selected(AUDIENCE, header.counts.audience)?,
            requests: selected(REQUESTS, header.counts.requests)?,
            request_expiry: selected(REQUEST_EXPIRY, header.counts.requests)?,
            departures: selected(DEPARTURES, header.counts.departures)?,
            notices: selected(NOTICES, header.counts.notices)?,
            terminals: selected(TERMINALS, header.counts.terminals)?,
            terminal_groups: selected(TERMINAL_GROUPS, header.counts.terminals)?,
            terminal_subjects: selected(TERMINAL_SUBJECTS, header.counts.terminals)?,
            cancellations: selected(CANCELLATIONS, header.counts.cancellations)?,
            cancellation_expiry: selected(CANCELLATION_EXPIRY, header.counts.cancellations)?,
            pending_invitations: selected(PENDING_INVITATIONS, header.counts.pending_invitations)?,
            unsolicited_cancellations: selected(
                UNSOLICITED_CANCELLATIONS,
                header.counts.unsolicited_cancellations,
            )?,
            incoming_peers: selected(INCOMING_PEERS, header.counts.incoming_peers)?,
            pending_recipients: selected(PENDING_RECIPIENTS, header.counts.pending_recipients)?,
            work_ids: selected(WORK_IDS, header.counts.work_ids)?,
            terminal_obligations: selected(
                TERMINAL_OBLIGATIONS,
                header.counts.terminal_obligations,
            )?,
            header,
        })
    }
}

fn open_rows(
    tx: &ReadTransaction,
    definition: TableDefinition<&'static [u8], &'static [u8]>,
    rows: u64,
) -> Result<ReadOnlyTable<&'static [u8], &'static [u8]>, EnrollmentError> {
    let table = tx.open_table(definition).map_err(|error| match error {
        redb::TableError::TableDoesNotExist(_) => EnrollmentError::Corrupt,
        error => db_error(error),
    })?;
    if table.len().map_err(db_error)? != rows.checked_add(1).ok_or(EnrollmentError::Corrupt)?
        || table
            .get(b"".as_slice())
            .map_err(db_error)?
            .as_ref()
            .map(|row| row.value())
            != Some(MARKER)
    {
        return Err(EnrollmentError::Corrupt);
    }
    Ok(table)
}

pub(super) fn initialize_in(
    tx: &WriteTransaction,
    local: IdentityHash,
    limits: CatalogLimits,
) -> Result<(), EnrollmentError> {
    limits.validate()?;
    // Initialization is a cold, explicit operation. Reject every partial or
    // wrong-kind selected table instead of completing or replacing its schema.
    for table in tx.list_tables().map_err(db_error)? {
        if table.name() == HEADER.name() || ROW_TABLES.iter().any(|d| d.name() == table.name()) {
            return Err(EnrollmentError::AlreadyInitialized);
        }
    }
    for table in tx.list_multimap_tables().map_err(db_error)? {
        use redb::MultimapTableHandle;
        if table.name() == HEADER.name() || ROW_TABLES.iter().any(|d| d.name() == table.name()) {
            return Err(EnrollmentError::AlreadyInitialized);
        }
    }
    let header = Header {
        version: VERSION,
        local: local.into_bytes(),
        limits,
        revision: 0,
        witness: [0; 32],
        bytes: 0,
        reserved: 0,
        counts: Counts::default(),
        request_retired_before: 0,
        invitation_time_floor: 0,
        invitation_maintenance_after: None,
        output_maintenance_after: None,
        maintenance_phase: maintenance::Phase::Requests,
        purge_after: None,
        terminal_after: None,
        obligation_after: None,
    };
    tx.open_table(HEADER)
        .map_err(db_error)?
        .insert(
            0,
            encode_with_limit(&header, Header::MAX_ENCODED_BYTES)?.as_slice(),
        )
        .map_err(db_error)?;
    for definition in ROW_TABLES {
        tx.open_table(definition)
            .map_err(db_error)?
            .insert(b"".as_slice(), MARKER)
            .map_err(db_error)?;
    }
    Ok(())
}
