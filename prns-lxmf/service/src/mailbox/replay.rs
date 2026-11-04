//! Exact queued-generation paging; accepted rows remain durable between turns.
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxReplayRequest {
    pub after: Option<u64>,
    pub limit: usize,
}

pub(super) fn page(
    database: &Database,
    request: MailboxReplayRequest,
) -> Result<MailboxReply, MailboxFailure> {
    let read = database
        .begin_read()
        .map_err(|e| classify_redb_error(e.into(), "read queued mailbox page"))?;
    let header = policy::read(&read)?;
    let capacity = policy::page_capacity(header.limits, request.limit)?;
    if capacity == 0 {
        return Err(MailboxFailure::Capacity);
    }
    let rows = read
        .open_table(LXMF_MESSAGES)
        .map_err(|e| classify_redb_error(e.into(), "open mailbox rows"))?;
    let queue = read
        .open_table(policy::QUEUED)
        .map_err(|e| classify_redb_error(e.into(), "open mailbox queued index"))?;
    let meta = read
        .open_table(LXMF_META)
        .map_err(|e| classify_redb_error(e.into(), "open mailbox metadata"))?;
    let revision = required_meta(&meta, LXMF_REVISION_KEY)?;
    let mut messages = policy::allocate_page(header.limits, capacity)?;
    let mut next = None;
    let mut last = request.after.unwrap_or(0);
    for item in queue
        .range((Bound::Excluded(last), Bound::Unbounded))
        .map_err(|e| classify_redb_error(e.into(), "scan queued mailbox index"))?
    {
        let (k, g) =
            item.map_err(|e| classify_redb_error(e.into(), "read queued mailbox index"))?;
        if messages.len() == capacity {
            next = Some(last);
            break;
        }
        let id = k.value();
        let value = rows
            .get(id)
            .map_err(|e| classify_redb_error(e.into(), "read queued mailbox row"))?
            .ok_or_else(|| MailboxFailure::reset_required("queued mailbox row is missing"))?;
        let row = decode_record(id, value.value())?;
        if policy::queued_generation(&row) != Some(g.value()) {
            return Err(MailboxFailure::reset_required(
                "queued mailbox generation mismatch",
            ));
        }
        messages.push(public_message(id, row));
        last = id;
    }
    policy::check_page(header.limits, &messages)?;
    Ok(MailboxReply::QueuedPage {
        messages,
        next,
        revision,
    })
}
