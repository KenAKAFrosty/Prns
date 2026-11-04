use crate::wire::{Request, Response};
use crate::*;
use prns_groups_service::{Fault, GroupService, Incoming, InventoryPage, RecordId};
use prns_groups_store::{AtomicBackend, OperationTicket};

/// Serve one identified inventory/fetch request. Storage work completes before
/// the response is queued. Caller owns dispatch, serialization and shutdown.
pub async fn serve<B: AtomicBackend + Send>(
    network: &Network,
    service: &GroupService<'_, B>,
    job: &RequestJob,
) -> Result<(), Error<B::Error>> {
    let Some(recipient) = network
        .requester(job)
        .filter(|peer| service.readers().contains(peer))
    else {
        return send_error(network, job, RemoteError::Unauthorized).await;
    };
    let request = match Request::decode(job.data(), network.limits()) {
        Ok(request) => request,
        Err(_) => return send_error(network, job, RemoteError::InvalidRequest).await,
    };
    if request.group() != service.group() {
        return send_error(network, job, RemoteError::UnknownGroup).await;
    }
    let frame = match response(service, request, recipient, network.limits()) {
        Ok(frame) => frame,
        Err(error) => {
            let remote = match &error {
                Error::Service(prns_groups_service::Error::Fault(Fault::RestartInventory)) => {
                    RemoteError::RestartInventory
                }
                Error::Service(prns_groups_service::Error::Fault(Fault::InvalidEncoding)) => {
                    RemoteError::InvalidRequest
                }
                Error::Protocol(ProtocolError::Oversized)
                | Error::Service(prns_groups_service::Error::Fault(Fault::Capacity)) => {
                    RemoteError::OverBudget
                }
                _ => RemoteError::Unavailable,
            };
            send_error::<B::Error>(network, job, remote).await?;
            return Err(error);
        }
    };
    network.respond(job, &frame).await?;
    Ok(())
}
fn response<B: AtomicBackend + Send>(
    service: &GroupService<'_, B>,
    request: Request<'_>,
    recipient: Endpoint,
    limits: Limits,
) -> Result<Vec<u8>, Error<B::Error>> {
    match request {
        Request::Capabilities { .. } | Request::Submit { .. } => {
            Ok(Response::Error(RemoteError::InvalidRequest).encode(limits)?)
        }
        Request::Snapshot { .. } => Ok(Response::Snapshot(service.snapshot()?).encode(limits)?),
        Request::Inventory {
            snapshot,
            after,
            kind,
            scan_limit,
            ..
        } => {
            let page = service.inventory(snapshot, after, kind, recipient, scan_limit)?;
            Ok(Response::Inventory(page).encode(limits)?)
        }
        Request::Fetch { id, .. } => match service.envelope(id)? {
            Some(bytes) => Ok(Response::Envelope { id, bytes: &bytes }.encode(limits)?),
            None => Ok(Response::Error(RemoteError::NotFound).encode(limits)?),
        },
    }
}
async fn send_error<E>(
    network: &Network,
    job: &RequestJob,
    error: RemoteError,
) -> Result<(), Error<E>> {
    network
        .respond(job, &Response::Error(error).encode(network.limits())?)
        .await?;
    Ok(())
}

/// Serve through an existing native storage lane without borrowing it while
/// waiting for Reticulum to transmit the response.
pub async fn serve_storage<S: SyncStorage>(
    network: &Network,
    service: &mut S,
    job: &RequestJob,
) -> Result<(), Error<S::Error>> {
    serve_storage_inner(network, service, job, None).await
}

/// Mutable direct service wrapper for authenticated push acceptance. The caller
/// supplies trusted receiver time and store incarnation, never request metadata.
pub async fn serve_mut<B: AtomicBackend + Send>(
    network: &Network,
    service: &mut GroupService<'_, B>,
    job: &RequestJob,
    options: SyncOptions,
) -> Result<(), Error<B::Error>>
where
    B::Error: Send,
{
    serve_storage_with_options(
        network,
        &mut crate::storage::DirectStorage(service),
        job,
        options,
    )
    .await
}

/// Advertise push support and accept at most one bounded envelope. A successful
/// response is queued only after storage and receipt repair finish. Plain Submit
/// acceptance is not an authenticated device receipt.
pub async fn serve_storage_with_options<S: SyncStorage>(
    network: &Network,
    service: &mut S,
    job: &RequestJob,
    options: SyncOptions,
) -> Result<(), Error<S::Error>> {
    if options.budget.max_envelopes == 0 || options.budget.max_bytes == 0 {
        return Err(Error::InvalidBudget);
    }
    serve_storage_inner(network, service, job, Some(options)).await
}

async fn serve_storage_inner<S: SyncStorage>(
    network: &Network,
    service: &mut S,
    job: &RequestJob,
    options: Option<SyncOptions>,
) -> Result<(), Error<S::Error>> {
    let Some(recipient) = network
        .requester(job)
        .filter(|peer| service.is_reader(*peer))
    else {
        return send_error(network, job, RemoteError::Unauthorized).await;
    };
    let request = match Request::decode(job.data(), network.limits()) {
        Ok(request) => request,
        Err(_) => return send_error(network, job, RemoteError::InvalidRequest).await,
    };
    if request.group() != service.group() {
        return send_error(network, job, RemoteError::UnknownGroup).await;
    }
    let response = async {
        match request {
            Request::Capabilities { .. } => {
                let Some(options) = options else {
                    return Ok(
                        Response::Error(RemoteError::InvalidRequest).encode(network.limits())?
                    );
                };
                Ok(Response::Capabilities {
                    snapshot: service.snapshot().await?,
                    max_submit_bytes: options.budget.max_bytes.min(
                        network
                            .limits()
                            .max_frame_bytes
                            .saturating_sub(crate::wire::SUBMIT_OVERHEAD),
                    ),
                }
                .encode(network.limits())?)
            }
            Request::Submit {
                id,
                bytes,
                receipt_bytes,
                ..
            } => {
                let Some(options) = options else {
                    return Ok(
                        Response::Error(RemoteError::InvalidRequest).encode(network.limits())?
                    );
                };
                if bytes.len() > options.budget.max_bytes {
                    return Ok(Response::Error(RemoteError::OverBudget).encode(network.limits())?);
                }
                if service.envelope_id(bytes) != id {
                    return Ok(
                        Response::Error(RemoteError::InvalidRequest).encode(network.limits())?
                    );
                }
                let incoming = service.receive(bytes, options).await?;
                let mut receipt = None;
                if let Incoming::Record(record) = incoming {
                    // Receiving/duplicate repair has already durably retained
                    // this exact receipt. Exceeding a reply allowance leaves it
                    // available for later polling; never regenerate ciphertext.
                    receipt = service.receipt(record).await?.filter(|wire| {
                        wire.len() <= receipt_bytes
                            && wire.len() <= options.budget.max_bytes.saturating_sub(bytes.len())
                    });
                }
                Ok(Response::Submitted {
                    id,
                    accepted: !matches!(incoming, Incoming::Rejected(_)),
                    receipt: receipt.as_deref().unwrap_or_default(),
                }
                .encode(network.limits())?)
            }
            Request::Snapshot { .. } => {
                Ok(Response::Snapshot(service.snapshot().await?).encode(network.limits())?)
            }
            Request::Inventory {
                snapshot,
                after,
                kind,
                scan_limit,
                ..
            } => Ok(Response::Inventory(
                service
                    .inventory(snapshot, after, kind, recipient, scan_limit)
                    .await?,
            )
            .encode(network.limits())?),
            Request::Fetch { id, .. } => match service.envelope(id).await? {
                Some(bytes) => {
                    Ok(Response::Envelope { id, bytes: &bytes }.encode(network.limits())?)
                }
                None => Ok(Response::Error(RemoteError::NotFound).encode(network.limits())?),
            },
        }
    }
    .await;
    match response {
        Ok(frame) => {
            network.respond(job, &frame).await?;
            Ok(())
        }
        Err(error) => {
            let remote = match &error {
                Error::Service(prns_groups_service::Error::Fault(Fault::RestartInventory)) => {
                    RemoteError::RestartInventory
                }
                Error::Service(prns_groups_service::Error::Fault(Fault::InvalidEncoding)) => {
                    RemoteError::InvalidRequest
                }
                Error::Protocol(ProtocolError::Oversized)
                | Error::Service(prns_groups_service::Error::Fault(Fault::Capacity)) => {
                    RemoteError::OverBudget
                }
                _ => RemoteError::Unavailable,
            };
            send_error::<S::Error>(network, job, remote).await?;
            Err(error)
        }
    }
}

/// Pull a stable inventory suffix, records before receipts. A checkpoint only
/// crosses inputs durably accepted/rejected or already known locally. A dropped
/// future/result is safe to restart from an earlier cursor. The scheduler calls
/// peers in both directions to spread content and receipts; this function owns
/// no background task and makes no delivery claim from transport acknowledgments.
pub async fn reconcile<B: AtomicBackend + Send>(
    network: &Network,
    service: &mut GroupService<'_, B>,
    peer: Endpoint,
    group: GroupId,
    options: SyncOptions,
    cursor: Option<SyncCursor>,
) -> Result<SyncReport, Error<B::Error>>
where
    B::Error: Send,
{
    reconcile_storage(
        network,
        &mut crate::storage::DirectStorage(service),
        peer,
        group,
        options,
        cursor,
    )
    .await
}

/// Reconcile through a caller-owned storage lane, releasing it between requests.
pub async fn reconcile_storage<S: SyncStorage>(
    network: &Network,
    service: &mut S,
    peer: Endpoint,
    group: GroupId,
    options: SyncOptions,
    cursor: Option<SyncCursor>,
) -> Result<SyncReport, Error<S::Error>> {
    if options.budget.max_requests < 2
        || options.budget.max_envelopes == 0
        || options.budget.max_bytes == 0
    {
        return Err(Error::InvalidBudget);
    }
    if group != service.group() || !service.is_reader(peer) || peer == service.local_endpoint() {
        return Err(Error::InvalidPeer);
    }
    if cursor.is_some_and(|cursor| {
        cursor.peer != peer || cursor.recipient != service.local_endpoint() || cursor.group != group
    }) {
        return Err(Error::InvalidCursor);
    }
    let peer_binding = network.peer(peer).ok_or(Error::InvalidPeer)?;
    let connection = network.connect(peer_binding).await?;
    let tuning = network.sync_policy().tuning(connection.capabilities());
    let limits = Limits {
        max_page_items: network.limits().max_page_items.min(tuning.max_page_items),
        ..network.limits()
    };
    let options = SyncOptions {
        budget: crate::SyncBudget {
            max_requests: options.budget.max_requests.min(tuning.max_requests),
            max_envelopes: options.budget.max_envelopes.min(tuning.max_envelopes),
            ..options.budget
        },
        ..options
    };
    let mut result = pull(
        &connection,
        limits,
        service,
        peer,
        options,
        cursor,
        tuning.target_bytes,
        network.push_enabled(),
    )
    .await;
    if let Ok(report) = &mut result {
        report.path = connection.capabilities();
    }
    // Drop also queues close if the request fails or the caller cancels.
    let _closed = connection.close().await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn pull<S: SyncStorage>(
    connection: &Connection,
    limits: Limits,
    service: &mut S,
    peer: Endpoint,
    options: SyncOptions,
    cursor: Option<SyncCursor>,
    target_bytes: usize,
    push_enabled: bool,
) -> Result<SyncReport, Error<S::Error>> {
    let group = service.group();
    let mut report = SyncReport {
        push_supported: None,
        path: connection.capabilities(),
        cursor,
        fetched: 0,
        accepted_records: 0,
        accepted_receipts: 0,
        rejected: 0,
        already_known: 0,
        requests: 0,
        bytes: 0,
    };
    // A two-request legacy continuation must still fit inventory + fetch.
    // New peers combine snapshot and capabilities in one bounded response.
    let negotiated = if push_enabled && options.budget.max_requests >= 3 {
        report.requests += 1;
        crate::push::capabilities(connection, group, limits).await?
    } else {
        None
    };
    if push_enabled && options.budget.max_requests >= 3 {
        report.push_supported = Some(negotiated.is_some());
    }
    let max_submit = negotiated.map_or(0, |(_, maximum)| maximum);
    let mut cursor = match cursor {
        Some(cursor) if !cursor.is_complete() => cursor,
        previous => {
            let bytes;
            let snapshot_response = if let Some((snapshot, _)) = negotiated {
                Response::Snapshot(snapshot)
            } else {
                bytes = exchange(
                    connection,
                    Request::Snapshot { group },
                    limits,
                    &mut report,
                    46,
                )
                .await?;
                Response::decode(&bytes, limits)?
            };
            match snapshot_response {
                Response::Snapshot(Some(snapshot)) => {
                    refreshed_cursor(previous, snapshot, peer, service.local_endpoint(), group)
                }
                Response::Snapshot(None) => {
                    report.cursor = None;
                    return Ok(report);
                }
                Response::Error(error) => return Err(Error::Remote(error)),
                _ => return Err(ProtocolError::InvalidEncoding.into()),
            }
        }
    };
    loop {
        cursor = match next_scan(cursor) {
            Scan::Pending(cursor) => cursor,
            Scan::Complete(cursor) => {
                report.cursor = Some(cursor);
                return Ok(report);
            }
        };
        report.cursor = Some(cursor);
        if report.requests >= options.budget.max_requests
            || report.fetched >= options.budget.max_envelopes
            || (report.fetched > 0 && report.bytes >= target_bytes)
        {
            return Ok(report);
        }
        let request = Request::Inventory {
            group,
            snapshot: cursor.snapshot,
            after: cursor.after,
            kind: cursor.kind,
            scan_limit: limits.max_page_items,
        };
        let bytes = exchange(
            connection,
            request,
            limits,
            &mut report,
            57 + 77 * limits.max_page_items,
        )
        .await?;
        let page = match Response::decode(&bytes, limits)? {
            Response::Inventory(page) => page,
            Response::Error(error) => return Err(Error::Remote(error)),
            _ => return Err(ProtocolError::InvalidEncoding.into()),
        };
        validate_page(&page, cursor, limits)?;
        for item in page.items {
            if service.known(item, options.incarnation).await? {
                report.already_known += 1;
                if item.kind == InventoryKind::Record {
                    submit_receipt(
                        connection,
                        limits,
                        service,
                        item.record,
                        options,
                        max_submit,
                        &mut report,
                    )
                    .await?;
                }
                cursor.after = item.position;
                report.cursor = Some(cursor);
                continue;
            }
            if item.bytes > options.budget.max_bytes {
                return Err(Error::EnvelopeBudget {
                    required_bytes: item.bytes,
                    cursor: Box::new(cursor),
                });
            }
            if report.requests >= options.budget.max_requests
                || report.fetched >= options.budget.max_envelopes
                || item.bytes > options.budget.max_bytes.saturating_sub(report.bytes)
                || (report.fetched > 0 && item.bytes > target_bytes.saturating_sub(report.bytes))
            {
                return Ok(report);
            }
            let bytes = exchange(
                connection,
                Request::Fetch { group, id: item.id },
                limits,
                &mut report,
                41 + item.bytes,
            )
            .await?;
            let wire = match Response::decode(&bytes, limits)? {
                Response::Envelope { id, bytes }
                    if id == item.id
                        && bytes.len() == item.bytes
                        && service.envelope_id(bytes) == id =>
                {
                    bytes
                }
                Response::Error(error) => return Err(Error::Remote(error)),
                Response::Envelope { .. } => return Err(ProtocolError::WrongEnvelope.into()),
                _ => return Err(ProtocolError::InvalidEncoding.into()),
            };
            let incoming = service.receive(wire, options).await?;
            report.fetched += 1;
            report.bytes += wire.len();
            match incoming {
                Incoming::Record(record)
                    if item.kind == InventoryKind::Record && record == item.record =>
                {
                    report.accepted_records += 1;
                    submit_receipt(
                        connection,
                        limits,
                        service,
                        record,
                        options,
                        max_submit,
                        &mut report,
                    )
                    .await?;
                }
                Incoming::Receipt(receipt)
                    if item.kind == InventoryKind::Receipt && receipt.record == item.record =>
                {
                    report.accepted_receipts += 1;
                }
                Incoming::Rejected(_) => {
                    report.rejected += 1;
                }
                Incoming::Record(_) | Incoming::Receipt(_) => {
                    return Err(ProtocolError::WrongEnvelope.into())
                }
            }
            cursor.after = item.position;
            report.cursor = Some(cursor);
        }
        cursor.after = page.next;
    }
}
/// The exact local receipt returns over the already authenticated pull Link.
/// If the pass is exhausted it remains in durable inventory for recovery. A
/// response loss may repeat this Submit; the receiver deduplicates the envelope.
#[allow(clippy::too_many_arguments)]
async fn submit_receipt<S: SyncStorage>(
    connection: &Connection,
    limits: Limits,
    service: &mut S,
    record: RecordId,
    options: SyncOptions,
    max_submit: usize,
    report: &mut SyncReport,
) -> Result<(), Error<S::Error>> {
    if max_submit == 0 || report.requests >= options.budget.max_requests {
        return Ok(());
    }
    let Some(wire) = service.receipt(record).await? else {
        return Ok(());
    };
    if wire.len() > max_submit
        || wire.len()
            > limits
                .max_frame_bytes
                .saturating_sub(crate::wire::SUBMIT_OVERHEAD)
        || wire.len() > options.budget.max_bytes.saturating_sub(report.bytes)
    {
        return Ok(());
    }
    let id = service.envelope_id(&wire);
    report.requests += 1;
    report.bytes += wire.len();
    let response = crate::push::submit(connection, limits, service.group(), id, &wire, 0).await?;
    match Response::decode(&response, limits)? {
        Response::Submitted {
            id: returned,
            accepted: _,
            receipt,
        } if returned == id && receipt.is_empty() => Ok(()),
        Response::Error(error) => Err(Error::Remote(error)),
        _ => Err(ProtocolError::WrongEnvelope.into()),
    }
}

enum Scan {
    Pending(SyncCursor),
    Complete(SyncCursor),
}

fn refreshed_cursor(
    previous: Option<SyncCursor>,
    snapshot: prns_groups_service::InventorySnapshot,
    peer: Endpoint,
    recipient: Endpoint,
    group: GroupId,
) -> SyncCursor {
    if let Some(old) = previous.filter(|old| old.is_complete() && old.snapshot == snapshot) {
        // Keep byte-identical checkpoints for unchanged polls so persistence
        // adapters can avoid a needless durable write.
        return old;
    }
    // Compaction changes generation; rollback may lower the head. Neither may
    // reuse coverage from the old ledger. This conservative fallback is wire
    // compatible with peers that know nothing about incremental checkpoints.
    let base = previous
        .filter(|old| {
            old.is_complete()
                && old.snapshot.generation == snapshot.generation
                && old.snapshot.head <= snapshot.head
        })
        .map_or(InventoryCursor(0), |old| old.snapshot.head);
    SyncCursor {
        peer,
        recipient,
        group,
        snapshot,
        base,
        kind: InventoryKind::Record,
        after: base,
    }
}

fn next_scan(mut cursor: SyncCursor) -> Scan {
    if cursor.after != cursor.snapshot.head {
        return Scan::Pending(cursor);
    }
    match cursor.kind {
        InventoryKind::Receipt => Scan::Complete(cursor),
        InventoryKind::Record if cursor.snapshot.head == cursor.base => {
            cursor.kind = InventoryKind::Receipt;
            Scan::Complete(cursor)
        }
        InventoryKind::Record => {
            cursor.kind = InventoryKind::Receipt;
            cursor.after = cursor.base;
            Scan::Pending(cursor)
        }
    }
}
async fn exchange<E>(
    connection: &Connection,
    request: Request<'_>,
    limits: Limits,
    report: &mut SyncReport,
    response_bytes: usize,
) -> Result<Vec<u8>, Error<E>> {
    let frame = request.encode(limits)?;
    report.requests += 1;
    Ok(connection.request_bounded(&frame, response_bytes).await?)
}
fn validate_page(
    page: &InventoryPage,
    cursor: SyncCursor,
    limits: Limits,
) -> Result<(), ProtocolError> {
    if page.snapshot != cursor.snapshot
        || page.next.0 <= cursor.after.0
        || page.next.0 > cursor.snapshot.head.0
        || page.items.len() > limits.max_page_items
    {
        return Err(ProtocolError::InvalidInventory);
    }
    let mut previous = cursor.after;
    for item in &page.items {
        if item.position.0 <= previous.0
            || item.position.0 > page.next.0
            || item.kind != cursor.kind
            || item.bytes == 0
            || item.bytes > limits.max_frame_bytes.saturating_sub(41)
        {
            return Err(ProtocolError::InvalidInventory);
        }
        previous = item.position;
    }
    Ok(())
}
pub(crate) fn ensure_receipt<B: AtomicBackend + Send>(
    service: &mut GroupService<'_, B>,
    record: RecordId,
    incarnation: StoreIncarnation,
) -> Result<(), Error<B::Error>> {
    if service.has_pending_receipt(record)? {
        let operation = next_operation(service, incarnation)?;
        service.emit_receipt(operation, record)?;
    }
    Ok(())
}
pub(crate) fn next_operation<B: AtomicBackend + Send>(
    service: &GroupService<'_, B>,
    incarnation: StoreIncarnation,
) -> Result<prns_groups_service::OperationId, Error<B::Error>> {
    let revision = service
        .coordinator()
        .head()
        .map_err(prns_groups_service::Error::Store)?;
    let sequence = revision.0.checked_add(1).ok_or(Error::OperationExhausted)?;
    Ok(OperationTicket::new(incarnation, sequence)
        .map_err(|_| Error::OperationExhausted)?
        .operation_id())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use prns_groups_service::{EndpointId, EndpointIncarnation, EnvelopeId, EnvelopeInfo};
    #[test]
    fn empty_and_completed_snapshots_finish_without_requesting_an_empty_page() {
        let endpoint = Endpoint {
            id: EndpointId([1; 32]),
            incarnation: EndpointIncarnation::new(1).unwrap(),
        };
        let cursor = SyncCursor {
            peer: endpoint,
            recipient: endpoint,
            group: GroupId([2; 32]),
            snapshot: InventorySnapshot {
                generation: [3; 32],
                head: InventoryCursor(0),
            },
            base: InventoryCursor(0),
            kind: InventoryKind::Record,
            after: InventoryCursor(0),
        };
        assert!(matches!(next_scan(cursor), Scan::Complete(next) if next.is_complete()));
        let end_of_records = SyncCursor {
            snapshot: InventorySnapshot {
                head: InventoryCursor(5),
                ..cursor.snapshot
            },
            after: InventoryCursor(5),
            ..cursor
        };
        assert!(
            matches!(next_scan(end_of_records), Scan::Pending(next) if next.kind == InventoryKind::Receipt && next.after == InventoryCursor(0))
        );
        assert!(matches!(
            next_scan(SyncCursor {
                kind: InventoryKind::Receipt,
                ..end_of_records
            }),
            Scan::Complete(next) if next.is_complete()
        ));
    }
    #[test]
    fn invalid_page_cannot_skip_unexamined_items_or_switch_snapshot() {
        let endpoint = Endpoint {
            id: EndpointId([1; 32]),
            incarnation: EndpointIncarnation::new(1).unwrap(),
        };
        let cursor = SyncCursor {
            peer: endpoint,
            recipient: endpoint,
            group: GroupId([2; 32]),
            snapshot: InventorySnapshot {
                generation: [3; 32],
                head: InventoryCursor(20),
            },
            base: InventoryCursor(0),
            kind: InventoryKind::Record,
            after: InventoryCursor(5),
        };
        let limits = Limits {
            max_frame_bytes: 4096,
            max_page_items: 4,
            max_peers: 3,
            inbound_jobs: 4,
            exchange_timeout: Duration::from_secs(1),
        };
        let mut page = InventoryPage {
            snapshot: cursor.snapshot,
            next: InventoryCursor(9),
            items: vec![EnvelopeInfo {
                position: InventoryCursor(7),
                id: EnvelopeId([4; 32]),
                kind: InventoryKind::Record,
                record: RecordId([5; 32]),
                bytes: 500,
            }],
        };
        assert!(validate_page(&page, cursor, limits).is_ok());
        // Retired positions can create large gaps. The item count remains
        // bounded, and each actual item must remain ordered inside the range.
        page.next = InventoryCursor(20);
        assert!(validate_page(&page, cursor, limits).is_ok());
        page.next = InventoryCursor(21);
        assert_eq!(
            validate_page(&page, cursor, limits),
            Err(ProtocolError::InvalidInventory)
        );
        page.next = InventoryCursor(9);
        page.items[0].position = InventoryCursor(5);
        assert_eq!(
            validate_page(&page, cursor, limits),
            Err(ProtocolError::InvalidInventory)
        );
        page.items[0].position = InventoryCursor(7);
        page.snapshot.generation = [6; 32];
        assert_eq!(
            validate_page(&page, cursor, limits),
            Err(ProtocolError::InvalidInventory)
        );
    }

    #[test]
    fn incremental_passes_share_the_completed_floor_and_reset_on_ledger_change() {
        let endpoint = Endpoint {
            id: EndpointId([1; 32]),
            incarnation: EndpointIncarnation::new(1).unwrap(),
        };
        let old = SyncCursor {
            peer: endpoint,
            recipient: endpoint,
            group: GroupId([2; 32]),
            snapshot: InventorySnapshot {
                generation: [3; 32],
                head: InventoryCursor(200),
            },
            base: InventoryCursor(100),
            kind: InventoryKind::Receipt,
            after: InventoryCursor(200),
        };
        let newer = InventorySnapshot {
            head: InventoryCursor(205),
            ..old.snapshot
        };
        let suffix = refreshed_cursor(Some(old), newer, endpoint, endpoint, old.group);
        assert_eq!(suffix.base, old.snapshot.head);
        assert_eq!(suffix.after, suffix.base);
        assert!(
            matches!(next_scan(SyncCursor { after: newer.head, ..suffix }),
            Scan::Pending(receipts) if receipts.kind == InventoryKind::Receipt && receipts.after == suffix.base)
        );
        let unchanged = refreshed_cursor(Some(old), old.snapshot, endpoint, endpoint, old.group);
        assert!(matches!(next_scan(unchanged), Scan::Complete(next) if next.is_complete()));
        for changed in [
            InventorySnapshot {
                generation: [4; 32],
                ..newer
            },
            InventorySnapshot {
                head: InventoryCursor(199),
                ..old.snapshot
            },
        ] {
            let reset = refreshed_cursor(Some(old), changed, endpoint, endpoint, old.group);
            assert_eq!(reset.base, InventoryCursor(0));
            assert_eq!(reset.after, InventoryCursor(0));
        }
        let pending = SyncCursor {
            kind: InventoryKind::Record,
            ..old
        };
        let reset = refreshed_cursor(Some(pending), newer, endpoint, endpoint, old.group);
        assert_eq!(
            reset.base,
            InventoryCursor(0),
            "unfinished progress is never a completed floor"
        );
    }
}
