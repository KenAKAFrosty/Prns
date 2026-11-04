use super::*;
use crate::{
    ChangeToken, RecordChanges, RecordCursor, RecordDeliveryProgress, RecordPage, RecordSelection,
    StoredRecord,
};
use prns_groups_service::{GroupId, Record};

pub(super) fn progress(
    service: &Service<'_>,
    cfg: &Provisioned,
    id: RecordId,
    clock: ClockReading,
) -> Result<RecordDeliveryProgress, Error> {
    let summary = service.progress(id, clock)?;
    Ok(RecordDeliveryProgress {
        membership_changed: (!service.in_current_era(id)?
            || cfg.lifecycle != crate::GroupLifecycle::Active)
            && !summary.all_received(),
        summary,
    })
}

fn project(
    service: &Service<'_>,
    cfg: &Provisioned,
    record: Record,
    sequence: u64,
    clock: ClockReading,
) -> Result<StoredRecord, Error> {
    let outgoing = record.metadata.author == service.local_endpoint();
    let author_identity = identity(service, cfg, record.metadata.author)?.ok_or(Error::Corrupt)?;
    Ok(StoredRecord {
        progress: progress(service, cfg, record.metadata.record, clock)?,
        client_request_id: outgoing.then_some(crate::RequestId(record.nonce)),
        record,
        position: ChangeToken {
            group: cfg.group,
            incarnation: cfg.incarnation,
            position: sequence,
        },
        author_identity,
        outgoing,
    })
}

pub(super) fn identity(
    service: &Service<'_>,
    cfg: &Provisioned,
    endpoint: prns_groups_core::Endpoint,
) -> Result<Option<prns_host::IdentityHash>, Error> {
    if endpoint == service.local_endpoint() {
        Ok(Some(cfg.local_reticulum_identity))
    } else {
        let mut mappings = cfg
            .peers
            .iter()
            .chain(
                cfg.history_peers
                    .iter()
                    .filter(|_| cfg.owner_identity.is_none()),
            )
            .filter(|peer| peer.endpoint == endpoint)
            .map(|peer| peer.identity);
        let identity = mappings.next();
        if mappings.any(|other| Some(other) != identity) {
            return Err(Error::Corrupt);
        }
        match identity {
            Some(identity) => Ok(Some(identity)),
            None if cfg.owner_identity.is_some() => crate::enrollment::historical_author(
                service.coordinator(),
                cfg.store_limits,
                endpoint,
            )
            .map_err(Error::Enrollment),
            None => Ok(None),
        }
    }
}

/// One selected record is decoded under its profile bounds. Only the returned
/// prefix and its exact identity bindings are allocated/resolved for the page.
pub(super) fn delivery_targets(
    database: &Database,
    service: &Service<'_>,
    cfg: &Provisioned,
    id: RecordId,
    after: Option<prns_groups_core::Endpoint>,
    limit: crate::PageLimits,
) -> Result<Option<crate::DeliveryTargetPage>, Error> {
    let slots = crate::DeliveryTargetPage::capacity(limit).map_err(Error::Api)?;
    #[cfg(feature = "archive")]
    let stored = if crate::archive::selected(database, cfg.local_reticulum_identity)? {
        crate::archive::reads::delivery_targets(database, cfg, id)?
    } else {
        service.delivery_targets(id)?
    };
    #[cfg(not(feature = "archive"))]
    let stored = {
        let _ = database;
        service.delivery_targets(id)?
    };
    let Some(stored) = stored else {
        return Ok(None);
    };
    let first =
        stored.partition_point(|target| after.is_some_and(|after| target.endpoint() <= after));
    let count = slots.min(stored.len() - first);
    let mut targets = Vec::new();
    targets
        .try_reserve_exact(count)
        .map_err(|_| Error::StorageFull)?;
    if targets
        .capacity()
        .checked_mul(std::mem::size_of::<crate::DeliveryTarget>())
        .and_then(|n| n.checked_add(std::mem::size_of::<crate::DeliveryTargetPage>()))
        .is_none_or(|n| n as u64 > limit.max_bytes)
    {
        return Err(Error::InvalidInput);
    }
    for target in &stored[first..first + count] {
        targets.push(crate::DeliveryTarget {
            endpoint: target.endpoint(),
            identity: identity(service, cfg, target.endpoint())?,
            receipt: target.receipt(),
            removal: target.removal(),
        });
    }
    let next = if first + count < stored.len() {
        Some(targets.last().ok_or(Error::Corrupt)?.endpoint)
    } else {
        None
    };
    Ok(Some(crate::DeliveryTargetPage { targets, next }))
}

pub(super) fn page(
    database: &Database,
    service: &Service<'_>,
    cfg: &Provisioned,
    before: Option<RecordCursor>,
    limit: usize,
    clock: ClockReading,
    selection: RecordSelection,
) -> Result<RecordPage, Error> {
    let group = cfg.group;
    if limit == 0
        || limit > 100
        || before
            .is_some_and(|cursor| cursor.group != group || cursor.incarnation != cfg.incarnation)
    {
        return Err(Error::InvalidInput);
    }
    #[cfg(feature = "archive")]
    if crate::archive::selected(database, cfg.local_reticulum_identity)? {
        let head = service.changes(ChangeCursor(0), 1)?.head.0;
        return crate::archive::reads::page(database, cfg, before, limit, clock, selection, head);
    }
    #[cfg(not(feature = "archive"))]
    let _ = database;
    let changes = service.changes(ChangeCursor(0), cfg.service_limits.max_changes)?;
    let before = before.and_then(|cursor| cursor.before);
    if before.is_some_and(|position| position > changes.head.0) {
        return Err(Error::InvalidInput);
    }
    let mut records = Vec::new();
    for change in changes.changes.into_iter().rev() {
        if before.is_some_and(|before| change.cursor.0 >= before) {
            continue;
        }
        let id = match change.kind {
            ChangeKind::Published(id) | ChangeKind::Received(id) => id,
            _ => continue,
        };
        let record = service.record(id)?.ok_or(Error::Corrupt)?;
        if !selection.includes(&record) {
            continue;
        }
        records.push(project(service, cfg, record, change.cursor.0, clock)?);
        if records.len() > limit {
            break;
        }
    }
    let next_before = if records.len() > limit {
        records.pop();
        records
            .last()
            .map(|record| RecordCursor::from(record.position))
    } else {
        None
    };
    Ok(RecordPage {
        records,
        visited_heads: vec![ChangeToken {
            group,
            incarnation: cfg.incarnation,
            position: changes.head.0,
        }],
        current_incarnation: cfg.incarnation,
        next_before,
    })
}

pub(super) fn record(
    database: &Database,
    service: &Service<'_>,
    cfg: &Provisioned,
    group: GroupId,
    id: RecordId,
    clock: ClockReading,
) -> Result<Option<StoredRecord>, Error> {
    if group != cfg.group {
        return Err(Error::InvalidInput);
    }
    #[cfg(feature = "archive")]
    if crate::archive::selected(database, cfg.local_reticulum_identity)? {
        return crate::archive::reads::record(database, cfg, id, clock);
    }
    #[cfg(not(feature = "archive"))]
    let _ = database;
    let Some(record) = service.record(id)? else {
        return Ok(None);
    };
    let changes = service.changes(ChangeCursor(0), cfg.service_limits.max_changes)?;
    let sequence = changes
        .changes
        .into_iter()
        .find_map(|change| match change.kind {
            ChangeKind::Published(found) | ChangeKind::Received(found) if found == id => {
                Some(change.cursor.0)
            }
            _ => None,
        })
        .ok_or(Error::Corrupt)?;
    project(service, cfg, record, sequence, clock).map(Some)
}

pub(super) fn changes(
    service: &Service<'_>,
    cfg: &Provisioned,
    group: GroupId,
    after: Option<ChangeToken>,
    limit: usize,
) -> Result<RecordChanges, Error> {
    if group != cfg.group
        || limit == 0
        || limit > 100
        || after
            .is_some_and(|cursor| cursor.group != group || cursor.incarnation != cfg.incarnation)
    {
        return Err(Error::InvalidInput);
    }
    let position = after.map_or(0, |cursor| cursor.position);
    let page = service.changes(ChangeCursor(position), limit)?;
    let mut previous = position;
    let mut history_pruned = false;
    for change in &page.changes {
        history_pruned |= previous.checked_add(1) != Some(change.cursor.0);
        previous = change.cursor.0;
    }
    history_pruned |= page.next.0 > previous;
    let token = |position| ChangeToken {
        group,
        incarnation: cfg.incarnation,
        position,
    };
    Ok(RecordChanges {
        changes: page.changes,
        next: token(page.next.0),
        head: token(page.head.0),
        history_pruned,
    })
}

#[cfg(feature = "text")]
pub(crate) fn message(item: StoredRecord) -> Result<Message, Error> {
    let record = item.record;
    Ok(Message {
        membership_changed: item.progress.membership_changed,
        id: record.metadata.record,
        client_request_id: item.client_request_id,
        position: item.position,
        author: item.author_identity,
        text: prns_group_text::Text::decode(&record.body)
            .map_err(|_| Error::Corrupt)?
            .as_str()
            .to_owned(),
        outgoing: item.outgoing,
        received_count: item.progress.summary.received,
        recipient_count: item.progress.summary.original_targets,
        sent_at_millis: record
            .metadata
            .validity
            .created_at()
            .0
            .checked_mul(1000)
            .ok_or(Error::Corrupt)?,
    })
}

/// Text is only a decoder over the generic retained record projection.
#[cfg(feature = "text")]
pub(crate) fn messages(page: RecordPage) -> Result<MessagePage, Error> {
    let messages = page
        .records
        .into_iter()
        .map(message)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MessagePage {
        messages,
        visited_heads: page.visited_heads,
        current_incarnation: page.current_incarnation,
        next_before: page.next_before,
    })
}
