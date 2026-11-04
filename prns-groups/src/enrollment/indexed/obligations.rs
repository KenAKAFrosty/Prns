//! Cleanup liability follows actual admitted facts, never the supported member
//! ceiling. Values are derived from the same row serializer used at commit.
use super::*;

/// Both the outbox row and its exact completion-routing index are persisted.
/// Admission checks each future value against the selected row limit as well as
/// reserving their serialized bytes; a small policy cannot admit unfinishable work.
pub(super) fn notice_bytes(max_row_bytes: usize) -> Result<u64, EnrollmentError> {
    let row = rows::Notice {
        terminal: [u8::MAX; 32],
        recipient: [u8::MAX; 16],
        acknowledged: false,
    };
    let value = encode_with_limit(&row, max_row_bytes)?.len();
    let work = encode_with_limit(&work::WorkSource::Notice([u8::MAX; 32]), max_row_bytes)?.len();
    value
        .checked_add(32)
        .and_then(|n| n.checked_add(work))
        .and_then(|n| n.checked_add(32))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or(EnrollmentError::Capacity)
}

pub(super) fn close_notice_bytes(max_row_bytes: usize) -> Result<u64, EnrollmentError> {
    let mut audience = rows::Audience {
        admitted: true,
        offers: u64::MAX,
        close_notice_funded: true,
    };
    let before = encode_with_limit(&audience, max_row_bytes)?.len();
    audience.close_notice_funded = false;
    let growth = encode_with_limit(&audience, max_row_bytes)?
        .len()
        .checked_sub(before)
        .ok_or(EnrollmentError::Corrupt)?;
    notice_bytes(max_row_bytes)?
        .checked_add(u64::try_from(growth).map_err(|_| EnrollmentError::Capacity)?)
        .ok_or(EnrollmentError::Capacity)
}

pub(super) fn audience_reserve(
    row: Option<&rows::Audience>,
    max_row_bytes: usize,
) -> Result<u64, EnrollmentError> {
    if row.is_some_and(|row| row.close_notice_funded) {
        close_notice_bytes(max_row_bytes)
    } else {
        Ok(0)
    }
}

pub(super) fn terminal_reserve(
    row: Option<&rows::TerminalRow>,
    max_row_bytes: usize,
) -> Result<u64, EnrollmentError> {
    let Some(row) = row.filter(|r| !r.complete) else {
        return Ok(0);
    };
    let current = encode_with_limit(row, max_row_bytes)?.len();
    let maximum = rows::TerminalRow {
        proof: row.proof.clone(),
        after: Some([u8::MAX; 16]),
        complete: false,
    };
    let growth = encode_with_limit(&maximum, max_row_bytes)?
        .len()
        .checked_sub(current)
        .ok_or(EnrollmentError::Corrupt)?;
    u64::try_from(growth).map_err(|_| EnrollmentError::Capacity)
}

/// Immutable Offer bytes never change; only these presentation/retry facts can
/// grow. Fund their exact largest canonical representation at first admission,
/// so terminal transitions do not need newly available catalog or row capacity.
pub(super) fn invitation_reserve(
    row: &Invite,
    max_row_bytes: usize,
) -> Result<u64, EnrollmentError> {
    let current = encode_with_limit(row, max_row_bytes)?.len();
    let mut maximum = row.clone();
    maximum.state = InvitationState::UnsupportedProfile;
    maximum.acknowledged = false;
    maximum.dismissed = false;
    maximum.terminal_at = Some(u64::MAX);
    maximum.deferred_join = Some([u8::MAX; 32]);
    let maximum = encode_with_limit(&maximum, max_row_bytes)?.len();
    u64::try_from(
        maximum
            .checked_sub(current)
            .ok_or(EnrollmentError::Corrupt)?,
    )
    .map_err(|_| EnrollmentError::Capacity)
}

/// Every visible generation funds its one local Delete receipt and the largest
/// encoding of its durable fence before ordinary traffic can consume the quota.
pub(super) fn current_reserve(
    row: &rows::Current,
    max_row_bytes: usize,
) -> Result<u64, EnrollmentError> {
    if !row.visible {
        return Ok(0);
    }
    let before = encode_with_limit(row, max_row_bytes)?.len();
    let mut deleted = row.clone();
    // These fixed metadata fields can grow on recovery/Remove/Close/Leave
    // without admitting another member. Counts retain their actual admitted
    // widths; future ordinary growth funds its own new representation.
    deleted.lifecycle = match row.lifecycle {
        GroupLifecycle::Active
        | GroupLifecycle::Leaving
        | GroupLifecycle::Removed
        | GroupLifecycle::Closed => GroupLifecycle::Removed,
    };
    deleted.ready = false;
    deleted.head = Some([u8::MAX; 32]);
    deleted.departure = Some([u8::MAX; 32]);
    deleted.visible = false;
    deleted.purge_through = Some(u32::MAX);
    deleted.invitations_deleted_through = u64::MAX;
    let growth = encode_with_limit(&deleted, max_row_bytes)?
        .len()
        .checked_sub(before)
        .ok_or(EnrollmentError::Corrupt)?;
    u64::try_from(growth)
        .map_err(|_| EnrollmentError::Capacity)?
        .checked_add(receipt_bytes(None, max_row_bytes)?)
        .ok_or(EnrollmentError::Capacity)
}

/// Both the exact receipt and its ordered retirement index are persisted.
/// Maximum identifiers are serializer sizing inputs, never admitted requests.
fn receipt_bytes(
    namespace: Option<rows::NamespaceReceipt>,
    max_row_bytes: usize,
) -> Result<u64, EnrollmentError> {
    let receipt = rows::Receipt {
        request: Request {
            id: [u8::MAX; 32],
            fingerprint: [u8::MAX; 32],
            result: [u8::MAX; 32],
        },
        namespace,
    };
    encode_with_limit(&receipt, max_row_bytes)?
        .len()
        .checked_add(32 + 40 + 32)
        .and_then(|n| u64::try_from(n).ok())
        .ok_or(EnrollmentError::Capacity)
}

pub(super) fn incarnation_reserve(
    row: &rows::Incarnation,
    max_row_bytes: usize,
) -> Result<u64, EnrollmentError> {
    let retired = match row {
        rows::Incarnation::Live(entry) | rows::Incarnation::Archived(entry) => {
            rows::RetiredIncarnation::from_entry(entry, false)
        }
        rows::Incarnation::Retired(_) => return Ok(0),
    };
    let before = encode_with_limit(row, max_row_bytes)?.len();
    let after = encode_with_limit(&rows::Incarnation::Retired(retired), max_row_bytes)?.len();
    u64::try_from(after.saturating_sub(before)).map_err(|_| EnrollmentError::Capacity)
}

/// Reserve the canonical proof row and both exact indexes. Removed also owns
/// a distinct notice/routing row; it never spends the eventual Close audience.
pub(super) fn terminal_bytes(
    statement: &TerminalStatement,
    local: [u8; 16],
    max_row_bytes: usize,
) -> Result<u64, EnrollmentError> {
    let mut statement = statement.clone();
    statement.head = [u8::MAX; 32];
    statement.revision = u64::MAX;
    let removed = matches!(statement.kind, TerminalKind::Removed(_));
    let outgoing = statement.owner.rns == local;
    let row = rows::TerminalRow {
        proof: Terminal {
            statement,
            signature: vec![u8::MAX; 64],
        },
        after: None,
        complete: removed || !outgoing,
    };
    let bytes = encode_with_limit(&row, max_row_bytes)?.len();
    // TERMINALS key32, TERMINAL_GROUPS key64/value32, TERMINAL_SUBJECTS key73/value32.
    let mut reserve = u64::try_from(bytes)
        .map_err(|_| EnrollmentError::Capacity)?
        .checked_add(32 + 64 + 32 + 73 + 32)
        .ok_or(EnrollmentError::Capacity)?;
    reserve = reserve
        .checked_add(terminal_reserve(Some(&row), max_row_bytes)?)
        .ok_or(EnrollmentError::Capacity)?;
    if removed && outgoing {
        reserve = reserve
            .checked_add(notice_bytes(max_row_bytes)?)
            .ok_or(EnrollmentError::Capacity)?;
    }
    if outgoing {
        // Local Remove/Close atomically projects its namespace-backed request
        // result as well as the proof. Received terminal facts do not create a
        // local management request. Namespace widths are fixed and checked.
        reserve = reserve
            .checked_add(receipt_bytes(
                Some(rows::NamespaceReceipt {
                    group: [u8::MAX; 32],
                    generation: u32::MAX,
                    incarnation: [u8::MAX; 24],
                }),
                max_row_bytes,
            )?)
            .ok_or(EnrollmentError::Capacity)?;
    }
    Ok(reserve)
}
